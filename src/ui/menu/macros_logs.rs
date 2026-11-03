//! The log submenu of the macros screen: record one use of an item against
//! today (the log-item flow, no date ask), record a cheat against today (the
//! log-cheat flow — a note describing the cheat, and the day marked as one),
//! create or open a day's log by date
//! (the create-log flow, landing on that log's options), browse the stored
//! day-logs (the history date picker), or back out to the macros menu. The
//! opposite of the items submenu ([`super::macros_items`]).

use crate::ui::menu::Menu;
use crate::ui::utils::{Shell, UiError};

/// Ask the log-submenu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(
        Some("Macro logs".to_string()),
        ["log item", "log cheat", "create log", "history", "back"],
    )?;
    Ok(match choice.as_deref() {
        Some("log item") => Some(Menu::LogItem),
        // "log cheat" sits under "log item": both record something against
        // today rather than opening a date's log first.
        Some("log cheat") => Some(Menu::LogCheat),
        Some("create log") => Some(Menu::CreateLog),
        Some("history") => Some(Menu::ShowLogs),
        _ => Some(Menu::Macros), // "back", or Escape
    })
}
