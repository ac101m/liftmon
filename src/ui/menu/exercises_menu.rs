//! The exercise screen: browse the exercises (via the shared "Select exercise"
//! submenu), add a new one, or back out to the main menu. "Browse exercises"
//! is the most common action, so it sits first.

use rusqlite::Connection;

use crate::ui::utils::{Shell, UiError};

use super::Menu;
use super::select_exercise;

/// Ask the exercise menu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(
        Some("Exercises".to_string()),
        ["Browse exercises", "Add exercise", "Back"],
    )?;
    Ok(match choice.as_deref() {
        // "Browse exercises" runs the shared chooser submenu inline; backing
        // out of it (Escape, or ultimately the submenu's Escape) returns here
        // without picking, like the session flow's is.
        Some("Browse exercises") => match select_exercise::pick(shell, conn)? {
            Some(exercise) => Some(Menu::ExerciseOptions {
                name: exercise.name,
            }),
            None => Some(Menu::Exercises), // backed out of the chooser
        },
        Some("Add exercise") => Some(Menu::AddExercise),
        _ => Some(Menu::Main), // "Back", or Escape
    })
}
