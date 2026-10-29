//! The reusable element framework the screens are built on.
//!
//! Everything in here is self-contained: it knows nothing about the app's
//! screens or the database, only about driving a terminal session. The
//! [`crate::ui::menu`] screens sit on top of it.
//!
//! * The [`Element`] trait — a piece of the interactive UI that owns its own
//!   state, its look, and how keys move it around, reporting a result when the
//!   user is done with it (see [`ElementResult`]).
//! * The [`shell`] session — owns a terminal (raw mode + the alternate screen,
//!   restored on drop) and the single event/render loop shared by all
//!   elements. Every frame it lays the screen out in three bands: the run's
//!   log of past actions on top, the active element below it, and a one-row
//!   prompt bar of key hints at the bottom.
//! * The elements themselves: the multiple-choice [`choice`] list, the
//!   filterable [`picker`], the wrapping [`text_input`], and the cycling
//!   [`cycle_picker`] that sets a level on each row.
//! * [`error`]: the errors the UI layer can produce.

mod choice;
mod cycle_picker;
mod error;
mod picker;
mod shell;
mod text_input;
mod wrap;

pub use choice::get_user_selection;
pub(crate) use cycle_picker::Level;
pub(crate) use error::UiError;
pub(crate) use shell::{CycleOutcome, ReadOutcome, Shell};
pub(crate) use wrap::wrap_line;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// A piece of the interactive UI driven by the [`shell`] session.
///
/// The shell owns the screen and the loop; an element owns everything about
/// itself: how tall it would like to be, what hint it shows in the prompt bar,
/// how it draws, and how keys move it around. Returning
/// [`Some(ElementResult)`] from [`Element::on_key`] finishes the element with
/// that result.
pub(crate) trait Element {
    /// Preferred height in rows, for the given column width, before the shell
    /// caps it to fit the screen. Text that wraps (e.g. input) grows with its
    /// width.
    fn natural_height(&self, width: u16) -> u16;

    /// One-line key hint shown in the prompt bar at the bottom.
    fn hint(&self) -> String;

    /// Draw the element into the given area. Called every frame.
    fn render(&mut self, area: Rect, buf: &mut Buffer);

    /// Handle one key press. `Some(result)` means the user is done with the
    /// element.
    fn on_key(&mut self, key: KeyEvent) -> Option<ElementResult>;

    /// Where, in element coordinates, the shell should park the real terminal
    /// cursor — used by text input so the caret follows the user's typing.
    /// `None` (the default) leaves the cursor alone.
    fn cursor(&self) -> Option<(u16, u16)> {
        None
    }
}

/// How an element finishes: with the value the user produced, or with Escape
/// when the user backed out before committing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ElementResult {
    Value(String),
    Cancelled,
}

/// Build the input row shared by the input elements ([`self::picker`]'s
/// filter and [`self::text_input`]'s entry): a two-column "> " gutter marking
/// the row as a prompt, followed by the typed content, or the element's
/// dimmed suggestion when the field is empty (an element without a suggestion
/// then shows just the gutter). The suggestion starts two columns past where
/// typed text would — the caret parks at the first text column (2) when the
/// field is empty, so the hint is drawn clear of it; typed text still begins
/// right after the gutter. Pass `content` as [`None`] to show the empty-field
/// row.
pub(crate) fn prompt_row(content: Option<String>, prompt: Option<&str>) -> Line<'static> {
    match content {
        Some(text) => Line::from(vec![Span::raw("> "), Span::raw(text)]),
        None => match prompt {
            // Two blank columns keep the dimmed hint out from under the block
            // caret, which parks at column 2 in an empty field.
            Some(prompt) => Line::from(vec![
                Span::raw("> "),
                Span::raw("  "),
                Span::styled(prompt.to_owned(), Style::new().add_modifier(Modifier::DIM)),
            ]),
            None => Line::from(vec![Span::raw("> ")]),
        },
    }
}
