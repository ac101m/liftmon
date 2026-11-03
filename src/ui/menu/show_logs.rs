//! The show-logs screen: browse every day's macro log in the filter picker
//! (typing narrows the list), then into the picked log's options screen.
//!
//! Each entry is named by its date, in the app's `YYYY/MM/DD` convention,
//! newest date first — the same most-recent-first reading the session list
//! gives its sessions, since a log is usually wanted for a day the user just
//! added to — with the day's resolved calories and protein riding along as a
//! suffix (`2026/10/02 · 320kcal | 60g protein`), so a day's totals are
//! legible before opening it. The calories are right-aligned to the widest
//! calories label among the shown options, so every protein quantity starts
//! at the same column, the pipe separating the pair.
//! A day with nothing logged reads `· nothing
//! recorded` instead — there are no totals to show — and a day flagged as a
//! cheat reads `· CHEAT`, the marker in red standing in for the totals (the
//! point of a cheat day's row is the flag, not its macros).
//! The dates are UNIQUE (one
//! log per day), so
//! picking one names the log exactly. Escape backs out to the log submenu
//! ([`super::macros_logs`]).

use std::collections::HashMap;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
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
    // Two passes. First resolve every date's totals and remember the widest
    // calories label, so the second pass can right-align all of them to that
    // width — the "longest calories text of any option + 2 spaces" column,
    // where every row's protein quantity starts. Any day can widen the
    // column, whether its total is 4 digits or fractional; the whole column
    // flexes to fit rather than letting one row's protein spill sideways.
    // One `cache` spans the bunch: each day's items resolve at most once.
    let mut rows = Vec::with_capacity(logs.len());
    let mut calorie_width = 0usize;
    for log in logs.iter().rev() {
        let date = format_date(log.date);
        if log.is_cheat {
            // A cheat day's row carries only its red marker — the totals are
            // not the point of the day, and marking the cheat out of the
            // column (no kcal) keeps it from widening the alignment.
            rows.push((date, true, None));
        } else if MacroLogInclusion::for_log(conn, log.id)?.is_empty() {
            rows.push((date, false, None));
        } else {
            let macros = MacroLog::resolved_macros_cached(conn, log.id, &mut cache)?;
            let kcal = format!("{}kcal", round2(macros.calories));
            calorie_width = calorie_width.max(kcal.len());
            rows.push((date, false, Some((kcal, round2(macros.protein)))));
        }
    }
    let mut labels = Vec::with_capacity(rows.len());
    for (date, is_cheat, totals) in rows {
        // A flagged cheat day reads `· CHEAT` in red, standing in for whatever
        // the totals or the "nothing recorded" placeholder would read.
        if is_cheat {
            labels.push(Line::from(vec![
                Span::raw(format!("{date} · ")),
                Span::styled("CHEAT", Style::new().fg(Color::Red)),
            ]));
            continue;
        }
        // A day with nothing logged — its row made by "create log", or emptied
        // by removals — has no totals to carry, so its row reads "nothing
        // recorded" instead of "0kcal | 0g protein", and resolves nothing.
        // Only the trailing "nothing recorded" is dimmed (an attribute, not a
        // hard-coded colour, so it stays legible under any terminal theme) —
        // the date and separator keep their full strength, so an empty day
        // still reads normally and just its placeholder recedes.
        let label = match totals {
            None => Line::from(vec![
                Span::raw(format!("{date} · ")),
                Span::styled("nothing recorded", Style::new().add_modifier(Modifier::DIM)),
            ]),
            // The calories are right-aligned within the widest calories label
            // — a shorter ` 540kcal` lines up under a `1234kcal` with its
            // right edge on the same column — then a ` | ` separates the
            // pair, so every protein quantity starts on the same column.
            Some((kcal, protein)) => Line::from(format!(
                "{date} · {kcal:>calorie_width$} | {protein}g protein",
                kcal = kcal,
                protein = protein,
                calorie_width = calorie_width,
            )),
        };
        labels.push(label);
    }
    let Some(chosen) = shell.pick_styled(
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
