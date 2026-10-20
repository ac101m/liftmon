//! The exercise screen: add an exercise, list the existing ones, or back out
//! to the main menu.

use crate::ui::menu::Menu;
use crate::ui::utils::{Shell, UiError};

/// Ask the exercise menu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(
        Some("Exercises".to_string()),
        ["Add exercise", "List exercises", "Back"],
    )?;
    Ok(match choice.as_str() {
        "Add exercise" => Some(Menu::AddExercise),
        "List exercises" => Some(Menu::SelectExercise),
        _ => Some(Menu::Main), // "Back"
    })
}
