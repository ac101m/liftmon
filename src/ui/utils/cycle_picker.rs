//! The cycle picker: a list where the user sets each row's level by scrolling
//! to it and pressing Space, then confirms the whole selection with Enter.
//!
//! Rows carry a [`Level`]: unselected, or one of the three stimulus strengths
//! an exercise can give a muscle group. Space on the highlighted row advances
//! it `None → Low → Medium → High → None`, so one key cycles a group from
//! unselected, up through the stronger levels, and back around. The chosen
//! levels render as a name-aligned trailing label (`· low`, `· medium`,
//! `· high`) coloured green, yellow and red so the grid reads at a glance.
//!
//! A filter band sits under the top border, like the filter picker's: typing
//! narrows which rows are shown, so a group is found by name instead of
//! scrolled to, and the box sizes to its matches. The filter is only a view —
//! a hidden row keeps its level, and Enter always submits (Esc cancels) the
//! whole underlying set, never just the visible matches.
//!
//! Enter submits the whole selection and Escape cancels it; the shell reads the
//! rows back out of the element afterwards via [`CyclePicker::rows`] — the
//! submit payload itself is unused, exactly like text input is harvested on
//! cancel. One consequence of the app's reversed row highlight: a coloured
//! label on the cursor row renders as a colour *block* (reverse video swaps
//! foreground and background), which is fine — that is the row being edited,
//! and the colour reads on every other row.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget,
};

use super::Element;
use super::ElementResult;

/// The level a row carries: unselected, or one of the three stimulus strengths
/// an exercise can give a muscle group (migration v5's `stimulus_type` values,
/// plus the fourth "not selected" state that stores no row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    None,
    Low,
    Medium,
    High,
}

impl Level {
    /// The next level after a Space press, wrapping: None → Low → Medium →
    /// High → None.
    fn cycle(self) -> Level {
        match self {
            Level::None => Level::Low,
            Level::Low => Level::Medium,
            Level::Medium => Level::High,
            Level::High => Level::None,
        }
    }

    /// The trailing label a selected row shows, or [`None`] for an unselected
    /// row. `pub(crate)` so the menu layer can render the same label the picker
    /// does (e.g. the exercise info screen's muscle-group list).
    pub(crate) fn label(self) -> Option<&'static str> {
        match self {
            Level::None => None,
            Level::Low => Some("low"),
            Level::Medium => Some("medium"),
            Level::High => Some("high"),
        }
    }

    /// The colour a selected row's label renders as: green for low, yellow for
    /// medium, red for high — the same intensity language the session summary's
    /// difficulty markers use. `None` for an unselected row, which shows no
    /// label. `pub(crate)` so the menu layer can reuse the picker's colour
    /// language.
    pub(crate) fn colour(self) -> Option<Color> {
        match self {
            Level::None => None,
            Level::Low => Some(Color::Green),
            Level::Medium => Some(Color::Yellow),
            Level::High => Some(Color::Red),
        }
    }

    /// How strong the level's stimulus reads, for ordering an assigned-group
    /// list strongest-first: high outranks medium, medium outranks low. The
    /// unselected state ranks lowest — it never appears in an ordered display,
    /// since unselected rows are dropped before ordering. `pub(crate)` so the
    /// menu layer can order the same levels the picker cycles.
    pub(crate) fn strength(self) -> u8 {
        match self {
            Level::None => 0,
            Level::Low => 1,
            Level::Medium => 2,
            Level::High => 3,
        }
    }
}

/// State of the cycle picker while it is being answered.
pub(crate) struct CyclePicker {
    /// The box title in the top border; `None` shows the generic "Select an
    /// option" title.
    title: Option<String>,
    /// The rows in display order: each muscle group's name and its current
    /// level. The filter band below only hides some of them from view — this
    /// stays the authoritative set the picker submits.
    rows: Vec<(String, Level)>,
    /// Index into `rows` (the whole set) of the highlighted row.
    selected: usize,
    /// The display width of the longest row name across the whole set, so every
    /// row's level label starts in the same column even while the filter hides
    /// rows.
    name_pad: usize,
    /// The typed filter.
    filter: String,
    /// Byte index into `filter` where the caret sits (a char boundary).
    filter_cursor: usize,
    /// Column width of the last render; used to clip the filter row.
    last_width: u16,
    /// Scroll/selection state for the underlying ratatui `List`.
    state: ListState,
}

impl CyclePicker {
    /// Build a cycle picker over `rows` with `selected` as the row the
    /// highlight rests on — what a re-asking caller uses to keep a re-asked
    /// picker on the row just used (like [`crate::ui::utils::Choice::with_selected`
    /// does for menus, via [`crate::ui::utils::Shell::ask_selected`]).
    /// The seed is clamped into range, so a stale index from a picker whose
    /// rows have since changed length cannot point past the end.
    pub(crate) fn with_selected(
        title: Option<String>,
        rows: Vec<(String, Level)>,
        selected: usize,
    ) -> Self {
        let name_pad = rows
            .iter()
            .map(|(name, _)| name.chars().count())
            .max()
            .unwrap_or(0);
        let selected = selected.min(rows.len().saturating_sub(1));
        let mut state = ListState::default();
        state.select(Some(selected));
        Self {
            title,
            rows,
            selected,
            name_pad,
            filter: String::new(),
            filter_cursor: 0,
            last_width: 0,
            state,
        }
    }

    /// The real row indices whose name matches the filter, in row order; an
    /// empty filter lists every row. Case-insensitive substring matching, like
    /// the filter picker's.
    fn matches(&self) -> Vec<usize> {
        let filter_lower = self.filter.to_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, (name, _))| {
                filter_lower.is_empty() || name.to_lowercase().contains(&filter_lower)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Where the highlighted row sits among the matches, or `None` when it is
    /// filtered out of view (e.g. nothing matches).
    fn selected_position(&self) -> Option<usize> {
        self.matches().iter().position(|&i| i == self.selected)
    }

    /// Move the highlight by `delta` rows through the matches, wrapping around
    /// the ends. With no matches there is nothing to move.
    fn move_selected(&mut self, delta: isize) {
        let matches = self.matches();
        let len = matches.len();
        if len == 0 {
            return;
        }
        let pos = self.selected_position().unwrap_or(0);
        let next = (pos as isize + delta).rem_euclid(len as isize) as usize;
        self.selected = matches[next];
        self.state.select(Some(next));
    }

    /// The index of the highlighted row — reported back by
    /// [`crate::ui::utils::Shell::cycle_pick`] so a re-asked picker picks up
    /// where the user left off. Always a real index into the whole set, so a
    /// re-ask (with an empty filter) lands on the same row regardless of how
    /// the previous picker was filtered.
    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    /// The rows and their current levels. The shell reads this back after the
    /// element finishes: the submitted selection on Enter, and the partial
    /// (in-progress) selection on Escape so a "keep editing" caller can re-seed.
    /// The whole set is always reported, visible and filtered-out rows alike,
    /// so filtering can never drop an assignment.
    pub(crate) fn rows(&self) -> Vec<(String, Level)> {
        self.rows.clone()
    }

    /// The filter text to draw and the caret's column: the window of `width`
    /// characters ending at the caret, so the caret never slides off the right
    /// edge while typing past the width.
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

    /// Move the caret to the very start of the filter.
    fn move_cursor_home(&mut self) {
        self.filter_cursor = 0;
    }

    /// Move the caret past the last character (to the end of the filter).
    fn move_cursor_end(&mut self) {
        self.filter_cursor = self.filter.len();
    }

    /// The filter changed, so the matches may have narrowed: keep the highlight
    /// on the row being worked on while it still matches, otherwise jump to the
    /// first match (or clear the selection when nothing matches).
    fn filter_changed(&mut self) {
        let matches = self.matches();
        if let Some(pos) = self.selected_position() {
            self.state.select(Some(pos));
        } else if let Some(&first) = matches.first() {
            self.selected = first;
            self.state.select(Some(0));
        } else {
            self.state.select(None);
        }
    }
}

impl Element for CyclePicker {
    fn natural_height(&self, _width: u16) -> u16 {
        // Top border, the filter band, the rows shown (or a no-match hint), and
        // the bottom border — so the box narrows as the filter narrows, like the
        // filter picker, and an empty filter shows every row.
        2 + 1 + self.matches().len().max(1) as u16
    }

    fn hint(&self) -> String {
        "type to filter · ↑/↓ move · Space cycle · Enter save · Esc back · Ctrl+C quit".into()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        self.last_width = area.width;
        let title = self
            .title
            .clone()
            .unwrap_or_else(|| "Select an option".into());
        // Same look as the filter picker and the choice box: a ═[header] top edge
        // and a double bottom rule, with no side bars — drawn explicitly here so
        // the filter band can sit between the borders and the rows.
        let block = Block::default()
            .title(format!("═[{title}]"))
            .border_type(BorderType::Double)
            .borders(Borders::TOP | Borders::BOTTOM);
        let inner = block.inner(area);
        block.render(area, buf);

        // The filter band: the shared "> " prompt marks the field as an input,
        // and an empty filter shows the element's dimmed suggestion so the
        // prompt is discoverable. The caret is parked here by the shell via
        // `cursor()`.
        let filter_area = Rect::new(inner.x, inner.y, inner.width, 1);
        let field_width = inner.width.saturating_sub(2); // room for the "> " prompt
        let (filter_text, _) = self.visible_filter(field_width);
        let content = if self.filter.is_empty() {
            None
        } else {
            Some(filter_text)
        };
        super::prompt_row(content, Some("type to filter")).render(filter_area, buf);

        let matches = self.matches();
        if matches.is_empty() {
            let hint_area = Rect::new(inner.x, inner.y + 1, inner.width, 1);
            Paragraph::new("no matching groups")
                .style(Style::new().add_modifier(Modifier::DIM))
                .render(hint_area, buf);
            return;
        }
        let items: Vec<ListItem> = matches
            .iter()
            .map(|&i| {
                let (name, level) = &self.rows[i];
                // Left-pad the name to the whole-set column the labels line up
                // in (stable even while the filter hides rows); the coloured
                // `· level` span starts there for every selected row.
                let mut spans = vec![Span::raw(format!(" {name:<pad$}", pad = self.name_pad))];
                if let (Some(label), Some(colour)) = (level.label(), level.colour()) {
                    spans.push(Span::styled(format!("· {label}"), Style::new().fg(colour)));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        let list_area = Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(1),
        );
        self.state.select(self.selected_position());
        StatefulWidget::render(
            List::new(items).highlight_style(Style::new().reversed()),
            list_area,
            buf,
            &mut self.state,
        );
    }

    /// Where the caret sits on the filter band, in element coordinates: row 1
    /// (beneath the top border), two columns past the "> " prompt, at its
    /// clipped column.
    fn cursor(&self) -> Option<(u16, u16)> {
        let field_width = self.last_width.saturating_sub(2); // room for "> "
        let (_, x) = self.visible_filter(field_width.max(1));
        Some((x + 2, 1))
    }

    fn on_key(&mut self, key: KeyEvent) -> Option<ElementResult> {
        match key.code {
            KeyCode::Up => self.move_selected(-1),
            KeyCode::Down => self.move_selected(1),
            // Space advances the highlighted row to its next level. Only a
            // visible row can be cycled — a row the filter has hidden stays
            // put — and this arm must come before the typing one, which would
            // otherwise treat Space as text.
            KeyCode::Char(' ') if self.selected_position().is_some() => {
                let (_, level) = &mut self.rows[self.selected];
                *level = level.cycle();
            }
            KeyCode::Char(' ') => {}
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
            KeyCode::Home => self.move_cursor_home(),
            KeyCode::End => self.move_cursor_end(),
            // The submit payload is unused: the shell reads the whole selection
            // back out of the element afterwards (see [`CyclePicker::rows`]),
            // so a real value would be thrown away.
            KeyCode::Enter => {
                return Some(ElementResult::Value(String::new()));
            }
            // Backing out without committing. The screen decides what that means
            // (usually its "back" option).
            KeyCode::Esc => return Some(ElementResult::Cancelled),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn space() -> KeyEvent {
        key(KeyCode::Char(' '))
    }

    fn picker() -> CyclePicker {
        CyclePicker::with_selected(
            Some("Muscle groups".into()),
            vec![
                ("Abs".into(), Level::None),
                ("Biceps".into(), Level::Medium),
                ("Front delts".into(), Level::High),
            ],
            0,
        )
    }

    fn type_text(input: &mut CyclePicker, text: &str) {
        for c in text.chars() {
            input.on_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn up_down_moves_highlight_and_wraps_around() {
        let mut picker = picker();
        assert_eq!(picker.selected, 0, "starts on the first row");

        picker.on_key(key(KeyCode::Down)); // 0 -> 1
        assert_eq!(picker.selected, 1);
        picker.on_key(key(KeyCode::Down)); // 1 -> 2
        assert_eq!(picker.selected, 2);
        picker.on_key(key(KeyCode::Down)); // wraps 2 -> 0
        assert_eq!(picker.selected, 0);
        picker.on_key(key(KeyCode::Up)); // wraps 0 -> 2
        assert_eq!(picker.selected, 2);
    }

    #[test]
    fn with_selected_starts_and_reports_the_given_highlight() {
        let mut picker = CyclePicker::with_selected(
            None,
            vec![
                ("Abs".into(), Level::None),
                ("Biceps".into(), Level::Medium),
                ("Front delts".into(), Level::High),
            ],
            2,
        );
        assert_eq!(picker.selected, 2, "the seed row holds the highlight");
        assert_eq!(picker.selected(), 2, "reported back to the caller");
        // Cycling acts on the seeded row, not the top one.
        picker.on_key(space()); // Front delts: High -> None
        assert_eq!(picker.rows()[0].1, Level::None, "Abs untouched");
        assert_eq!(picker.rows()[2].1, Level::None);

        // A stale seed beyond the last row is clamped, exactly like Choice.
        let clamped = CyclePicker::with_selected(
            None,
            vec![("Abs".into(), Level::None), ("Biceps".into(), Level::High)],
            5,
        );
        assert_eq!(
            clamped.selected, 1,
            "out-of-range seed clamps to the last row"
        );
    }

    #[test]
    fn space_cycles_only_the_highlighted_row_through_all_four_levels() {
        let mut picker = picker();
        assert_eq!(picker.rows()[0].1, Level::None);

        picker.on_key(space()); // None -> Low
        assert_eq!(picker.rows()[0].1, Level::Low);
        picker.on_key(space()); // Low -> Medium
        assert_eq!(picker.rows()[0].1, Level::Medium);
        picker.on_key(space()); // Medium -> High
        assert_eq!(picker.rows()[0].1, Level::High);
        picker.on_key(space()); // High -> None, back around
        assert_eq!(picker.rows()[0].1, Level::None);

        // The other rows are untouched by cycling the highlighted one.
        assert_eq!(picker.rows()[1].1, Level::Medium);
        assert_eq!(picker.rows()[2].1, Level::High);
    }

    #[test]
    fn cycling_moves_with_the_highlight() {
        let mut picker = picker();
        picker.on_key(key(KeyCode::Down)); // highlight Biceps
        picker.on_key(space()); // Medium -> High
        assert_eq!(
            picker.rows()[0].1,
            Level::None,
            "the first row is untouched"
        );
        assert_eq!(picker.rows()[1].1, Level::High);
    }

    #[test]
    fn enter_returns_value_and_the_rows_read_back() {
        let mut picker = picker();
        picker.on_key(key(KeyCode::Down));
        picker.on_key(space());
        assert_eq!(
            picker.on_key(key(KeyCode::Enter)),
            Some(ElementResult::Value(String::new())),
            "submit payload is a placeholder; the selection comes from rows()"
        );
        assert_eq!(picker.rows()[1].1, Level::High);
    }

    #[test]
    fn escape_returns_cancelled_and_leaves_state_intact() {
        let mut picker = picker();
        picker.on_key(space()); // Abs -> Low
        assert_eq!(
            picker.on_key(key(KeyCode::Esc)),
            Some(ElementResult::Cancelled)
        );
        assert_eq!(picker.rows()[0].1, Level::Low, "the work so far is kept");
    }

    #[test]
    fn typing_narrows_the_visible_rows_but_keeps_the_whole_set_intact() {
        let mut picker = picker();
        type_text(&mut picker, "bi");
        assert_eq!(picker.matches(), vec![1], "only Biceps matches");
        assert_eq!(picker.selected, 1, "the first match holds the highlight");
        assert_eq!(
            picker.rows().len(),
            3,
            "rows() is the whole set, never just the matches"
        );
        // The two rows hidden by the filter keep their levels while out of view.
        assert_eq!(picker.rows()[0].1, Level::None);
        assert_eq!(picker.rows()[2].1, Level::High);
    }

    #[test]
    fn typing_jumps_the_highlight_to_the_first_match_when_the_old_row_drops_out() {
        let mut picker = picker(); // highlighted Abs (index 0)
        type_text(&mut picker, "bi");
        assert_eq!(
            picker.selected, 1,
            "Abs stopped matching, so Biceps takes the highlight"
        );
        assert_eq!(
            picker.rows()[0].1,
            Level::None,
            "the hidden row is untouched"
        );
    }

    #[test]
    fn typing_keeps_the_highlight_when_the_row_still_matches() {
        let mut picker = picker();
        picker.on_key(key(KeyCode::Down)); // Biceps (index 1)
        type_text(&mut picker, "bi");
        assert_eq!(
            picker.selected, 1,
            "Biceps still matches, so the highlight stays on it"
        );
    }

    #[test]
    fn up_down_move_through_the_matches_only_and_wrap() {
        let mut picker = picker();
        type_text(&mut picker, "e"); // Biceps (1) and Front delts (2) remain; Abs drops out
        assert_eq!(picker.matches(), vec![1, 2]);
        assert_eq!(picker.selected, 1);

        picker.on_key(key(KeyCode::Down)); // 1 -> 2
        assert_eq!(picker.selected, 2);
        picker.on_key(key(KeyCode::Down)); // wraps 2 -> 1
        assert_eq!(picker.selected, 1);
        picker.on_key(key(KeyCode::Up)); // wraps 1 -> 2
        assert_eq!(picker.selected, 2);
        assert_eq!(
            picker.rows()[0].1,
            Level::None,
            "Abs stays hidden throughout"
        );
    }

    #[test]
    fn a_filtered_row_cycles_but_hidden_rows_stay_put() {
        let mut picker = picker();
        type_text(&mut picker, "bi"); // Biceps only
        picker.on_key(space());
        assert_eq!(
            picker.rows()[1].1,
            Level::High,
            "Biceps cycles: medium -> high"
        );
        assert_eq!(picker.rows()[0].1, Level::None, "hidden Abs is untouched");
        assert_eq!(
            picker.rows()[2].1,
            Level::High,
            "hidden Front delts is untouched"
        );
    }

    #[test]
    fn space_does_nothing_with_no_matches() {
        let mut picker = picker();
        type_text(&mut picker, "zzz"); // nothing matches
        picker.on_key(space());
        assert_eq!(picker.rows()[0].1, Level::None, "no visible row to cycle");
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn backspace_edits_the_filter_and_re_widens_the_matches() {
        let mut picker = picker();
        type_text(&mut picker, "bix");
        assert!(picker.matches().is_empty(), "the stray 'x' hides every row");

        picker.on_key(key(KeyCode::Backspace)); // removes the 'x'
        assert_eq!(picker.matches(), vec![1], "Biceps is back in view");
    }

    #[test]
    fn other_keys_edit_the_filter_band() {
        let mut picker = picker();
        type_text(&mut picker, "abs");
        assert_eq!(picker.filter, "abs");
        assert_eq!(picker.matches(), vec![0]);

        // Home then typing inserts at the front, like the filter picker.
        picker.on_key(key(KeyCode::Home));
        type_text(&mut picker, "x");
        assert_eq!(picker.filter, "xabs");
        picker.on_key(key(KeyCode::Backspace));
        assert_eq!(picker.filter, "abs");
    }

    #[test]
    fn natural_height_sizes_to_the_matches() {
        let mut picker = picker(); // 3 rows
        assert_eq!(
            picker.natural_height(30),
            2 + 1 + 3,
            "empty filter: every row"
        );
        type_text(&mut picker, "bi");
        assert_eq!(
            picker.natural_height(30),
            2 + 1 + 1,
            "one match: band + row + borders"
        );
        type_text(&mut picker, "zz");
        assert_eq!(
            picker.natural_height(30),
            2 + 1 + 1,
            "nothing matches: the hint row keeps the same height"
        );
    }

    #[test]
    fn renders_rows_with_aligned_coloured_labels() {
        let mut picker = picker();
        let area = Rect::new(0, 0, 30, picker.natural_height(30));
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);

        // Bracketed header on the top border row; bottom edge is a double rule.
        assert!(RowText::at(&buf, 0).starts_with("═[Muscle groups]"));
        assert!(RowText::at(&buf, 5).chars().all(|c| c == '═'));

        // The filter band sits under the header; the rows start below it.
        assert_eq!(RowText::at(&buf, 1).trim_end(), ">   type to filter");

        // Each selected row puts its label in the aligned column (one leading
        // space + the longest name "Front delts", 11 chars). Unselected "Abs"
        // (row 2) shows no label at all.
        let label_col: u16 = 1 + picker.name_pad as u16;
        assert_eq!(char_at(&RowText::at(&buf, 2), label_col), ' ');
        assert_eq!(char_at(&RowText::at(&buf, 3), label_col), '·');
        assert_eq!(char_at(&RowText::at(&buf, 4), label_col), '·');
        assert_eq!(
            RowText::at(&buf, 3).trim_end(),
            " Biceps     · medium",
            "Biceps (6 chars) pads to the 11-char column with five spaces"
        );
        assert_eq!(RowText::at(&buf, 4).trim_end(), " Front delts· high");

        // The label spans carry the level colour: yellow for medium, red for
        // high, and no level colour anywhere on the unselected row (its row
        // is the raw name span plus ratatui's blank padding, neither of which
        // is Reversed).
        assert_eq!(buf[(label_col, 3)].style().fg, Some(Color::Yellow));
        assert_eq!(buf[(label_col, 4)].style().fg, Some(Color::Red));
        for x in 0..buf.area.width {
            let fg = buf[(x, 2)].style().fg;
            assert!(
                !matches!(
                    fg,
                    Some(Color::Green) | Some(Color::Yellow) | Some(Color::Red)
                ),
                "unselected row carries no level colour at column {x} (got {fg:?})"
            );
        }
    }

    #[test]
    fn renders_a_filtered_list_and_the_typed_filter() {
        let mut picker = picker();
        type_text(&mut picker, "bi");
        let area = Rect::new(0, 0, 30, picker.natural_height(30));
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);

        let row = |y: u16| (0..30).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(
            row(1).trim_end(),
            "> bi",
            "the typed filter shows on the band"
        );
        assert_eq!(
            row(2).trim_end(),
            " Biceps     · medium",
            "only the match is listed"
        );
        assert!(row(3).chars().all(|c| c == '═'), "bottom rule");
        // The caret parks on the band, two columns past the "> " gutter and the
        // two typed characters.
        assert_eq!(picker.cursor(), Some((4, 1)));
    }

    #[test]
    fn renders_a_dim_hint_when_nothing_matches() {
        let mut picker = picker();
        type_text(&mut picker, "zzz");
        let area = Rect::new(0, 0, 30, picker.natural_height(30));
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);

        let row2: String = (0..30).map(|x| buf[(x, 2)].symbol()).collect();
        assert_eq!(row2.trim_end(), "no matching groups");
        assert!(
            buf[(0, 2)].style().add_modifier.contains(Modifier::DIM),
            "the hint is dimmed so it doesn't read as a selectable option"
        );
    }

    #[test]
    fn the_highlighted_row_is_reversed_and_keeps_the_label_colour() {
        let mut picker = picker();
        picker.on_key(key(KeyCode::Down)); // highlight Biceps (medium label)
        let area = Rect::new(0, 0, 30, picker.natural_height(30));
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);

        let label_col: u16 = 1 + picker.name_pad as u16;
        // The cursor row is reversed, and its label keeps its fg — the merge
        // leaves the colour set, reverse video merely inverts it on screen.
        assert!(
            buf[(0, 3)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert_eq!(buf[(label_col, 3)].style().fg, Some(Color::Yellow));
        assert!(
            !buf[(0, 2)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "unselected rows must not be highlighted"
        );
    }

    // Helper joining a row's cells into a string, mirroring buffer output.
    struct RowText;
    impl RowText {
        fn at(buf: &Buffer, y: u16) -> String {
            (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
        }
    }

    fn char_at(text: &str, x: u16) -> char {
        text.chars().nth(x as usize).unwrap_or(' ')
    }
}
