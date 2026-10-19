//! The shell: owns a terminal session and runs interactive elements.
//!
//! A [`Shell`] puts the terminal into raw mode + the alternate screen for its
//! lifetime (restoring both in `Drop`, so even a panic-UX near-accident or
//! `process::exit` path that happens to drop us first leaves the terminal
//! usable), keeps the run's log (`history`), and shares one event/render loop
//! across elements. Every frame the screen is laid out as three horizontal
//! bands:
//!
//! ```text
//! (history)     the log of past actions, newest at the bottom
//! (element)     the active element, e.g. the multiple-choice list
//! (prompt bar)  1 row at the very bottom with the element's key hints
//! ```

use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{List, Paragraph, Widget};
use ratatui::{DefaultTerminal, Frame};

use super::Element;
use super::choice::Choice;
use super::error::UiError;

/// How often the shell redraws even with no input, so a terminal resize shows
/// up quickly.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A running terminal session.
pub(crate) struct Shell {
    terminal: DefaultTerminal,
    history: Vec<String>,
}

impl Shell {
    /// Enter raw mode + the alternate screen. Restores the terminal again if
    /// setup fails part-way.
    pub(crate) fn new() -> Result<Self, UiError> {
        let terminal = match ratatui::try_init() {
            Ok(terminal) => terminal,
            Err(err) => {
                // `try_init` may have left raw mode on before failing; put the
                // terminal back before reporting the error.
                let _ = ratatui::try_restore();
                return Err(UiError::Io(err));
            }
        };
        Ok(Self {
            terminal,
            history: Vec::new(),
        })
    }

    /// Append a line to the history area.
    pub(crate) fn log(&mut self, line: impl Into<String>) {
        self.history.push(line.into());
    }

    /// Ask a multiple-choice question within this session.
    ///
    /// Runs a [`Choice`] element and returns the user's selection. Unlike
    /// [`super::choice::get_user_selection`], this keeps the same session (and
    /// history) across calls — the natural way to drive a sequence of
    /// questions, as the main menu does.
    pub(crate) fn ask<S: AsRef<str>>(
        &mut self,
        prompt: Option<String>,
        options: impl IntoIterator<Item = S>,
    ) -> Result<String, UiError> {
        let options: Vec<String> = options.into_iter().map(|s| s.as_ref().to_owned()).collect();
        assert!(
            !options.is_empty(),
            "cannot ask a question with no options to choose from"
        );
        self.run(&mut Choice::new(prompt, options))
    }

    /// Run one [`Element`] to completion, returning the value it produced.
    ///
    /// The element is redrawn every frame, with the history above it and the
    /// prompt bar below. Keys are the element's business; Ctrl+C is the
    /// shell's, and quits as [`UiError::Interrupted`].
    pub(crate) fn run(&mut self, element: &mut dyn Element) -> Result<String, UiError> {
        loop {
            self.terminal.try_draw(|frame| {
                render_frame(frame, &self.history, element);
                Ok::<(), std::io::Error>(())
            })?;

            if !event::poll(POLL_INTERVAL)? {
                // Nothing to read: redraw on the next loop and pick up resizes.
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue; // ignore paste/mouse/resize; a redraw dealt with it.
            };
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Err(UiError::Interrupted);
            }
            if let Some(value) = element.on_key(key) {
                return Ok(value);
            }
        }
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        // Bring the cursor back, then leave raw mode + the alternate screen.
        self.terminal.show_cursor().ok();
        ratatui::restore();
    }
}

/// Renders one frame: history on top, the element below it, prompt bar last.
fn render_frame(frame: &mut Frame<'_>, history: &[String], element: &mut dyn Element) {
    let [history_area, element_area, bar_area] = layout_areas(frame.area(), element);
    let buf = frame.buffer_mut();

    render_history(buf, history_area, history);
    element.render(element_area, buf);
    render_prompt_bar(buf, bar_area, element.hint());
}

/// Three horizontal bands: history, the element, and the 1-row prompt bar.
///
/// The element takes `min(its natural height, half the available rows)` so a
/// long option list scrolls inside the element instead of pushing the history
/// off the screen.
fn layout_areas(area: Rect, element: &dyn Element) -> [Rect; 3] {
    let available = area.height.saturating_sub(1); // reserve the prompt bar
    let element_height = element.natural_height().min(available / 2).min(available);

    let chunks = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(element_height),
        Constraint::Length(1),
    ])
    .split(area);

    [chunks[0], chunks[1], chunks[2]]
}

/// The log of past actions, newest at the bottom (clipped, never wrapped).
fn render_history(buf: &mut Buffer, area: Rect, history: &[String]) {
    if area.height == 0 || area.width == 0 || history.is_empty() {
        return;
    }
    // Show the tail that fits; the "scrollback" is everything above.
    let start = history.len().saturating_sub(area.height as usize);
    let lines: Vec<Line> = history[start..]
        .iter()
        .map(|line| Line::from(line.clone()).style(Style::new().add_modifier(Modifier::DIM)))
        .collect();
    List::new(lines).render(area, buf);
}

/// The one-row status bar with the element's key hints, in reverse video.
fn render_prompt_bar(buf: &mut Buffer, area: Rect, hint: String) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    buf.set_style(area, Style::new().add_modifier(Modifier::REVERSED));
    Paragraph::new(hint).render(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Element;

    struct DummyElement(u16);
    impl Element for DummyElement {
        fn natural_height(&self) -> u16 {
            self.0
        }
        fn hint(&self) -> String {
            "hints here".into()
        }
        fn render(&mut self, _area: Rect, _buf: &mut Buffer) {}
        fn on_key(&mut self, _key: crossterm::event::KeyEvent) -> Option<String> {
            None
        }
    }

    #[test]
    fn layout_puts_bar_at_bottom_and_element_above_history() {
        // A tall enough screen: the element hugs its natural height.
        let [history, element, bar] = layout_areas(Rect::new(0, 0, 80, 24), &DummyElement(5));
        assert_eq!(bar.y + bar.height, 24, "prompt bar touches the bottom edge");
        assert_eq!(bar.height, 1);
        assert_eq!(
            element.height, 5,
            "short option list keeps its natural height"
        );
        assert_eq!(
            history.height,
            24 - 5 - 1,
            "history takes the remaining rows"
        );
        assert_eq!(history.y, 0, "history is on top");
        assert_eq!(
            element.y, history.height,
            "element sits right under history"
        );
    }

    #[test]
    fn layout_caps_element_when_options_do_not_fit() {
        // A huge list on a short screen is capped to half the available rows.
        let [_, element, bar] = layout_areas(Rect::new(0, 0, 80, 20), &DummyElement(10_000));
        assert_eq!(bar.y + bar.height, 20);
        assert_eq!(element.height, (20 - 1) / 2);
        assert_eq!(element.y, 20 - element.height - 1);
    }

    #[test]
    fn history_shows_the_newest_lines_at_the_bottom() {
        let history: Vec<String> = ["first", "second", "third"].map(String::from).to_vec();
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 2));
        render_history(&mut buf, Rect::new(0, 0, 40, 2), &history);

        let row = |y: u16| (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(0).trim_end(), "second");
        assert_eq!(row(1).trim_end(), "third");
    }
}
