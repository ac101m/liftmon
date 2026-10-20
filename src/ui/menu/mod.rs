//! The app's screens and the [`Menu`] state machine that walks between them.
//!
//! [`Menu`] records where we currently stand in the app's screen tree, and
//! [`Menu::next`] asks the current screen's question and returns the next
//! state — or [`None`] when the user leaves the menus entirely ("Exit" or
//! Ctrl+C).
//!
//! Each screen's behaviour lives in its own file — see [`self::main_menu`],
//! [`self::exercises_menu`], [`self::add_exercise`] and
//! [`self::select_exercise`] — so this file stays a thin dispatcher as more
//! menus are added. The screens are built on the reusable element framework in
//! [`super::utils`] (the shell, the input elements, errors).

use rusqlite::Connection;

use crate::ui::utils::{Shell, UiError};

mod add_exercise;
mod exercises_menu;
mod main_menu;
mod select_exercise;

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
            Menu::Main => main_menu::run(shell),
            Menu::Exercises => exercises_menu::run(shell),
            Menu::AddExercise => add_exercise::run(shell, conn),
            Menu::SelectExercise => select_exercise::run(shell, conn),
        }
    }
}
