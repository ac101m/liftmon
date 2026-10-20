//! The exercise screen: reach the exercise actions, or back out to the main
//! menu.

use crate::ui::error::UiError;
use crate::ui::menu::Menu;
use crate::ui::shell::Shell;

/// Ask the exercise menu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(Some("Exercises".to_string()), ["Add exercise", "Back"])?;
    Ok(match choice.as_str() {
        "Add exercise" => Some(Menu::AddExercise),
        _ => Some(Menu::Main), // "Back"
    })
}
