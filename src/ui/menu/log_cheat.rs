//! The log-cheat screen: record a cheat against today — a note describing
//! what got cheated, and today's macro log marked as a cheat day.
//!
//! The prompt asks for the cheat's text (there is no picking: a cheat's own
//! words are all there is to say), which **must be supplied** — a blank answer
//! re-asks with what was typed kept, and Escape cancels without writing
//! anything. The text becomes a note on today's log (via
//! [`MacroLogNote::create`], so it shows under the log info's "Notes:" and in
//! "show notes" like any other), and the day's `is_cheat` flag is set in the
//! same breath — the log info then reads `[CHEAT]` in red and the history
//! picker shows `CHEAT` for the day. The counterpart of [`super::log_item`]
//! (which records food against today); both return to the logs submenu.

use chrono::{Local, Utc};
use rusqlite::Connection;

use crate::domain::{MacroLog, MacroLogNote};

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::Menu;

/// Ask for the cheat's text and record it against today's log (creating the
/// log if today has none yet — the log-item flow's find-or-create), marking
/// the day as a cheat. Always returns to the logs submenu; Escape anywhere
/// cancels without recording a cheat.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    let today = Local::now().date_naive();
    let mut prefill = String::new();
    let text = loop {
        match shell.read_line(Some("Describe the cheat".to_string()), None, &prefill)? {
            ReadOutcome::Value(text) => {
                let text = text.trim().to_owned();
                if text.is_empty() {
                    shell.log("The cheat needs a description");
                    prefill.clear();
                    continue;
                }
                break text;
            }
            ReadOutcome::Cancelled { .. } => return Ok(Some(Menu::MacrosLogs)),
        }
    };
    let log = MacroLog::find_or_create(conn, today)?;
    MacroLogNote::create(conn, log.id, text, Utc::now())?;
    log.set_is_cheat(conn, true)?;
    shell.log("Logged cheat");
    Ok(Some(Menu::MacrosLogs))
}
