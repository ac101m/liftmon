//! The items submenu of the macros screen: browse the stored items (the show
//! items filter picker) or create a new one (plain or recipe), or back out to
//! the macros menu. The opposite of the log submenu ([`super::macros_logs`]),
//! which holds the day-log flows.

use crate::ui::menu::Menu;
use crate::ui::utils::{Shell, UiError};

/// Ask the items-submenu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(
        Some("Macro items".to_string()),
        ["show items", "new item", "back"],
    )?;
    Ok(match choice.as_deref() {
        Some("show items") => Some(Menu::ShowItems),
        Some("new item") => Some(Menu::AddItem),
        _ => Some(Menu::Macros), // "back", or Escape
    })
}
