//! The show-weight-logs screen: browse every weigh-in in the filter picker
//! (typing narrows the list), then into the picked entry's options screen.
//!
//! Each entry reads `<date> <HH:MM> · <weight> kg` — the day it records (the
//! app's `YYYY/MM/DD` convention, per [`super::select_session::format_date`]),
//! the local time it was entered, and the weight — newest first, so the most
//! recent weigh-in reads at the top (a backdated entry still sorts under its
//! own day, since [`crate::domain::WeightMeasurement::all`] orders by recorded
//! date). Rows need not be unique: a day can hold several weigh-ins, so rows
//! that read identically (same date, time and weight) are interchangeable, and
//! picking takes the first matching row by its own id. Escape — or an empty
//! history, which says so — backs out to the weight menu
//! ([`super::weight_menu`]).

use chrono::Local;
use rusqlite::Connection;

use crate::domain::WeightMeasurement;

use crate::ui::utils::{Shell, UiError};

use super::Menu;
use super::item_options::round2;
use super::select_session::format_date;

/// Ask the user to filter and pick one weigh-in, landing on its options
/// screen. Escape backs out to the weight menu.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    // `WeightMeasurement::all` returns oldest first — the reverse of the order
    // the picker wants, so the most recent weigh-in reads at the top.
    let measurements = WeightMeasurement::all(conn)?;
    if measurements.is_empty() {
        shell.log("No weigh-ins yet — log one first");
        return Ok(Some(Menu::Weight));
    }
    let rows: Vec<(String, i64)> = measurements
        .iter()
        .rev()
        .map(|measurement| (label(measurement), measurement.id))
        .collect();
    let labels: Vec<String> = rows.iter().map(|(label, _)| label.clone()).collect();
    let Some(chosen) = shell.pick(
        Some("Select weigh-in".to_string()),
        Some("type to filter".to_string()),
        labels,
    )?
    else {
        return Ok(Some(Menu::Weight)); // Escape: backed out
    };
    // First matching row, by label. Identical labels (same date, time and
    // weight) are interchangeable in a day's log, so which of the duplicates
    // is picked cannot be told apart in what remains.
    let Some((_, id)) = rows.into_iter().find(|(label, _)| label == &chosen) else {
        shell.log("That weight log is no longer there");
        return Ok(Some(Menu::Weight));
    };
    Ok(Some(Menu::WeightLogOptions { id }))
}

/// The row a weigh-in reads as: `<date> <HH:MM> · <weight> kg` — the app's
/// date convention, the local time it was entered, and the weight rounded to
/// the hundredth. Shared with the entry options screen so its title (and the
/// entry's identity on the screen) reads the same there.
pub(super) fn label(measurement: &WeightMeasurement) -> String {
    let local = measurement.created_at.with_timezone(&Local);
    format!(
        "{} {} · {} kg",
        format_date(measurement.date),
        local.format("%H:%M"),
        round2(measurement.weight_kg)
    )
}
