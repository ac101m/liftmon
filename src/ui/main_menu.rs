//! The main menu screen: the app's top level.
//!
//! Its options are "Exercises" (descend into the exercise screen) and "Exit"
//! (leave the menus entirely).

use crate::ui::error::UiError;
use crate::ui::menu::Menu;
use crate::ui::shell::Shell;

/// Ask the main menu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(Some("Main menu".to_string()), ["Exercises", "Exit"])?;
    Ok(match choice.as_str() {
        "Exercises" => Some(Menu::Exercises),
        _ => None, // "Exit"
    })
}
