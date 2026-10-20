//! The multiple-choice element: a numbered list navigated with ↑/↓ and
//! confirmed with Enter.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, StatefulWidget};

use super::Element;
use super::ElementResult;
use super::error::UiError;
use super::shell::Shell;

/// Ask the user to pick one of `options` in a throwaway shell, returning their
/// selection.
///
/// `options` must not be empty. The question (if any) is written to the
/// shell's history area, and also used as the box title around the list.
/// Ctrl+C quits the app (restoring the terminal first).
///
/// Each call opens its own shell session, so history does not carry across
/// calls. Callers that want a persistent history (e.g. a main menu that logs
/// between questions) hold a [`Shell`] themselves and use [`Shell::ask`].
pub fn get_user_selection<S: AsRef<str>>(
    prompt: Option<String>,
    options: impl IntoIterator<Item = S>,
) -> String {
    let mut shell = match Shell::new() {
        Ok(shell) => shell,
        Err(err) => {
            eprintln!("error: could not start the terminal UI: {err}");
            std::process::exit(1);
        }
    };

    match shell.ask(prompt, options) {
        Ok(selection) => selection,
        Err(err) => {
            let interrupted = matches!(err, UiError::Interrupted);
            // `process::exit` skips destructors, so restore the terminal before
            // leaving (exit code 130 = the conventional SIGINT code).
            drop(shell);
            if interrupted {
                std::process::exit(130);
            }
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    }
}

/// State of the multiple-choice list while it is being answered.
pub(crate) struct Choice {
    /// Question text; `None` shows the generic "Select an option" title.
    prompt: Option<String>,
    options: Vec<String>,
    /// Index into `options` of the highlighted row.
    selected: usize,
    /// Scroll/selection state for the underlying ratatui `List`.
    state: ListState,
}

impl Choice {
    /// Build a choice list; `options` must not be empty.
    pub(crate) fn new(prompt: Option<String>, options: Vec<String>) -> Self {
        let mut state = ListState::default();
        state.select(Some(0));
        Self {
            prompt,
            options,
            selected: 0,
            state,
        }
    }

    /// Move the highlight by `delta` rows, wrapping around the ends.
    fn move_selected(&mut self, delta: isize) {
        let len = self.options.len();
        let next = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
        self.selected = next;
        self.state.select(Some(next));
    }
}

impl Element for Choice {
    fn natural_height(&self, _width: u16) -> u16 {
        // One row per option, plus the box's top and bottom border.
        self.options.len() as u16 + 2
    }

    fn hint(&self) -> String {
        "↑/↓ move · Enter select · Ctrl+C quit".into()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let items: Vec<ListItem> = self
            .options
            .iter()
            .enumerate()
            .map(|(i, option)| ListItem::new(format!("{}. {option}", i + 1)))
            .collect();
        let prompt = self
            .prompt
            .clone()
            .unwrap_or_else(|| "Select an option".into());
        // Title is bracketed to read as a header (─[Main menu]), led by a single
        // border glyph so it reads as part of the top edge; only the top/bottom
        // edges are drawn, since side │bars would eat two columns for nothing on
        // a narrow terminal (e.g. a phone over ssh).
        let list = List::new(items)
            .block(
                Block::default()
                    .title(format!("─[{prompt}]"))
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
            KeyCode::Enter => {
                return Some(ElementResult::Value(self.options[self.selected].clone()));
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::Choice;
    use crate::ui::{Element, ElementResult};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn up_down_moves_highlight_and_wraps_around() {
        let mut choice = Choice::new(None, vec!["a".into(), "b".into(), "c".into()]);
        assert_eq!(choice.selected, 0, "starts on the first option");

        choice.on_key(key(KeyCode::Down)); // 0 -> 1
        assert_eq!(choice.selected, 1);
        choice.on_key(key(KeyCode::Down)); // 1 -> 2
        assert_eq!(choice.selected, 2);
        choice.on_key(key(KeyCode::Down)); // wraps 2 -> 0
        assert_eq!(choice.selected, 0);
        choice.on_key(key(KeyCode::Up)); // wraps 0 -> 2
        assert_eq!(choice.selected, 2);
    }

    #[test]
    fn enter_returns_the_selected_option() {
        let mut choice = Choice::new(
            Some("pick one".into()),
            vec!["bench".into(), "squat".into()],
        );
        choice.on_key(key(KeyCode::Down));
        assert_eq!(
            choice.on_key(key(KeyCode::Enter)),
            Some(ElementResult::Value("squat".to_string()))
        );
    }

    #[test]
    fn other_keys_do_nothing() {
        let mut choice = Choice::new(None, vec!["only".into()]);
        assert_eq!(choice.on_key(key(KeyCode::Char('x'))), None);
        assert_eq!(choice.selected, 0);
    }

    #[test]
    fn renders_numbered_options_with_the_selection_highlighted() {
        let mut choice = Choice::new(
            Some("pick".into()),
            vec!["bench press".into(), "squat".into()],
        );
        let area = Rect::new(0, 0, 30, 6);
        let mut buf = Buffer::empty(area);
        choice.render(area, &mut buf);

        // Bracketed header on the top border row, led by one border glyph;
        // no side bars anywhere.
        assert!(RowText::at(&buf, 0).starts_with("─[pick]"));
        assert!(RowText::at(&buf, 1).contains("1. bench press"));
        assert!(RowText::at(&buf, 2).contains("2. squat"));
        assert!(
            RowText::at(&buf, 5).chars().all(|c| c == '─'),
            "bottom edge is a plain rule, no corners"
        );

        // The first option (selected) is reversed; the second is not.
        assert!(
            buf[(0, 1)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "the selected row must be highlighted"
        );
        assert!(
            !buf[(0, 2)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "unselected rows must not be highlighted"
        );

        // Moving down moves the highlight with it. In the real shell the
        // buffer is reset between frames (`Terminal::draw`), so mimic that
        // here — without it the old REVERSED modifier would linger.
        choice.on_key(key(KeyCode::Down));
        buf.reset();
        choice.render(area, &mut buf);
        assert!(
            !buf[(0, 1)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert!(
            buf[(0, 2)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED)
        );
    }

    // Helper joining a row's cells into a string, mirroring buffer output.
    struct RowText;
    impl RowText {
        fn at(buf: &Buffer, y: u16) -> String {
            (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
        }
    }
}
