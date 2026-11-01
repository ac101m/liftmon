//! The log submenu of the macros screen: record one use of an item against a
//! day (the log-item flow) or browse the stored day-logs (the history date
//! picker), or back out to the macros menu. The opposite of the items submenu
//! ([`super::macros_items`]).

use crate::ui::menu::Menu;
use crate::ui::utils::{Shell, UiError};

/// Ask the log-submenu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(Some("Log".to_string()), ["log item", "history", "back"])?;
    Ok(match choice.as_deref() {
        Some("log item") => Some(Menu::LogItem),
        Some("history") => Some(Menu::ShowLogs),
        _ => Some(Menu::Macros), // "back", or Escape
    })
}
