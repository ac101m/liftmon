//! The interactive terminal UI, split into two layers:
//!
//! * [`utils`] — the reusable element framework: the shell session, the input
//!   elements (choice list, filterable picker, text input), and the UI errors.
//!   Self-contained; it knows nothing about the app's screens.
//! * [`menu`] — the app's screens (main menu, exercises menu, the add form,
//!   the picker, the per-exercise actions) and the [`Menu`] state machine that
//!   walks between them over one shared shell session.
//!
//! This module re-exports the public surface the rest of the app uses:
//! [`get_user_selection`] for one-off questions, and [`Shell`], [`Menu`] and
//! [`UiError`] for the main loop in `main.rs`.
//!
//! The shell needs a real terminal (raw mode); running it with a pipe as stdin
//! or no controlling terminal fails cleanly via [`UiError`].

mod menu;
mod utils;

pub(crate) use menu::Menu;
pub(crate) use utils::Shell;
pub(crate) use utils::UiError;
pub use utils::get_user_selection;
