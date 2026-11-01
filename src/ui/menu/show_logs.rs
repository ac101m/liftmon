//! The show-logs screen: browse every day's macro log in the filter picker
//! (typing narrows the list), then into the picked log's options screen.
//!
//! Each entry is named by its date, in the app's `YYYY/MM/DD` convention,
//! newest date first — the same most-recent-first reading the session list
//! gives its sessions, since a log is usually wanted for a day the user just
//! added to — with the day's resolved calories and protein riding along as a
//! suffix (`2026/10/02 · 320kcal · 60g protein`), so a day's totals are
//! legible before opening it. The dates are UNIQUE (one log per day), so
//! picking one names the log exactly. Escape backs out to the macros menu.

use rusqlite::Connection;

use crate::domain::MacroLog;

use crate::ui::utils::{Shell, UiError};

use super::Menu;
use super::item_options::round2;
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
    // Each label is one resolved-macros pass — the same per-log resolution the
    // log options info screen does for the picked log — so a day's totals
    // match the totals its info shows, rounded to the hundredth.
    let mut labels = Vec::with_capacity(logs.len());
    for log in logs.iter().rev() {
        let macros = MacroLog::resolved_macros(conn, log.id)?;
        labels.push(format!(
            "{} · {}kcal · {}g protein",
            format_date(log.date),
            round2(macros.calories),
            round2(macros.protein),
        ));
    }
    let Some(chosen) = shell.pick(
        Some("Select date".to_string()),
        Some("type to filter".to_string()),
        labels,
    )?
    else {
        return Ok(Some(Menu::Macros)); // Escape: backed out
    };

    // Each label begins with its fixed-width (10-char) date, and dates are
    // UNIQUE, so the picked label's date prefix names exactly one log.
    let Some(log) = logs
        .into_iter()
        .find(|log| chosen.starts_with(&format_date(log.date)))
    else {
        shell.log("That log is no longer there");
        return Ok(Some(Menu::Macros));
    };
    Ok(Some(Menu::LogOptions { date: log.date }))
}
