//! The session screen: begin a freeform workout session, list the existing
//! sessions (each opening its own options screen), or back out to the main
//! menu.

use crate::ui::menu::{FreeformStart, Menu};
use crate::ui::utils::{Shell, UiError};

/// Ask the session menu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(
        Some("Sessions".to_string()),
        ["new session", "list sessions", "back"],
    )?;
    Ok(match choice.as_deref() {
        Some("new session") => Some(Menu::FreeformSession(FreeformStart::New)),
        Some("list sessions") => Some(Menu::SelectSession),
        _ => Some(Menu::Main), // "back", or Escape
    })
}
