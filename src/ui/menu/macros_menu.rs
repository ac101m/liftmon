//! The macros screen: the slow-growing home of the macronutrient tracker.
//!
//! It offers browsing the stored items (the filter picker), adding an item,
//! or backing out (up to the main menu); the meal-building screens are still
//! to come.

use crate::ui::menu::Menu;
use crate::ui::utils::{Shell, UiError};

/// Ask the macros menu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(
        Some("Macros".to_string()),
        ["show items", "add item", "back"],
    )?;
    Ok(match choice.as_deref() {
        Some("show items") => Some(Menu::ShowItems),
        Some("add item") => Some(Menu::AddItem),
        _ => Some(Menu::Main), // "back", or Escape
    })
}
