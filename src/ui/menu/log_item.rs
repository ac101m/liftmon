//! The log-item screen: record one use of an item against a calendar day.
//!
//! The date is asked first, defaulting to today's local calendar date — Enter
//! accepts it, typing a different date backdates the entry (the day's log row
//! exists independently of when the entry was actually recorded). The item
//! comes second, through the same "Select item" filter picker the other item
//! screens use, and then the **quantity factor** — how many of the item this
//! row records, defaulting to one and typing anything scales it. A use creates
//! a [`MacroLogInclusion`] at that quantity for the day, and the day's
//! [`MacroLog`] row itself only if it doesn't exist yet — so a date's first
//! entry makes the row, later entries reuse it.
//!
//! Escape at the date field, the picker or the quantity field cancels the whole
//! thing: nothing has been recorded yet, so there is no in-progress state to
//! confirm abandoning. After a successful log the screen returns to the macros
//! menu. The pick-then-quantity-then-record body is [`record_use`], shared with
//! the log options screen's "add item" (which already knows its date).

use chrono::{Local, NaiveDate, Utc};
use rusqlite::Connection;

use crate::domain::{Item, MacroLog, MacroLogInclusion};

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::Menu;
use super::add_recipe_item::parse_quantity;
use super::select_session::format_date;

/// Ask for the date, then record the day's use via [`record_use`]. Always
/// returns to the macros menu (nothing is recorded on Escape).
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    let Some(date) = ask_date(shell)? else {
        return Ok(Some(Menu::Macros)); // Escape: nothing recorded yet
    };
    record_use(shell, conn, date)?;
    Ok(Some(Menu::Macros))
}

/// Pick an item, ask its quantity factor, and record the use against `date` —
/// finding or creating the day's log. The shared body of the "log item" screen
/// (which asked the date above) and the log options screen's "add item" (whose
/// date is already set). Whether or not anything got recorded, Escape anywhere
/// backs out of the picker or the quantity field silently.
pub(super) fn record_use(
    shell: &mut Shell,
    conn: &Connection,
    date: NaiveDate,
) -> Result<(), UiError> {
    // The same filter picker show_items uses; "log item" needs the item's row
    // for the inclusion's item_id, so resolve the picked name back from the
    // list just fetched (names are UNIQUE, so it identifies exactly one).
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

/// Ask for the log's date: the first ask presents today as a replaceable
/// default (Enter accepts it, typing a different `YYYY/MM/DD` backdates), and
/// a date that won't parse re-asks with what was typed, so a single digit can
/// be fixed. `None` when the user cancels with Escape.
fn ask_date(shell: &mut Shell) -> Result<Option<NaiveDate>, UiError> {
    // Today in the app's `YYYY/MM/DD` date convention (the session screens'
    // format_date), the value the field presents as its replaceable default.
    let today = format_date(Local::now().date_naive());
    // `pending` seeds the field after a refused date; `None` means the first
    // ask, which presents today's date as the replaceable default.
    let mut pending: Option<String> = None;
    loop {
        let outcome = match &pending {
            None => {
                shell.read_line_default(Some("Log date (YYYY/MM/DD)".to_string()), None, &today)?
            }
            Some(raw) => shell.read_line(Some("Log date (YYYY/MM/DD)".to_string()), None, raw)?,
        };
        match outcome {
            ReadOutcome::Value(raw) => match parse_date(&raw) {
                Some(date) => return Ok(Some(date)),
                None => {
                    shell.log("Enter a date as YYYY/MM/DD.");
                    pending = Some(raw);
                }
            },
            ReadOutcome::Cancelled { .. } => return Ok(None),
        }
    }
}

/// Ask how many of the picked item this row records: the first ask presents 1
/// as a replaceable default (Enter logs a single item, typing anything scales),
/// and a quantity that won't parse re-asks with what was typed, so a "2." or
/// "0" left on the default can be fixed the same way the date field's re-ask
/// works. `None` when the user cancels with Escape — nothing is recorded.
fn ask_quantity(shell: &mut Shell, item_name: &str) -> Result<Option<f64>, UiError> {
    // `pending` seeds the field after a refused quantity; `None` means the
    // first ask, which presents 1 as the replaceable default.
    let mut pending: Option<String> = None;
    loop {
        let outcome = match &pending {
            None => shell.read_line_default(Some(format!("{item_name} · Quantity")), None, "1")?,
            Some(raw) => shell.read_line(Some(format!("{item_name} · Quantity")), None, raw)?,
        };
        match outcome {
            ReadOutcome::Value(raw) => match parse_quantity(&raw) {
                Some(quantity) => return Ok(Some(quantity)),
                None => {
                    shell.log("Enter a number greater than 0");
                    pending = Some(raw);
                }
            },
            ReadOutcome::Cancelled { .. } => return Ok(None),
        }
    }
}

/// Read a calendar date — as `YYYY/MM/DD` (the app's date convention, and
/// what the field prompts for) or the hyphenated `YYYY-MM-DD` the
/// `macro_logs` table stores its `date` column in. Whitespace is tolerated
/// around the input, and the month/day may skip zero-padding (`2026/1/1`
/// still reads) — the value round-trips to the canonical `YYYY-MM-DD` text on
/// store, so leniency costs nothing. Anything not a date (blank, wrong
/// separators, a day-first or month-first order, an out-of-range day) is
/// rejected.
pub(super) fn parse_date(raw: &str) -> Option<NaiveDate> {
    let raw = raw.trim();
    NaiveDate::parse_from_str(raw, "%Y/%m/%d")
        .or_else(|_| NaiveDate::parse_from_str(raw, "%Y-%m-%d"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_date_reads_an_app_convention_calendar_date() {
        // The app's date convention is YYYY/MM/DD (the session screens'
        // format_date), which is what the field prompts for — so it reads.
        assert_eq!(
            parse_date("2026/10/01"),
            NaiveDate::from_ymd_opt(2026, 10, 1)
        );
        // Surrounding whitespace is tolerated, and so is missing zero-padding
        // on the month/day — the stored text is canonical either way.
        assert_eq!(
            parse_date(" 2026/10/01 "),
            NaiveDate::from_ymd_opt(2026, 10, 1)
        );
        assert_eq!(parse_date("2026/1/1"), NaiveDate::from_ymd_opt(2026, 1, 1));
        // The hyphenated YYYY-MM-DD the column stores still reads, so typing
        // it (a backdated entry) is never refused.
        assert_eq!(
            parse_date("2026-10-01"),
            NaiveDate::from_ymd_opt(2026, 10, 1)
        );
    }

    #[test]
    fn parse_date_rejects_everything_else() {
        // Blank, words, wrong separators, a day-first or month-first order,
        // and out-of-range days all fail.
        for raw in ["", "   ", "abc", "01/10/2026", "10/31/2026", "2026/10/32"] {
            assert_eq!(
                parse_date(raw),
                None,
                "raw {raw:?} is not a YYYY/MM/DD date"
            );
        }
    }
}
