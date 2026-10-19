//! The exercise screen: add exercises, or back out to the main menu.

use crate::ui::error::UiError;
use crate::ui::menu::Menu;
use crate::ui::shell::Shell;

/// Ask the exercise menu question and return where the user's choice leads.
///
/// "Add exercise" is a placeholder for now: it logs a TODO and stays on this
/// screen, so the log line sits in the history area above the menu.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(Some("Exercises".to_string()), ["Add exercise", "Back"])?;
    match choice.as_str() {
        "Add exercise" => {
            shell.log("add exercise: TODO");
            // A real "add" will become its own screen; for now stay put so
            // the TODO stays visible in the history area above the menu.
            Ok(Some(Menu::Exercises))
        }
        _ => Ok(Some(Menu::Main)), // "Back"
    }
}
