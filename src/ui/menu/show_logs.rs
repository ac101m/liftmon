//! The show-logs screen: browse every day's macro log in the filter picker
//! (typing narrows the list), then into the picked log's options screen.
//!
//! Each entry is named by its date, in the app's `YYYY/MM/DD` convention,
//! newest date first — the same most-recent-first reading the session list
//! gives its sessions, since a log is usually wanted for a day the user just
//! added to. The dates are UNIQUE (one log per day), so picking one names the
//! log exactly. Escape backs out to the macros menu.

use rusqlite::Connection;

use crate::domain::MacroLog;

use crate::ui::utils::{Shell, UiError};

use super::Menu;
use super::select_session::format_date;

/// Ask the user to filter and pick one day's log, landing on its options
/// screen. Escape backs out to the macros menu.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    // `MacroLog::all` returns oldest first — the reverse of the order the
    // picker wants, so the most recent log reads at the top.
    let logs = MacroLog::all(conn)?;
    if logs.is_empty() {
        shell.log("No logs yet — log an item first");
        return Ok(Some(Menu::Macros));
    }
    let labels: Vec<String> = logs.iter().rev().map(|log| format_date(log.date)).collect();
    let Some(chosen) = shell.pick(
        Some("Select date".to_string()),
        Some("type to filter".to_string()),
        labels,
    )?
    else {
        return Ok(Some(Menu::Macros)); // Escape: backed out
    };

    // Each date renders to one distinct label, so the picked label names
    // exactly one log (dates are UNIQUE).
    let Some(log) = logs.into_iter().find(|log| format_date(log.date) == chosen) else {
        shell.log("That log is no longer there");
        return Ok(Some(Menu::Macros));
    };
    Ok(Some(Menu::LogOptions { date: log.date }))
}
