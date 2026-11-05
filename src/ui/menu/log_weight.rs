//! The log-weight screen: record a body-weight measurement.
//!
//! The date is asked first, prefilled with today's local calendar date —
//! Enter accepts it, and a different date is typed in place, clearing the
//! default first (a historical day: the weigh-in is logged against that date,
//! the way a backdated macro log is). A date in the future isn't silently
//! accepted — the clock may be wrong — so it's met with a No-defaulted "log
//! anyway?" confirm, exactly like the create-log screen. Then the weight in
//! kilograms is asked. If the chosen date already holds a weigh-in, a
//! No-defaulted "log anyway?" confirm pops up before the new one is recorded
//! — a day may hold several, so an already-logged date is the user's call,
//! surfaced as a warning rather than recorded silently — and an explicit
//! "Yes" writes it as another entry. Escape at either field, or declining a
//! confirm, records nothing, and the screen returns to the weight menu
//! ([`super::weight_menu`]).

use chrono::{Local, Utc};
use rusqlite::Connection;

use crate::domain::WeightMeasurement;

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::Menu;
use super::add_recipe_item::parse_quantity;
use super::create_log::ask_date;
use super::select_session::format_date;

/// Ask for the date, ask for the weight, and record the weigh-in — warning
/// when the date already has one. Always returns to the weight menu (nothing
/// is recorded on Escape or a declined confirm).
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    let Some(date) = ask_date(shell)? else {
        return Ok(Some(Menu::Weight)); // Escape: nothing recorded
    };
    let Some(weight_kg) = ask_weight(shell, "")? else {
        return Ok(Some(Menu::Weight)); // Escape: nothing recorded
    };
    // A date can hold several weigh-ins, so a duplicate is the user's call —
    // surfaced as a No-defaulted warning rather than silently recorded (or
    // silently refused). "No" (or Escape) records nothing.
    if !WeightMeasurement::on_date(conn, date)?.is_empty()
        && !shell.confirm(Some(format!(
            "A weight log already exists for {}. Log anyway?",
            format_date(date)
        )))?
    {
        return Ok(Some(Menu::Weight));
    }
    let logged = WeightMeasurement::create(conn, weight_kg, date, Utc::now())?;
    shell.log(format!(
        "Logged {} kg on {} at {}.",
        logged.weight_kg,
        format_date(logged.date),
        logged.created_at.with_timezone(&Local).format("%H:%M")
    ));
    Ok(Some(Menu::Weight))
}

/// Ask for the weight in kilograms: a positive, finite number, re-asking with
/// what was typed when it won't parse. `initial` prefills the field (blank on
/// the log-weight screen; the current weight on the entry options' "modify
/// weight"). `None` when the user cancels with Escape.
pub(super) fn ask_weight(shell: &mut Shell, initial: &str) -> Result<Option<f64>, UiError> {
    let mut pending = initial.to_string();
    loop {
        let outcome = shell.read_line(
            Some("Weight (kg)".to_string()),
            Some("e.g. 82.5".to_string()),
            &pending,
        )?;
        match outcome {
            ReadOutcome::Value(raw) => match parse_quantity(&raw) {
                Some(weight) => return Ok(Some(weight)),
                None => {
                    shell.log("Enter a number greater than 0");
                    pending = raw;
                }
            },
            ReadOutcome::Cancelled { .. } => return Ok(None),
        }
    }
}
