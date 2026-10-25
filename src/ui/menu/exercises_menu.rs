//! The exercise screen: show all the exercises, show the exercises that
//! activate a picked muscle group, add a new one, or back out to the main menu.
//! "Show all exercises" is the most common action, so it sits first.

use crate::ui::menu::Menu;
use crate::ui::utils::{Shell, UiError};

/// Ask the exercise menu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(
        Some("Exercises".to_string()),
        [
            "Show all exercises",
            "Show exercises by muscle group",
            "Add exercise",
            "Back",
        ],
    )?;
    Ok(match choice.as_deref() {
        Some("Show all exercises") => Some(Menu::SelectExercise),
        Some("Show exercises by muscle group") => Some(Menu::SelectExerciseByGroup),
        Some("Add exercise") => Some(Menu::AddExercise),
        _ => Some(Menu::Main), // "Back", or Escape
    })
}
