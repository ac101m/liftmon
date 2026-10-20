//! The main menu screen: the app's top level.
//!
//! Its options are "Exercises" and "Sessions" (descend into the respective
//! screens) and "Exit" (leave the menus entirely).

use crate::ui::menu::Menu;
use crate::ui::utils::{Shell, UiError};

/// Ask the main menu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(
        Some("Main menu".to_string()),
        ["Exercises", "Sessions", "Exit"],
    )?;
    Ok(match choice.as_str() {
        "Exercises" => Some(Menu::Exercises),
        "Sessions" => Some(Menu::Sessions),
        _ => None, // "Exit"
    })
}
