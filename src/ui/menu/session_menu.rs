//! The session screen: start a new workout — creating its session row and
//! dropping the user onto that session's options screen — list the existing
//! sessions (each opening its own options screen), or back out to the main
//! menu.

use chrono::Utc;
use rusqlite::Connection;

use crate::domain::Session;
use crate::ui::menu::Menu;
use crate::ui::utils::{Shell, UiError};

/// Ask the session menu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(
        Some("Sessions".to_string()),
        ["new session", "list sessions", "back"],
    )?;
    Ok(match choice.as_deref() {
        // A new session's row is created up front — `created_at` stamps the
        // moment "new session" is chosen — and the user lands on its options
        // screen, the same screen every session lives on. Entering the
        // freeform flow from there always happens via "resume", so there is no
        // dropped-into-a-failed-picker surprise when there are no exercises.
        Some("new session") => {
            let session = Session::create(conn, Utc::now())?;
            shell.log("New session started");
            Some(Menu::SessionOptions {
                session_id: session.id,
            })
        }
        Some("list sessions") => Some(Menu::SelectSession),
        _ => Some(Menu::Main), // "back", or Escape
    })
}
