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
//! Enter submits the whole selection and Escape cancels it; the shell reads the
//! rows back out of the element afterwards via [`CyclePicker::rows`] — the
//! submit payload itself is unused, exactly like text input is harvested on
//! cancel. One consequence of the app's reversed row highlight: a coloured
//! label on the cursor row renders as a colour *block* (reverse video swaps
//! foreground and background), which is fine — that is the row being edited,
//! and the colour reads on every other row.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, StatefulWidget};

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
    /// level.
    rows: Vec<(String, Level)>,
    /// Index into `rows` of the highlighted row.
    selected: usize,
    /// The display width of the longest row name, so every row's level label
    /// starts in the same column.
    name_pad: usize,
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
            state,
        }
    }

    /// Move the highlight by `delta` rows, wrapping around the ends.
    fn move_selected(&mut self, delta: isize) {
        let len = self.rows.len();
        let next = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
        self.selected = next;
        self.state.select(Some(next));
    }

    /// The index of the highlighted row — reported back by
    /// [`crate::ui::utils::Shell::cycle_pick`] so a re-asked picker picks up
    /// where the user left off.
    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    /// The rows and their current levels. The shell reads this back after the
    /// element finishes: the submitted selection on Enter, and the partial
    /// (in-progress) selection on Escape so a "keep editing" caller can re-seed.
    pub(crate) fn rows(&self) -> Vec<(String, Level)> {
        self.rows.clone()
    }
}

impl Element for CyclePicker {
    fn natural_height(&self, _width: u16) -> u16 {
        // One row per option, plus the box's top and bottom border.
        self.rows.len() as u16 + 2
    }

    fn hint(&self) -> String {
        "↑/↓ move · Space cycle · Enter save · Esc back · Ctrl+C quit".into()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let items: Vec<ListItem> = self
            .rows
            .iter()
            .map(|(name, level)| {
                // Left-pad the name to the column the labels line up in; the
                // coloured `· level` span starts there for every selected row.
                let mut spans = vec![Span::raw(format!(" {name:<pad$}", pad = self.name_pad))];
                if let (Some(label), Some(colour)) = (level.label(), level.colour()) {
                    spans.push(Span::styled(format!("· {label}"), Style::new().fg(colour)));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        let title = self
            .title
            .clone()
            .unwrap_or_else(|| "Select an option".into());
        // Title is bracketed to read as a header (═[Main menu]), led by a
        // double border glyph so it reads as part of the top edge; only the
        // top/bottom edges are drawn as double rules, since side ║bars would
        // eat two columns for nothing on a narrow terminal (e.g. a phone over
        // ssh).
        let list = List::new(items)
            .block(
                Block::default()
                    .title(format!("═[{title}]"))
                    .border_type(BorderType::Double)
                    .borders(Borders::TOP | Borders::BOTTOM),
            )
            .highlight_style(Style::new().reversed());
        self.state.select(Some(self.selected));
        list.render(area, buf, &mut self.state);
    }

    fn on_key(&mut self, key: KeyEvent) -> Option<ElementResult> {
        match key.code {
            KeyCode::Up => self.move_selected(-1),
            KeyCode::Down => self.move_selected(1),
            // Space advances the highlighted row to its next level. Fires
            // before any general text arm — this element has none, since there
            // is no typing band (unlike the filter picker).
            KeyCode::Char(' ') => {
                let (_, level) = &mut self.rows[self.selected];
                *level = level.cycle();
            }
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
        picker.on_key(key(KeyCode::Char(' '))); // Front delts: High -> None
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
        let space = key(KeyCode::Char(' '));
        assert_eq!(picker.rows()[0].1, Level::None);

        picker.on_key(space); // None -> Low
        assert_eq!(picker.rows()[0].1, Level::Low);
        picker.on_key(space); // Low -> Medium
        assert_eq!(picker.rows()[0].1, Level::Medium);
        picker.on_key(space); // Medium -> High
        assert_eq!(picker.rows()[0].1, Level::High);
        picker.on_key(space); // High -> None, back around
        assert_eq!(picker.rows()[0].1, Level::None);

        // The other rows are untouched by cycling the highlighted one.
        assert_eq!(picker.rows()[1].1, Level::Medium);
        assert_eq!(picker.rows()[2].1, Level::High);
    }

    #[test]
    fn cycling_moves_with_the_highlight() {
        let mut picker = picker();
        picker.on_key(key(KeyCode::Down)); // highlight Biceps
        picker.on_key(key(KeyCode::Char(' '))); // Medium -> High
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
        picker.on_key(key(KeyCode::Char(' ')));
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
        picker.on_key(key(KeyCode::Char(' '))); // Abs -> Low
        assert_eq!(
            picker.on_key(key(KeyCode::Esc)),
            Some(ElementResult::Cancelled)
        );
        assert_eq!(picker.rows()[0].1, Level::Low, "the work so far is kept");
    }

    #[test]
    fn other_keys_do_nothing() {
        let mut picker = picker();
        assert_eq!(picker.on_key(key(KeyCode::Char('x'))), None);
        assert_eq!(picker.selected, 0);
        assert_eq!(picker.rows()[0].1, Level::None);
    }

    #[test]
    fn renders_rows_with_aligned_coloured_labels() {
        let mut picker = picker();
        let area = Rect::new(0, 0, 30, 5);
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);

        // Bracketed header on the top border row; bottom edge is a double rule.
        assert!(RowText::at(&buf, 0).starts_with("═[Muscle groups]"));
        assert!(RowText::at(&buf, 4).chars().all(|c| c == '═'));

        // Each selected row puts its label in the aligned column (one leading
        // space + the longest name "Front delts", 11 chars). Unselected "Abs"
        // (row 1) shows no label at all.
        let label_col: u16 = 1 + picker.name_pad as u16;
        assert_eq!(char_at(&RowText::at(&buf, 1), label_col), ' ');
        assert_eq!(char_at(&RowText::at(&buf, 2), label_col), '·');
        assert_eq!(char_at(&RowText::at(&buf, 3), label_col), '·');
        assert_eq!(
            RowText::at(&buf, 2).trim_end(),
            " Biceps     · medium",
            "Biceps (6 chars) pads to the 11-char column with five spaces"
        );
        assert_eq!(RowText::at(&buf, 3).trim_end(), " Front delts· high");

        // The label spans carry the level colour: yellow for medium, red for
        // high, and no level colour anywhere on the unselected row (its row
        // is the raw name span plus ratatui's blank padding, neither of which
        // is Reversed).
        assert_eq!(buf[(label_col, 2)].style().fg, Some(Color::Yellow));
        assert_eq!(buf[(label_col, 3)].style().fg, Some(Color::Red));
        for x in 0..buf.area.width {
            let fg = buf[(x, 1)].style().fg;
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
    fn the_highlighted_row_is_reversed_and_keeps_the_label_colour() {
        let mut picker = picker();
        picker.on_key(key(KeyCode::Down)); // highlight Biceps (medium label)
        let area = Rect::new(0, 0, 30, 5);
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);

        let label_col: u16 = 1 + picker.name_pad as u16;
        // The cursor row is reversed, and its label keeps its fg — the merge
        // leaves the colour set, reverse video merely inverts it on screen.
        assert!(
            buf[(0, 2)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert_eq!(buf[(label_col, 2)].style().fg, Some(Color::Yellow));
        assert!(
            !buf[(0, 1)]
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
