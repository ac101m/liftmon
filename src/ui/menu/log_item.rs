//! The log-item screen: record one use of an item against *today* — the
//! on-the-spot log.
//!
//! No date is asked: the use lands on today's local calendar date, creating
//! the day's log row if it doesn't exist yet. (A historical day's log is made
//! first via [`super::create_log`], which asks for the date and lands on that
//! log's options.) The item comes through the same "Select item" filter picker
//! the other item screens use, and then the **quantity factor** — how many of
//! the item this row records, prefilled with one so Enter logs a single use
//! and a different factor is typed in place. A use creates a
//! [`MacroLogInclusion`] at that quantity for today, and today's [`MacroLog`]
//! row itself only if it doesn't exist yet — so a day's first entry makes the
//! row, later entries reuse it.
//!
//! Escape at the picker or the quantity field cancels the whole thing:
//! nothing has been recorded yet, so there is no in-progress state to confirm
//! abandoning. After a successful log the screen returns to the log submenu
//! ([`super::macros_logs`]). The pick-then-quantity-then-record body is
//! [`record_use`], shared with the log options screen's "add item" (which
//! already knows its date).

use chrono::{Local, NaiveDate, Utc};
use rusqlite::Connection;

use crate::domain::{Item, MacroLog, MacroLogInclusion};

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::Menu;
use super::add_recipe_item::parse_quantity;
use super::select_session::format_date;

/// Record today's use via [`record_use`]. Always returns to the log submenu
/// (nothing is recorded on Escape).
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    let today = Local::now().date_naive();
    record_use(shell, conn, today)?;
    Ok(Some(Menu::MacrosLogs))
}

/// Pick an item, ask its quantity factor, and record the use against `date` —
/// finding or creating the day's log. The shared body of the "log item" screen
/// (which logs today) and the log options screen's "add item" (whose date is
/// already set). Whether or not anything got recorded, Escape anywhere backs
/// out of the picker or the quantity field silently.
pub(super) fn record_use(
    shell: &mut Shell,
    conn: &Connection,
    date: NaiveDate,
) -> Result<(), UiError> {
    // The same filter picker show_items uses; the log needs the item's row for
    // the inclusion's item_id, so resolve the picked name back from the list
    // just fetched (names are UNIQUE, so it identifies exactly one).
    let items = Item::all(conn)?;
    if items.is_empty() {
        shell.log("No items yet — add one first");
        return Ok(());
    }
    let names: Vec<String> = items.iter().map(|item| item.name.clone()).collect();
    let Some(chosen) = shell.pick(
        Some("Select item".to_string()),
        Some("type to filter".to_string()),
        names,
    )?
    else {
        return Ok(()); // Escape: backed out
    };
    let Some(item) = items.into_iter().find(|item| item.name == chosen) else {
        shell.log("That item is no longer there");
        return Ok(());
    };

    let Some(quantity) = ask_quantity(shell, &item.name)? else {
        return Ok(()); // Escape: cancelled the quantity
    };

    // Find-or-create the day's log, then record this use at this moment.
    let log = MacroLog::find_or_create(conn, date)?;
    MacroLogInclusion::add(conn, log.id, item.id, quantity, Utc::now())?;
    shell.log(format!(
        "Logged {} × '{}' on {}.",
        quantity,
        item.name,
        format_date(date)
    ));
    Ok(())
}

/// Ask how many of the picked item this row records: the field is prefilled
/// with 1 — Enter logs a single item, and a different factor is typed in
/// place, clearing the default first. A quantity that won't parse re-asks with
/// what was typed, so a "2." or "0" left on the prefill can be fixed the same
/// way. `None` when the user cancels with Escape — nothing is recorded.
fn ask_quantity(shell: &mut Shell, item_name: &str) -> Result<Option<f64>, UiError> {
    let mut pending = "1".to_string();
    loop {
        let outcome = shell.read_line(Some(format!("{item_name} · Quantity")), None, &pending)?;
        match outcome {
            ReadOutcome::Value(raw) => match parse_quantity(&raw) {
                Some(quantity) => return Ok(Some(quantity)),
                None => {
                    shell.log("Enter a number greater than 0");
                    pending = raw;
                }
            },
            ReadOutcome::Cancelled { .. } => return Ok(None),
        }
    }
}
