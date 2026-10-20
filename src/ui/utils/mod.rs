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
//!   filterable [`picker`], and the wrapping [`text_input`].
//! * [`error`]: the errors the UI layer can produce.

mod choice;
mod error;
mod picker;
mod shell;
mod text_input;

pub use choice::get_user_selection;
pub(crate) use error::UiError;
pub(crate) use shell::{ReadOutcome, Shell};

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

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
