//! The macros screen: the slow-growing home of the macronutrient tracker.
//!
//! It now branches into two submenus rather than listing every flow flat: the
//! items submenu ([`super::macros_items`] — browse the stored items or create
//! a new one) and the log submenu ([`super::macros_logs`] — log a use against
//! a day or browse the day-logs), plus backing out to the main menu. Each
//! submenu's flows are unchanged; this screen just hands the user into one.

use crate::ui::menu::Menu;
use crate::ui::utils::{Shell, UiError};

/// Ask the macros menu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(Some("Macros".to_string()), ["items", "logs", "back"])?;
    Ok(match choice.as_deref() {
        Some("items") => Some(Menu::MacrosItems),
        Some("logs") => Some(Menu::MacrosLogs),
        _ => Some(Menu::Main), // "back", or Escape
    })
}
