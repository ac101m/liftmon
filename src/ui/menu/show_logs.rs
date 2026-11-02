//! The show-logs screen: browse every day's macro log in the filter picker
//! (typing narrows the list), then into the picked log's options screen.
//!
//! Each entry is named by its date, in the app's `YYYY/MM/DD` convention,
//! newest date first — the same most-recent-first reading the session list
//! gives its sessions, since a log is usually wanted for a day the user just
//! added to — with the day's resolved calories and protein riding along as a
//! suffix (`2026/10/02 ·  320kcal  60g protein`), so a day's totals are
//! legible before opening it. The calories are right-aligned to a fixed
//! field, so the protein quantity always starts at the same column.
//! A day with nothing logged reads `· nothing
//! recorded` instead — there are no totals to show. The dates are UNIQUE (one
//! log per day), so
//! picking one names the log exactly. Escape backs out to the log submenu
//! ([`super::macros_logs`]).

use std::collections::HashMap;

use rusqlite::Connection;

use crate::domain::{MacroLog, MacroLogInclusion};

use crate::ui::utils::{Shell, UiError};

use super::Menu;
use super::item_options::round2;
use super::select_session::format_date;

/// Ask the user to filter and pick one day's log, landing on its options
/// screen. Escape backs out to the log submenu.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    // `MacroLog::all` returns oldest first — the reverse of the order the
    // picker wants, so the most recent log reads at the top.
    let logs = MacroLog::all(conn)?;
    if logs.is_empty() {
        shell.log("No logs yet — log an item first");
        return Ok(Some(Menu::MacrosLogs));
    }
    // Each label is a resolved-macros pass — the same per-day resolution the
    // log options info screen does for the picked log — so a day's totals
    // match the totals its info shows, rounded to the hundredth. One `cache`
    // spans the whole list: every day's items resolve at most once, so a long
    // history that repeats the same handful of items costs a couple of hash
    // lookups per extra day, not another DAG walk.
    let mut cache = HashMap::new();
    let mut labels = Vec::with_capacity(logs.len());
    for log in logs.iter().rev() {
        let date = format_date(log.date);
        // A day with nothing logged — its row made by "create log", or emptied
        // by removals — has no totals to carry, so its row reads "nothing
        // recorded" instead of "0kcal · 0g protein", and resolves nothing.
        let label = if MacroLogInclusion::for_log(conn, log.id)?.is_empty() {
            format!("{date} · nothing recorded")
        } else {
            let macros = MacroLog::resolved_macros_cached(conn, log.id, &mut cache)?;
            // The calories are right-aligned within a fixed field so every
            // row's protein quantity starts at the same column — ` 540kcal`
            // lines up under `  60kcal`, the numbers' right edges flush —
            // with two spaces separating the pair. A day whose totals outgrow
            // the 8-wide field (a 4-digit count already fills it) just shifts
            // that one row.
            format!(
                "{date} · {:>8}  {}g protein",
                format!("{}kcal", round2(macros.calories)),
                round2(macros.protein),
            )
        };
        labels.push(label);
    }
    let Some(chosen) = shell.pick(
        Some("Select date".to_string()),
        Some("type to filter".to_string()),
        labels,
    )?
    else {
        return Ok(Some(Menu::MacrosLogs)); // Escape: backed out
    };

    // Each label begins with its fixed-width (10-char) date, and dates are
    // UNIQUE, so the picked label's date prefix names exactly one log.
    let Some(log) = logs
        .into_iter()
        .find(|log| chosen.starts_with(&format_date(log.date)))
    else {
        shell.log("That log is no longer there");
        return Ok(Some(Menu::MacrosLogs));
    };
    Ok(Some(Menu::LogOptions { date: log.date }))
}
