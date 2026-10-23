//! The exercise screen: show the existing exercises (the most common action,
//! so it sits first in the menu), add a new one, or back out to the main menu.

use crate::ui::menu::Menu;
use crate::ui::utils::{Shell, UiError};

/// Ask the exercise menu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(
        Some("Exercises".to_string()),
        ["Show exercises", "Add exercise", "Back"],
    )?;
    Ok(match choice.as_deref() {
        Some("Show exercises") => Some(Menu::SelectExercise),
        Some("Add exercise") => Some(Menu::AddExercise),
        _ => Some(Menu::Main), // "Back", or Escape
    })
}
