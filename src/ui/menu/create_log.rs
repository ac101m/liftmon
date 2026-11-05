//! The create-log screen: make (or open) a day's macro log by date, landing on
//! that log's options menu.
//!
//! The date is asked first, prefilled with today's local calendar date — Enter
//! accepts it, and a different date is typed in place, clearing the default
//! first (a historical day: the log row is created for that date, independent
//! of today). A date in the future isn't silently accepted — the clock may be
//! wrong — so it's met with a No-defaulted "log anyway?" confirm: the safe
//! "No" (or Escape) keeps the typed text for a fix, and an explicit "Yes"
//! still creates. The day's log is created if it doesn't exist yet, or merely
//! opened if it already does (a date can hold only one log) — the console
//! message tells which happened — and the screen hands the user onto that
//! log's options screen ([`super::log_options`]), where "add item" extends the
//! date. This is the historical counterpart of [`super::log_item`], which
//! records a use against today without asking the date.

use chrono::{Local, NaiveDate};
use rusqlite::Connection;

use crate::domain::MacroLog;

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::Menu;
use super::select_session::format_date;

/// Ask the date, create or open that day's log, and land on its options
/// screen. Returns to the log submenu when the date is cancelled (Escape:
/// nothing created).
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    let Some(date) = ask_date(shell)? else {
        return Ok(Some(Menu::MacrosLogs)); // Escape: nothing created
    };
    // A date can hold only one log, so opening it is find-or-create; the
    // message tells the user which happened — a fresh row was made, or the
    // existing one was opened.
    let existing = MacroLog::find_by_date(conn, date)?.is_some();
    let log = MacroLog::find_or_create(conn, date)?;
    shell.log(if existing {
        format!("Opening existing log for {}.", format_date(date))
    } else {
        format!("Created a macro log for {}.", format_date(date))
    });
    Ok(Some(Menu::LogOptions { date: log.date }))
}

/// Ask for the log's date: the field is prefilled with today in the app's
/// `YYYY/MM/DD` convention — Enter accepts it, and a different date is typed in
/// place, clearing the default first (backdating the entry). A date that won't
/// parse re-asks with what was typed, so a single digit can be fixed, and a
/// date in the future is confirmed rather than trusted — the clock may be
/// wrong. `None` when the user cancels with Escape. Shared with the log-weight
/// screen, which asks its day's date the same way.
pub(super) fn ask_date(shell: &mut Shell) -> Result<Option<NaiveDate>, UiError> {
    let mut pending = format_date(Local::now().date_naive());
    loop {
        let outcome = shell.read_line(Some("Log date (YYYY/MM/DD)".to_string()), None, &pending)?;
        match outcome {
            ReadOutcome::Value(raw) => match parse_date(&raw) {
                // A date ahead of today is suspicious — the system clock may be
                // off — but it's the user's call, so ask behind a No-defaulted
                // "log anyway?": the safe "No" rests under Enter, and an
                // explicit "Yes" still creates (none of the user's data was in
                // the future; the clock just said so). No or Escape keeps the
                // typed text and re-asks, so a nearby future date is one edit
                // away.
                Some(date) if date > Local::now().date_naive() => {
                    if shell.confirm(Some(format!(
                        "{} is a future date — log anyway?",
                        format_date(date)
                    )))? {
                        return Ok(Some(date));
                    }
                    pending = raw;
                }
                Some(date) => return Ok(Some(date)),
                None => {
                    shell.log("Enter a date as YYYY/MM/DD.");
                    pending = raw;
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
fn parse_date(raw: &str) -> Option<NaiveDate> {
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
