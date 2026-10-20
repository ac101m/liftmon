//! Menu state machine.
//!
//! [`Menu`] records where we currently stand in the app's screen tree, and
//! [`Menu::next`] asks the current screen's question and returns the next
//! state — or [`None`] when the user leaves the menus entirely ("Exit" or
//! Ctrl+C).
//!
//! Each screen's behaviour lives in its own file — see [`super::main_menu`],
//! [`super::exercises_menu`], [`super::add_exercise`] and
//! [`super::select_exercise`] — so this file stays a thin dispatcher as more
//! menus are added.

use rusqlite::Connection;

use super::error::UiError;
use super::shell::Shell;

/// The current screen in the app's menu hierarchy.
pub(crate) enum Menu {
    /// The top-level menu (Exercises / Exit).
    Main,
    /// The exercise screen (Add exercise / Show exercise / Back).
    Exercises,
    /// The add-exercise form (name, then description).
    AddExercise,
    /// The pick-an-exercise screen (filter box + list).
    SelectExercise,
}

impl Menu {
    /// Ask the user where to go from here and return the next screen.
    ///
    /// A database connection is passed along so screens can persist (the
    /// add-exercise form writes a new row via [`crate::domain::Exercise`]).
    /// Returns [`None`] when the user leaves the menus entirely — choosing
    /// "Exit", or interrupting with Ctrl+C.
    pub(crate) fn next(
        self,
        shell: &mut Shell,
        conn: &Connection,
    ) -> Result<Option<Menu>, UiError> {
        match self {
            Menu::Main => super::main_menu::run(shell),
            Menu::Exercises => super::exercises_menu::run(shell),
            Menu::AddExercise => super::add_exercise::run(shell, conn),
            Menu::SelectExercise => super::select_exercise::run(shell, conn),
        }
    }
}
