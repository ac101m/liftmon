//! The main menu screen: the app's top level.
//!
//! Its options are "Exercises", "Sessions", "Macros" and "Muscle groups"
//! (descend into the respective screens) and "Exit" (leave the menus
//! entirely). There is no "back" from the top level, so Escape just re-asks
//! the same question.

use crate::ui::menu::Menu;
use crate::ui::utils::{Shell, UiError};

/// Ask the main menu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(
        Some("Main menu".to_string()),
        ["Exercises", "Sessions", "Macros", "Muscle groups", "Exit"],
    )?;
    Ok(match choice.as_deref() {
        Some("Exercises") => Some(Menu::Exercises),
        Some("Sessions") => Some(Menu::Sessions),
        Some("Macros") => Some(Menu::Macros),
        Some("Muscle groups") => Some(Menu::SelectMuscleGroup),
        Some("Exit") => None,
        _ => Some(Menu::Main), // Escape: nothing to back out of here
    })
}
