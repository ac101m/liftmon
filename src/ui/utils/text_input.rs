//! The text input element: a line of text the user types, wraps across rows
//! when it (or the terminal) grows narrow, and confirms with Enter — or backs
//! out with Escape. The counterpart to the multiple-choice list.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

use super::{Element, ElementResult};

/// A single-line (but wrapping) text entry. Editing keys move the caret;
/// Enter returns whatever has been typed so far, Escape cancels.
pub(crate) struct TextInput {
    prompt: Option<String>,
    /// Text typed so far.
    buffer: String,
    /// Byte index into `buffer` where the caret sits (always a char boundary).
    cursor: usize,
    /// Column width of the last render; the caret's row/column (and hence how
    /// tall the box needs to be) is computed against this width.
    last_width: u16,
}

impl TextInput {
    pub(crate) fn new(prompt: Option<String>, initial: &str) -> Self {
        Self {
            prompt,
            buffer: initial.to_owned(),
            cursor: initial.len(),
            last_width: 0,
        }
    }

    /// What the user has typed, for the shell to harvest after an Escape.
    pub(crate) fn text(&self) -> String {
        self.buffer.clone()
    }

    /// Move the caret one char backwards; at the start it stays put.
    fn move_cursor_left(&mut self) {
        self.cursor = self.buffer[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }

    /// Move the caret one char forwards; at the end it stays put.
    fn move_cursor_right(&mut self) {
        if self.cursor < self.buffer.len() {
            self.cursor += self.buffer[self.cursor..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
        }
    }
}

/// Row/column of the caret when `before` chars precede it across rows of
/// `width` columns (one column per character).
fn caret_col_row(before: usize, width: u16) -> (u16, u16) {
    let width = width.max(1) as usize;
    ((before % width) as u16, (before / width) as u16)
}

/// How many rows `n` characters occupy when wrapped across `width` columns,
/// at least one.
fn text_rows(n: usize, width: u16) -> usize {
    if n == 0 {
        return 1;
    }
    n.div_ceil(width.max(1) as usize)
}

impl Element for TextInput {
    fn natural_height(&self, width: u16) -> u16 {
        // Wrapped text rows, plus a row for the caret when it sits exactly at a
        // wrap boundary, plus the top/bottom borders.
        let chars = self.buffer.chars().count();
        let before = self.buffer[..self.cursor].chars().count();
        let caret_row = (before / width.max(1) as usize) + 1;
        text_rows(chars, width).max(caret_row) as u16 + 2
    }

    fn hint(&self) -> String {
        "type · ←/→ move · Esc cancel · Enter confirm · Ctrl+C quit".into()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let prompt = self.prompt.clone().unwrap_or_else(|| "Enter text".into());
        // Same look as the choice box: a ─[header] that reads as part of the
        // top edge, and a plain bottom rule, with no side bars. Text wraps to
        // the column width (trim off) so a long description or a narrow
        // terminal never clips mid-row.
        Paragraph::new(self.buffer.as_str())
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title(format!("─[{prompt}]"))
                    .borders(Borders::TOP | Borders::BOTTOM),
            )
            .render(area, buf);
        self.last_width = area.width;
    }

    /// Where the caret sits, in element coordinates: column within its wrapped
    /// row, on row one-below the top border.
    fn cursor(&self) -> Option<(u16, u16)> {
        let before = self.buffer[..self.cursor].chars().count();
        let (col, row) = caret_col_row(before, self.last_width.max(1));
        Some((col, 1 + row))
    }

    fn on_key(&mut self, key: KeyEvent) -> Option<ElementResult> {
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.buffer.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let start = self.buffer[..self.cursor]
                    .char_indices()
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                self.buffer.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            KeyCode::Left => self.move_cursor_left(),
            KeyCode::Right => self.move_cursor_right(),
            KeyCode::Enter => return Some(ElementResult::Value(self.buffer.clone())),
            KeyCode::Esc => return Some(ElementResult::Cancelled),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::TextInput;
    use super::{Element, ElementResult};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(input: &mut TextInput, text: &str) {
        for c in text.chars() {
            input.on_key(key(KeyCode::Char(c)));
        }
    }

    fn esc(input: &mut TextInput) -> Option<ElementResult> {
        input.on_key(key(KeyCode::Esc))
    }

    #[test]
    fn typing_inserts_at_the_caret() {
        let mut input = TextInput::new(None, "");
        type_text(&mut input, "be");
        assert_eq!(input.buffer, "be");
        assert_eq!(input.cursor, 2);

        // Moving left and typing inserts mid-string.
        input.on_key(key(KeyCode::Left));
        input.on_key(key(KeyCode::Char('a')));
        assert_eq!(input.buffer, "bae");
        assert_eq!(input.cursor, 2);
    }

    #[test]
    fn starts_from_initial_text() {
        let mut input = TextInput::new(None, "squat");
        assert_eq!(input.buffer, "squat");
        assert_eq!(input.cursor, 5, "caret sits after the initial text");

        type_text(&mut input, "s");
        assert_eq!(input.buffer, "squats");
        assert_eq!(
            input.on_key(key(KeyCode::Enter)),
            Some(ElementResult::Value("squats".into()))
        );
    }

    #[test]
    fn backspace_removes_the_char_before_the_caret() {
        let mut input = TextInput::new(None, "");
        type_text(&mut input, "abc");
        input.on_key(key(KeyCode::Left));
        input.on_key(key(KeyCode::Backspace)); // removes 'b'
        assert_eq!(input.buffer, "ac");
        assert_eq!(input.cursor, 1);
    }

    #[test]
    fn backspace_at_the_start_does_nothing() {
        let mut input = TextInput::new(None, "");
        input.on_key(key(KeyCode::Backspace));
        assert_eq!(input.buffer, "");
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn arrows_stop_at_the_ends() {
        let mut input = TextInput::new(None, "");
        type_text(&mut input, "hi");
        // Too far left — clamps to the first character.
        input.on_key(key(KeyCode::Left));
        input.on_key(key(KeyCode::Left));
        assert_eq!(input.cursor, 0);
        // Too far right — clamps past the last character.
        input.on_key(key(KeyCode::Right));
        input.on_key(key(KeyCode::Right));
        assert_eq!(input.cursor, 2);
    }

    #[test]
    fn enter_returns_the_typed_text() {
        let mut input = TextInput::new(Some("name".into()), "");
        type_text(&mut input, "squat");
        assert_eq!(
            input.on_key(key(KeyCode::Enter)),
            Some(ElementResult::Value("squat".to_string()))
        );
    }

    #[test]
    fn escape_cancels_the_input() {
        let mut input = TextInput::new(Some("name".into()), "");
        type_text(&mut input, "squat");
        assert_eq!(esc(&mut input), Some(ElementResult::Cancelled));
        assert_eq!(input.text(), "squat", "the partial text is still harvested");
    }

    #[test]
    fn caret_tracks_the_typed_characters() {
        let mut input = TextInput::new(None, "");
        input.render(
            Rect::new(0, 0, 20, 3),
            &mut Buffer::empty(Rect::new(0, 0, 20, 3)),
        );
        assert_eq!(
            input.cursor(),
            Some((0, 1)),
            "caret starts at the first column of the input row"
        );
        type_text(&mut input, "hi");
        assert_eq!(input.cursor(), Some((2, 1)));
        input.on_key(key(KeyCode::Left));
        assert_eq!(input.cursor(), Some((1, 1)));
    }

    #[test]
    fn caret_wraps_onto_the_next_row() {
        let mut input = TextInput::new(None, "");
        input.render(
            Rect::new(0, 0, 4, 6),
            &mut Buffer::empty(Rect::new(0, 0, 4, 6)),
        );
        type_text(&mut input, "abcd");
        assert_eq!(input.cursor(), Some((0, 2)), "caret wraps after 4 cols");
        type_text(&mut input, "e");
        assert_eq!(input.cursor(), Some((1, 2)));
    }

    #[test]
    fn renders_the_buffer_in_a_bordered_box() {
        let mut input = TextInput::new(Some("name".into()), "");
        type_text(&mut input, "squat");
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        input.render(area, &mut buf);

        let row = |y: u16| (0..20).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert!(row(0).starts_with("─[name]"), "header like the choice box");
        assert!(row(1).starts_with("squat"), "typed text on the input row");
        assert!(
            row(2).chars().all(|c| c == '─'),
            "bottom edge is a plain rule, no corners"
        );
    }

    #[test]
    fn wraps_long_text_onto_more_rows_and_grows() {
        let mut input = TextInput::new(Some("desc".into()), "");
        type_text(&mut input, "abcdefgh");
        let width = 5;
        let area = Rect::new(0, 0, width, input.natural_height(width));
        assert_eq!(area.height, 4, "8 chars at 5 cols wrap to 2 rows + borders");
        let mut buf = Buffer::empty(area);
        input.render(area, &mut buf);

        let row = |y: u16| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(1), "abcde", "first wrapped row");
        assert_eq!(row(2).trim_end(), "fgh", "second wrapped row");
        // The caret has wrapped with the text: after the 8th char on the
        // second wrapped row, column 3.
        assert_eq!(input.cursor(), Some((3, 2)));
    }
}
