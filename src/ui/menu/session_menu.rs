//! The session screen: begin a freeform workout session, resume a previous
//! one, or back out to the main menu.

use crate::ui::menu::{FreeformStart, Menu};
use crate::ui::utils::{Shell, UiError};

/// Ask the session menu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(
        Some("Sessions".to_string()),
        ["new session", "resume session", "back"],
    )?;
    Ok(match choice.as_deref() {
        Some("new session") => Some(Menu::FreeformSession(FreeformStart::New)),
        Some("resume session") => Some(Menu::ResumeSession),
        _ => Some(Menu::Main), // "back", or Escape
    })
}
