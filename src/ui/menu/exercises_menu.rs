//! The exercise screen: reach the exercise actions, or back out to the main
//! menu.

use crate::ui::menu::Menu;
use crate::ui::utils::{Shell, UiError};

/// Ask the exercise menu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(
        Some("Exercises".to_string()),
        ["Add exercise", "Show exercise", "Back"],
    )?;
    Ok(match choice.as_str() {
        "Add exercise" => Some(Menu::AddExercise),
        "Show exercise" => Some(Menu::SelectExercise),
        _ => Some(Menu::Main), // "Back"
    })
}
