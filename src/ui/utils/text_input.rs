//! The text input element: a line of text the user types, wraps across rows
//! when it (or the terminal) grows narrow, and confirms with Enter — or backs
//! out with Escape. The entry row is prefixed with the shared "> " prompt, and
//! (when the input has one) an empty field shows its dimmed suggestion. The
//! counterpart to the multiple-choice list.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use super::{Element, ElementResult};

/// A single-line (but wrapping) text entry. Editing keys move the caret;
/// Enter returns whatever has been typed so far, Escape cancels.
pub(crate) struct TextInput {
    /// The box title in the top border; `None` shows the generic "Enter text"
    /// title.
    title: Option<String>,
    /// The suggestion shown dimmed after the "> " prompt while the field is
    /// empty (e.g. "e.g. 100"); `None` leaves the row just the prompt.
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
    pub(crate) fn new(title: Option<String>, prompt: Option<String>, initial: &str) -> Self {
        Self {
            title,
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

/// The virtual column the `before`th text character occupies: the "> " prompt
/// takes the first two columns of the first row.
fn caret_virtual_col(before: usize) -> usize {
    before + 2
}

/// The 1-based text-row (among the text rows) that the caret sits on when
/// `before` characters precede it, wrapping at `width` columns per row — used
/// by `natural_height` so the box grows a row when the caret lands exactly on
/// a wrap boundary.
fn caret_row(before: usize, width: u16) -> usize {
    let width = width.max(1) as usize;
    caret_virtual_col(before) / width + 1
}

/// Column within its text row and the 0-based text-row index the caret sits at
/// when `before` characters precede it, across rows of `width` columns (one
/// column per character): the virtual column laid out over those rows.
fn caret_col_row(before: usize, width: u16) -> (u16, u16) {
    let width = width.max(1) as usize;
    let virtual_col = caret_virtual_col(before);
    ((virtual_col % width) as u16, (virtual_col / width) as u16)
}

/// How many text rows `n` characters occupy when the first row holds
/// `width - 2` columns (after the "> " prompt) and every later row holds
/// `width`, at least one.
fn text_rows(n: usize, width: u16) -> usize {
    if n == 0 {
        return 1;
    }
    let width = width.max(1) as usize;
    // Text starts at virtual column 2, so the n-th character ends at virtual
    // column n + 1; the row that column lands on is the last text row.
    (n + 1) / width + 1
}

impl Element for TextInput {
    fn natural_height(&self, width: u16) -> u16 {
        // Wrapped text rows — the first holds `width - 2` columns after the
        // "> " prompt — kept at least as tall as the row the caret sits on,
        // plus the top/bottom borders.
        let chars = self.buffer.chars().count();
        let before = self.buffer[..self.cursor].chars().count();
        let caret_row = caret_row(before, width);
        text_rows(chars, width).max(caret_row) as u16 + 2
    }

    fn hint(&self) -> String {
        "type · ←/→ move · Esc cancel · Enter confirm · Ctrl+C quit".into()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let title = self.title.clone().unwrap_or_else(|| "Enter text".into());
        // Same look as the choice box: a ─[header] that reads as part of the
        // top edge, and a plain bottom rule, with no side bars. The entry row
        // starts with the "> " prompt gutter (see [`super::prompt_row`]), then
        // the typed text or, when the field is empty and the input has one,
        // its dimmed suggestion.
        //
        // The text is wrapped into rows by hand — row 0 holds `width - 2`
        // columns after the gutter, every later row `width` — exactly as the
        // caret and height are computed, so a long description or a narrow
        // terminal never clips mid-row. (ratatui's own `Wrap` is not used: it
        // treats a line led by a prompt gutter by shunting the whole text onto
        // its own row, so it can't be relied on to line up with the caret.)
        let width = area.width.max(1) as usize;
        let first = width.saturating_sub(2); // text columns on row 0
        let chars: Vec<char> = self.buffer.chars().collect();
        let mut lines = Vec::new();
        if chars.is_empty() {
            lines.push(super::prompt_row(None, self.prompt.as_deref()));
        } else {
            let (head, rest) = chars.split_at(first.min(chars.len()));
            lines.push(super::prompt_row(Some(head.iter().collect()), None));
            lines.extend(
                rest.chunks(width)
                    .map(|chunk| Line::from(Span::raw(chunk.iter().collect::<String>()))),
            );
        }
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!("─[{title}]"))
                    .borders(Borders::TOP | Borders::BOTTOM),
            )
            .render(area, buf);
        self.last_width = area.width;
    }

    /// Where the caret sits, in element coordinates: column within its wrapped
    /// row, on the row one-below the top border. The "> " prompt shifts the
    /// caret two columns right on the first row.
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
    use ratatui::style::Modifier;

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
        let mut input = TextInput::new(None, None, "");
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
        let mut input = TextInput::new(None, None, "squat");
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
        let mut input = TextInput::new(None, None, "");
        type_text(&mut input, "abc");
        input.on_key(key(KeyCode::Left));
        input.on_key(key(KeyCode::Backspace)); // removes 'b'
        assert_eq!(input.buffer, "ac");
        assert_eq!(input.cursor, 1);
    }

    #[test]
    fn backspace_at_the_start_does_nothing() {
        let mut input = TextInput::new(None, None, "");
        input.on_key(key(KeyCode::Backspace));
        assert_eq!(input.buffer, "");
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn arrows_stop_at_the_ends() {
        let mut input = TextInput::new(None, None, "");
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
        let mut input = TextInput::new(Some("name".into()), None, "");
        type_text(&mut input, "squat");
        assert_eq!(
            input.on_key(key(KeyCode::Enter)),
            Some(ElementResult::Value("squat".to_string()))
        );
    }

    #[test]
    fn escape_cancels_the_input() {
        let mut input = TextInput::new(Some("name".into()), None, "");
        type_text(&mut input, "squat");
        assert_eq!(esc(&mut input), Some(ElementResult::Cancelled));
        assert_eq!(input.text(), "squat", "the partial text is still harvested");
    }

    #[test]
    fn caret_tracks_the_typed_characters() {
        let mut input = TextInput::new(None, None, "");
        input.render(
            Rect::new(0, 0, 20, 3),
            &mut Buffer::empty(Rect::new(0, 0, 20, 3)),
        );
        assert_eq!(
            input.cursor(),
            Some((2, 1)),
            "caret starts just after the '> ' prompt on the input row"
        );
        type_text(&mut input, "hi");
        assert_eq!(input.cursor(), Some((4, 1)));
        input.on_key(key(KeyCode::Left));
        assert_eq!(input.cursor(), Some((3, 1)));
    }

    #[test]
    fn caret_wraps_onto_the_next_row() {
        let mut input = TextInput::new(None, None, "");
        input.render(
            Rect::new(0, 0, 4, 6),
            &mut Buffer::empty(Rect::new(0, 0, 4, 6)),
        );
        type_text(&mut input, "abcd");
        assert_eq!(
            input.cursor(),
            Some((2, 2)),
            "the '> ' prompt leaves 2 rows-worth... 2 columns on row 0, so the caret wraps after 2 chars"
        );
        type_text(&mut input, "e");
        assert_eq!(input.cursor(), Some((3, 2)));
    }

    #[test]
    fn renders_the_buffer_in_a_bordered_box() {
        let mut input = TextInput::new(Some("name".into()), None, "");
        type_text(&mut input, "squat");
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        input.render(area, &mut buf);

        let row = |y: u16| (0..20).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert!(row(0).starts_with("─[name]"), "header like the choice box");
        assert!(
            row(1).starts_with("> squat"),
            "typed text on the input row after the '> ' prompt"
        );
        assert!(
            row(2).chars().all(|c| c == '─'),
            "bottom edge is a plain rule, no corners"
        );
    }

    #[test]
    fn wraps_long_text_onto_more_rows_and_grows() {
        let mut input = TextInput::new(Some("desc".into()), None, "");
        type_text(&mut input, "abcdefgh");
        let width = 5;
        let area = Rect::new(0, 0, width, input.natural_height(width));
        assert_eq!(
            area.height, 5,
            "8 chars at 5 cols: 3 fit row 0 (after '> '), 5 fill row 1, an extra row for the caret at the wrap boundary, plus borders"
        );
        let mut buf = Buffer::empty(area);
        input.render(area, &mut buf);

        let row = |y: u16| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(
            row(1),
            "> abc",
            "first wrapped row starts with the '> ' prompt, then 3 chars"
        );
        assert_eq!(row(2).trim_end(), "defgh", "second wrapped row");
        // The caret has wrapped with the text: after the 8th char it lands at
        // the start of the (extra) row past the wrap boundary, column 0.
        assert_eq!(input.cursor(), Some((0, 3)));
    }

    #[test]
    fn empty_input_shows_the_dimmed_suggestion_after_the_prompt() {
        let mut input = TextInput::new(Some("Weight (kg)".into()), Some("e.g. 100".into()), "");
        let area = Rect::new(0, 0, 25, 3);
        let mut buf = Buffer::empty(area);
        input.render(area, &mut buf);

        let row = |y: u16| (0..25).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(1).trim_end(), "> e.g. 100");
        assert!(
            buf[(2, 1)].style().add_modifier.contains(Modifier::DIM),
            "the suggestion is dim so it doesn't read as typed input"
        );
        assert_eq!(
            input.cursor(),
            Some((2, 1)),
            "caret sits right after the '> ' prompt"
        );

        // Typing replaces the suggestion; the prompt gutter stays.
        type_text(&mut input, "100");
        buf.reset();
        input.render(area, &mut buf);
        let row = |y: u16| (0..25).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(1).trim_end(), "> 100");
        assert_eq!(input.cursor(), Some((5, 1)));
    }

    #[test]
    fn empty_input_without_a_suggestion_shows_a_bare_prompt_row() {
        let mut input = TextInput::new(Some("Name".into()), None, "");
        let area = Rect::new(0, 0, 15, 3);
        let mut buf = Buffer::empty(area);
        input.render(area, &mut buf);

        let row = |y: u16| (0..15).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(
            row(1).trim_end(),
            ">",
            "no suggestion, just the prompt gutter — the field is still marked as an input"
        );
    }
}
