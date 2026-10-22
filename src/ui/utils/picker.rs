//! The picker element: a filter box on top of a multiple-choice list.
//!
//! The user's typing lives in a single-line filter row, prefixed with a "> "
//! prompt and showing the element's (dimmed) suggestion while it is empty —
//! e.g. "type to filter"; the list below shows every option whose name
//! contains the text (case-insensitively), so an empty filter lists everything
//! and the list narrows as they type. The box keeps a constant height
//! (`min(5, options)` list rows) so it never changes size while filtering;
//! the matches scroll within that fixed window. Up/Down move the highlight
//! through the filtered list, Enter picks the highlighted match, and Escape
//! cancels.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{
    Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget,
};

use super::{Element, ElementResult};

/// Which options match a filter (case-insensitive substring).
fn matches_filter(option: &str, filter_lower: &str) -> bool {
    filter_lower.is_empty() || option.to_lowercase().contains(filter_lower)
}

/// How many matches the list shows at once: `min(5, total options)`. The box
/// keeps this many rows no matter how the filter narrows, so it never changes
/// size while typing (up to five on screen, fewer when there are fewer
/// options to begin with).
const MAX_VISIBLE: usize = 5;

/// State of the filterable list while it is being answered.
pub(crate) struct Picker {
    /// The box title in the top border, e.g. "Select exercise".
    title: Option<String>,
    /// The suggestion shown dimmed in the filter row while it is empty (e.g.
    /// "type to filter"); `None` leaves the row just the "> " prompt.
    prompt: Option<String>,
    /// Every option, in their given order; the filter narrows this.
    all: Vec<String>,
    /// The typed filter.
    filter: String,
    /// Byte index into `filter` where the caret sits (a char boundary).
    filter_cursor: usize,
    /// Index into the current matches of the highlighted row.
    selected: usize,
    /// Column width of the last render; used to clip the filter row.
    last_width: u16,
    /// Scroll/selection state for the underlying ratatui `List`.
    state: ListState,
}

impl Picker {
    /// Build a picker over `options`; they must not be empty (empty options
    /// would be unresolvable, so [`super::shell::Shell::pick`] asserts it).
    pub(crate) fn new(title: Option<String>, prompt: Option<String>, options: Vec<String>) -> Self {
        let mut state = ListState::default();
        state.select(Some(0));
        Self {
            title,
            prompt,
            all: options,
            filter: String::new(),
            filter_cursor: 0,
            selected: 0,
            last_width: 0,
            state,
        }
    }

    /// The fixed number of list rows this picker draws: `min(5, total
    /// options)`. The shell gives the element exactly this much room, so the
    /// box keeps its size while the filter narrows the matches.
    fn visible_rows(&self) -> usize {
        self.all.len().min(MAX_VISIBLE)
    }

    /// The options currently matching the filter, in their original order.
    fn matches(&self) -> Vec<&str> {
        let filter_lower = self.filter.to_lowercase();
        self.all
            .iter()
            .filter(|option| matches_filter(option, &filter_lower))
            .map(String::as_str)
            .collect()
    }

    /// Move the highlight by `delta` rows, wrapping around the ends. With no
    /// matches there is nothing to move.
    fn move_selected(&mut self, delta: isize) {
        let matches = self.matches();
        let len = matches.len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
        self.state.select(Some(self.selected));
    }

    /// The text to draw on the (single-line) filter row and the caret's column:
    /// the window of `width` characters ending at the caret, so the caret
    /// never slides off the right edge while typing past the width.
    fn visible_filter(&self, width: u16) -> (String, u16) {
        if width == 0 {
            return (String::new(), 0);
        }
        let before = self.filter[..self.filter_cursor].chars().count();
        let start = before.saturating_sub(width as usize - 1);
        let text: String = self
            .filter
            .chars()
            .skip(start)
            .take(width as usize)
            .collect();
        (text, (before - start) as u16)
    }

    /// Move the caret one char backwards; at the start it stays put.
    fn move_cursor_left(&mut self) {
        self.filter_cursor = self.filter[..self.filter_cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }

    /// Move the caret one char forwards; at the end it stays put.
    fn move_cursor_right(&mut self) {
        if self.filter_cursor < self.filter.len() {
            self.filter_cursor += self.filter[self.filter_cursor..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
        }
    }

    /// The filter changed, so the matches may have narrowed: start the
    /// highlight back at the top of the new list.
    fn filter_changed(&mut self) {
        self.selected = 0;
        self.state.select(Some(0));
    }
}

impl Element for Picker {
    fn natural_height(&self, _width: u16) -> u16 {
        // Top border, the filter row, the matches (or a no-match hint), bottom
        // border. The list keeps its fixed `visible_rows` even as the filter
        // narrows, so the box does not resize while typing.
        2 + 1 + self.visible_rows() as u16
    }

    fn hint(&self) -> String {
        "↑/↓ move · Enter select · Esc back · Ctrl+C quit".into()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        self.last_width = area.width;
        let title = self.title.clone().unwrap_or_else(|| "Select".into());
        let matches = self.matches();
        // (`self.state` is kept in sync with `self.selected` by the editing
        // methods, so the List has nothing extra to set here.)

        // Same look as the choice box: a ─[header] top edge and a plain bottom
        // rule, with no side bars.
        let block = Block::default()
            .title(format!("─[{title}]"))
            .borders(Borders::TOP | Borders::BOTTOM);
        let inner = block.inner(area);
        block.render(area, buf);

        // The filter row: the shared "> " prompt marks the field as an input,
        // and an empty filter shows the element's dimmed suggestion so the
        // prompt is discoverable. The text is clipped to one line (the caret
        // is parked by the shell via `cursor()`).
        let filter_area = Rect::new(inner.x, inner.y, inner.width, 1);
        let field_width = inner.width.saturating_sub(2); // room for the "> " prompt
        let (filter_text, _) = self.visible_filter(field_width);
        let content = if self.filter.is_empty() {
            None
        } else {
            Some(filter_text)
        };
        super::prompt_row(content, self.prompt.as_deref()).render(filter_area, buf);

        // The matches below it (or a hint when nothing matches).
        if matches.is_empty() {
            let hint_area = Rect::new(inner.x, inner.y + 1, inner.width, 1);
            Paragraph::new("no matching exercises")
                .style(Style::new().add_modifier(Modifier::DIM))
                .render(hint_area, buf);
            return;
        }
        let items: Vec<ListItem> = matches
            .iter()
            .map(|name| ListItem::new(format!(" {name}")))
            .collect();
        let list_area = Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(1),
        );
        StatefulWidget::render(
            List::new(items).highlight_style(Style::new().reversed()),
            list_area,
            buf,
            &mut self.state,
        );
    }

    /// Where the caret sits on the filter row, in element coordinates: row 1
    /// (beneath the top border), two columns past the "> " prompt, at its
    /// clipped column.
    fn cursor(&self) -> Option<(u16, u16)> {
        let field_width = self.last_width.saturating_sub(2); // room for "> "
        let (_, x) = self.visible_filter(field_width.max(1));
        Some((x + 2, 1))
    }

    fn on_key(&mut self, key: KeyEvent) -> Option<ElementResult> {
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter.insert(self.filter_cursor, c);
                self.filter_cursor += c.len_utf8();
                self.filter_changed();
            }
            KeyCode::Backspace if self.filter_cursor > 0 => {
                let start = self.filter[..self.filter_cursor]
                    .char_indices()
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                self.filter.replace_range(start..self.filter_cursor, "");
                self.filter_cursor = start;
                self.filter_changed();
            }
            KeyCode::Left => self.move_cursor_left(),
            KeyCode::Right => self.move_cursor_right(),
            KeyCode::Up => self.move_selected(-1),
            KeyCode::Down => self.move_selected(1),
            KeyCode::Enter => {
                // Pick the highlighted match, if any.
                let matches = self.matches();
                if let Some(name) = matches.get(self.selected) {
                    return Some(ElementResult::Value((*name).to_owned()));
                }
            }
            KeyCode::Esc => return Some(ElementResult::Cancelled),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::Picker;
    use super::{Element, ElementResult};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;

    const OPTIONS: [&str; 4] = ["bench press", "deadlift", "incline bench", "squat"];

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn picker() -> Picker {
        Picker::new(
            Some("Select".into()),
            Some("type to filter".into()),
            OPTIONS.iter().map(|s| s.to_string()).collect(),
        )
    }

    /// A picker with more options than can fit on screen at once (to exercise
    /// the fixed five-row window).
    fn big_picker() -> Picker {
        Picker::new(
            Some("Select".into()),
            Some("type to filter".into()),
            (0..17).map(|i| format!("exercise {i:02}")).collect(),
        )
    }

    fn type_text(input: &mut Picker, text: &str) {
        for c in text.chars() {
            input.on_key(key(KeyCode::Char(c)));
        }
    }

    fn names(picker: &Picker) -> Vec<&str> {
        picker.matches()
    }

    #[test]
    fn empty_filter_lists_all_options() {
        let picker = picker();
        assert_eq!(names(&picker), OPTIONS.to_vec());
    }

    #[test]
    fn typing_narrows_matches_case_insensitively() {
        let mut picker = picker();
        type_text(&mut picker, "beNch");
        assert_eq!(names(&picker), vec!["bench press", "incline bench"]);
        // The filter is the whole string, so typing on narrows it further.
        type_text(&mut picker, " p");
        assert_eq!(names(&picker), vec!["bench press"]);
    }

    #[test]
    fn typing_resets_selection_to_the_first_match() {
        let mut picker = picker();
        picker.on_key(key(KeyCode::Down));
        picker.on_key(key(KeyCode::Down)); // selected = 2
        assert_eq!(picker.selected, 2, "sanity: we moved the highlight");

        type_text(&mut picker, "e");
        assert_eq!(picker.selected, 0, "filtering jumps back to the top");
        assert_eq!(
            names(&picker),
            vec!["bench press", "deadlift", "incline bench"]
        );
    }

    #[test]
    fn up_down_do_nothing_with_no_matches() {
        let mut picker = picker();
        type_text(&mut picker, "zzz");
        assert!(names(&picker).is_empty());
        picker.on_key(key(KeyCode::Down));
        picker.on_key(key(KeyCode::Up));
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn up_down_move_and_wrap_through_the_matches() {
        let mut picker = picker();
        picker.on_key(key(KeyCode::Down));
        assert_eq!(picker.selected, 1);
        picker.on_key(key(KeyCode::Down));
        assert_eq!(picker.selected, 2);
        picker.on_key(key(KeyCode::Down));
        picker.on_key(key(KeyCode::Down)); // wraps past the end
        assert_eq!(picker.selected, 0);
        picker.on_key(key(KeyCode::Up)); // wraps past the start
        assert_eq!(picker.selected, OPTIONS.len() - 1);
    }

    #[test]
    fn enter_returns_the_highlighted_filtered_match() {
        let mut picker = picker();
        type_text(&mut picker, "sq");
        picker.on_key(key(KeyCode::Down)); // 0 -> 1 (wraps back to 0 with one match)
        let result = picker.on_key(key(KeyCode::Enter));
        assert_eq!(result, Some(ElementResult::Value("squat".to_string())));
    }

    #[test]
    fn enter_with_no_matches_does_nothing() {
        let mut picker = picker();
        type_text(&mut picker, "zzz");
        assert_eq!(picker.on_key(key(KeyCode::Enter)), None);
    }

    #[test]
    fn escape_cancels_the_picker() {
        let mut picker = picker();
        type_text(&mut picker, "ben");
        assert_eq!(
            picker.on_key(key(KeyCode::Esc)),
            Some(ElementResult::Cancelled)
        );
    }

    #[test]
    fn backspace_edits_and_reapplies_the_filter() {
        let mut picker = picker();
        type_text(&mut picker, "bencx");
        assert!(names(&picker).is_empty(), "the stray 'x' kills every match");

        picker.on_key(key(KeyCode::Backspace)); // removes the 'x'
        assert_eq!(names(&picker), vec!["bench press", "incline bench"]);
    }

    #[test]
    fn arrow_keys_move_the_caret_through_the_filter() {
        let mut picker = picker();
        type_text(&mut picker, "abc");

        // Move back to the start and type there — text is inserted at the
        // caret, so this lands the 'x' in front.
        picker.on_key(key(KeyCode::Left));
        picker.on_key(key(KeyCode::Left));
        picker.on_key(key(KeyCode::Left));
        picker.on_key(key(KeyCode::Char('x')));
        assert_eq!(picker.filter, "xabc");

        // Backspace removes the char before the caret (the 'x').
        picker.on_key(key(KeyCode::Backspace));
        assert_eq!(picker.filter, "abc");
    }

    #[test]
    fn natural_height_is_fixed_while_the_filter_narrows() {
        let mut picker = big_picker(); // 17 options -> five list rows
        assert_eq!(picker.natural_height(40), 2 + 1 + 5);

        type_text(&mut picker, "exercise 1"); // narrows to a handful
        assert_eq!(picker.natural_height(40), 2 + 1 + 5);

        type_text(&mut picker, " 15"); // now nothing matches
        assert_eq!(picker.natural_height(40), 2 + 1 + 5);
    }

    #[test]
    fn natural_height_is_min_of_five_when_fewer_options() {
        // Four options: the fixed count is four, and it stays put (no
        // hint-only collapse) even when nothing matches.
        let mut picker = picker();
        assert_eq!(picker.natural_height(40), 2 + 1 + OPTIONS.len() as u16);

        type_text(&mut picker, "squ");
        assert_eq!(picker.natural_height(40), 2 + 1 + OPTIONS.len() as u16);

        type_text(&mut picker, "zzz");
        assert_eq!(picker.natural_height(40), 2 + 1 + OPTIONS.len() as u16);
    }

    #[test]
    fn renders_fixed_list_rows_and_scrolls_the_highlight_into_view() {
        let mut picker = big_picker();
        let area = Rect::new(0, 0, 40, picker.natural_height(40));

        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);
        let row = |y: u16| (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(2).trim_end(), " exercise 00");
        assert_eq!(row(6).trim_end(), " exercise 04", "box fits five rows");
        assert!(row(7).chars().all(|c| c == '─'), "bottom rule");

        // March the highlight off the bottom edge; the list scrolls to keep
        // the selection in view without the box changing size.
        for _ in 0..5 {
            picker.on_key(key(KeyCode::Down));
        }
        assert_eq!(picker.selected, 5);
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);
        let row = |y: u16| (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(2).trim_end(), " exercise 01", "list scrolled");
        assert_eq!(row(6).trim_end(), " exercise 05", "highlight in view");
    }

    #[test]
    fn renders_filter_row_and_matches_with_the_selection_highlighted() {
        let mut picker = picker();
        let area = Rect::new(0, 0, 40, picker.natural_height(40));
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);

        let row = |y: u16| (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert!(
            row(0).starts_with("─[Select]"),
            "header like the choice box"
        );
        assert_eq!(
            row(1).trim_end(),
            "> type to filter",
            "an empty filter shows the prompt placeholder"
        );
        assert!(row(2).trim_end().starts_with(" bench press"));
        assert!(row(3).trim_end().starts_with(" deadlift"));
        assert_eq!(row(5).trim_end(), " squat");

        // The first match (selected) is reversed; the second is not.
        assert!(
            buf[(0, 2)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert!(
            !buf[(0, 3)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED)
        );

        // The bottom edge is a plain rule, no corners.
        let bottom = area.height - 1;
        assert!(row(bottom).chars().all(|c| c == '─'));
    }

    #[test]
    fn renders_typed_filter_on_the_filter_row() {
        let mut picker = picker();
        type_text(&mut picker, "ben");
        let area = Rect::new(0, 0, 40, picker.natural_height(40));
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);

        let row1: String = (0..40).map(|x| buf[(x, 1)].symbol()).collect();
        assert_eq!(
            row1.trim_end(),
            "> ben",
            "typed filter shows on row 1 after the prompt"
        );
        assert_eq!(picker.cursor(), Some((5, 1)), "caret after the filter text");

        // Matches narrowed to the two 'bench' exercises.
        let row2: String = (0..40).map(|x| buf[(x, 2)].symbol()).collect();
        assert!(row2.trim_end().starts_with(" bench press"));
    }

    #[test]
    fn empty_filter_shows_a_dim_placeholder_that_yields_to_typing() {
        let mut picker = picker();
        let area = Rect::new(0, 0, 40, picker.natural_height(40));
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);

        let row1: String = (0..40).map(|x| buf[(x, 1)].symbol()).collect();
        assert_eq!(row1.trim_end(), "> type to filter");
        assert!(
            buf[(2, 1)].style().add_modifier.contains(Modifier::DIM),
            "the placeholder is dim so it doesn't read as typed input"
        );

        // Typing replaces the placeholder; the prompt prefix stays.
        type_text(&mut picker, "ben");
        buf.reset();
        picker.render(area, &mut buf);
        let row1: String = (0..40).map(|x| buf[(x, 1)].symbol()).collect();
        assert_eq!(row1.trim_end(), "> ben");
        assert_eq!(picker.cursor(), Some((5, 1)));
    }

    #[test]
    fn picker_without_a_prompt_shows_a_bare_filter_row() {
        let mut picker = Picker::new(
            Some("Select".into()),
            None,
            OPTIONS.iter().map(|s| s.to_string()).collect(),
        );
        let area = Rect::new(0, 0, 40, picker.natural_height(40));
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);

        let row1: String = (0..40).map(|x| buf[(x, 1)].symbol()).collect();
        assert_eq!(
            row1.trim_end(),
            ">",
            "no suggestion, just the prompt gutter — the field is still marked"
        );
    }

    #[test]
    fn renders_a_dim_hint_when_nothing_matches() {
        let mut picker = picker();
        type_text(&mut picker, "zzz");
        let area = Rect::new(0, 0, 40, picker.natural_height(40));
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);

        let row2: String = (0..40).map(|x| buf[(x, 2)].symbol()).collect();
        assert_eq!(row2.trim_end(), "no matching exercises");
        assert!(
            buf[(0, 2)].style().add_modifier.contains(Modifier::DIM),
            "the hint is dimmed so it doesn't read as a selectable option"
        );
    }
}
