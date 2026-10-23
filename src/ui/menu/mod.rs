//! The app's screens and the [`Menu`] state machine that walks between them.
//!
//! [`Menu`] records where we currently stand in the app's screen tree, and
//! [`Menu::next`] asks the current screen's question and returns the next
//! state — or [`None`] when the user leaves the menus entirely ("Exit" or
//! Ctrl+C).
//!
//! Each screen's behaviour lives in its own file — see [`self::main_menu`],
//! [`self::exercises_menu`], [`self::session_menu`], [`self::freeform_session`],
//! [`self::select_session`], [`self::session_options`], [`self::add_exercise`],
//! [`self::select_exercise`] and [`self::exercise_options`] — so this file
//! stays a thin dispatcher as more menus are added. The screens are built on
//! the reusable element framework in [`super::utils`] (the shell, the input
//! elements, errors).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use rusqlite::Connection;

use crate::domain::{Difficulty, Exercise};
use crate::ui::utils::{Shell, UiError};

mod add_exercise;
mod exercise_options;
mod exercises_menu;
mod freeform_session;
mod main_menu;
mod select_exercise;
mod select_session;
mod session_menu;
mod session_options;

/// The current screen in the app's menu hierarchy.
pub(crate) enum Menu {
    /// The top-level menu (Exercises / Sessions / Exit).
    Main,
    /// The exercise screen (Add exercise / List exercises / Back).
    Exercises,
    /// The add-exercise form (name, then description).
    AddExercise,
    /// The pick-an-exercise screen (filter box + list).
    SelectExercise,
    /// The actions for one picked exercise, named by its unique name so the
    /// prompt updates after a rename.
    ExerciseOptions { name: String },
    /// The session screen (New session / List sessions / Back).
    Sessions,
    /// The pick-a-session screen: list sessions, then into that session's
    /// options screen.
    SelectSession,
    /// The actions for one picked session, identified by its database row id.
    /// A brand-new session lands here too: the session screen creates the row
    /// and hands the user straight onto its options screen.
    SessionOptions { session_id: i64 },
    /// The freeform session flow: pick exercises and log sets until the user
    /// leaves, then land back on that session's options screen.
    FreeformSession { session_id: i64 },
}

/// The lines of an exercise's info section: the bold exercise name (with a
/// trailing colon, like any section heading) over its indented description.
pub(super) fn exercise_lines(exercise: &Exercise) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![Span::styled(
        format!("{}:", exercise.name),
        Style::new().add_modifier(Modifier::BOLD),
    )])];
    if !exercise.description.is_empty() {
        lines.push(Line::from(format!("  {}", exercise.description)));
    }
    lines
}

/// Print an exercise to the history area as a framed section: divider, name,
/// indented description, divider.
pub(super) fn log_exercise(shell: &mut Shell, exercise: &Exercise) {
    shell.log_section(exercise_lines(exercise));
}

/// The foreground colour a set's difficulty marker reads as: green for easy,
/// yellow for hard, red for a failed set. Placed on the difficulty span so it
/// survives the history's line-level dimming (span styles ride on top of it) —
/// used by the session summary and the freeform session's set-completion lines.
pub(super) fn difficulty_style(difficulty: Difficulty) -> Style {
    let colour = match difficulty {
        Difficulty::Easy => Color::Green,
        Difficulty::Hard => Color::Yellow,
        Difficulty::Failure => Color::Red,
    };
    Style::new().fg(colour)
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
            Menu::ExerciseOptions { name } => exercise_options::run(shell, conn, name),
            Menu::Sessions => session_menu::run(shell, conn),
            Menu::SelectSession => select_session::run(shell, conn),
            Menu::SessionOptions { session_id } => session_options::run(shell, conn, session_id),
            Menu::FreeformSession { session_id } => freeform_session::run(shell, conn, session_id),
        }
    }
}
