//! The interactive terminal UI.
//!
//! The shell surfaces a classic CLI layout: a **history area** on top showing
//! the log of past actions, and a **prompt bar** on the bottom showing the
//! active element's key hints. Between them lives the active **element**
//! (currently the multiple-choice list, later text input, etc.).
//!
//! * The [`shell::Shell`] owns the terminal session — raw mode, the alternate
//!   screen, restoring them on exit/panic — and the single event/render loop
//!   shared by all elements. Ctrl+C is the shell's business and quits the app.
//! * An [`Element`] owns its own state, its look, and how keys move it around;
//!   it reports a value when the user is done with it.
//! * Every frame redraws from [`ratatui::Frame::area`], so a terminal resize is
//!   picked up on the next frame with no special handling. Rendering clips to a
//!   fixed-size buffer instead of wrapping, so lines never wrap by accident.
//! * The [`menu::Menu`] state machine tracks which screen we're on and what the
//!   user's choice leads to; the main loop steps it forward over one shared
//!   shell session until the user leaves.
//!
//! The shell needs a real terminal (raw mode); running it with a pipe as stdin
//! or no controlling terminal fails cleanly via [`error::UiError`].
//!
//! For one-off questions, call [`get_user_selection`]. For a sequence of
//! questions that should share one session — so the history area accumulates
//! the log across them, as the app's main menu does — hold a [`Shell`] and use
//! [`shell::Shell::ask`].

mod choice;
mod error;
mod exercises_menu;
mod main_menu;
mod menu;
mod shell;

pub use choice::get_user_selection;
pub(crate) use error::UiError;
pub(crate) use menu::Menu;
pub(crate) use shell::Shell;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// A piece of the interactive UI driven by the [`shell::Shell`].
///
/// The shell owns the screen and the loop; an element owns everything about
/// itself: how tall it would like to be, what hint it shows in the prompt bar,
/// how it draws, and how keys move it around. Returning `Some(value)` from
/// [`Element::on_key`] finishes the element with that value.
pub(crate) trait Element {
    /// Preferred height in rows (before the shell caps it to fit the screen).
    fn natural_height(&self) -> u16;

    /// One-line key hint shown in the prompt bar at the bottom.
    fn hint(&self) -> String;

    /// Draw the element into the given area. Called every frame.
    fn render(&mut self, area: Rect, buf: &mut Buffer);

    /// Handle one key press. `Some(value)` means the user is done.
    fn on_key(&mut self, key: KeyEvent) -> Option<String>;
}
