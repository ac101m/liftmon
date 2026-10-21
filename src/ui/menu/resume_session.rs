//! The resume-session screen: pick a session to carry on with.
//!
//! Sessions have no "ended" state — every session is a continuing workout that
//! can be picked back up at any time. A session whose most recent exercise was
//! more than 6 hours ago is probably not the workout the user is actually
//! mid-flow in, so picking one asks for a Yes/No confirmation first.
//!
//! The list is a plain choice list (no filter, unlike the exercise picker),
//! ordered most recent first, each entry named by the date and when its most
//! recent exercise happened, e.g. `"2026/10/20 · (3 hours ago)"`. It is built
//! from the lightweight [`SessionInfo`] rows rather than full sessions — no
//! `sets` join, one row per session — so listing a long history stays cheap.
//! Selecting one hands the session to the freeform flow, which resumes at the
//! most recent exercise.

use chrono::{Local, NaiveDate, NaiveDateTime, TimeDelta};
use rusqlite::Connection;

use crate::domain::{Session, SessionInfo};
use crate::ui::utils::{Shell, UiError};

use super::{FreeformStart, Menu};

/// A session whose last exercise is older than this needs a Yes/No
/// confirmation before it is resumed.
const CONFIRM_AFTER: TimeDelta = TimeDelta::hours(6);

/// Ask which session to resume, handing the freeform flow the chosen one — or
/// the session screen when there is nothing to resume or the user backs out
/// with Escape.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    // `Session::list_info` orders newest first too, so the list comes back
    // most recent first.
    let sessions = Session::list_info(conn)?;
    if sessions.is_empty() {
        shell.log("No sessions to resume yet");
        return Ok(Some(Menu::Sessions));
    }
    // One "now" for the whole screen visit, so every label reads the same and
    // the confirm check keys off the same moment.
    let now = Local::now().naive_local();

    loop {
        let Some(chosen) = shell.ask(
            Some("Resume session".to_string()),
            sessions
                .iter()
                .map(|s| session_label(s, now))
                .collect::<Vec<_>>(),
        )?
        else {
            return Ok(Some(Menu::Sessions)); // Escape: backed out
        };
        let session = sessions
            .iter()
            .find(|s| session_label(s, now) == chosen)
            .expect("picked a listed session");

        // A session idle for over 6 hours probably belongs to an earlier
        // workout — check before resuming it. A session resumed within
        // `CONFIRM_AFTER` flows straight on.
        if needs_confirmation(session, now) {
            shell.log("That session's last exercise was over 6 hours ago.");
            let sure = shell.ask(
                Some(format!(
                    "Resume session from {}?",
                    format_date(session.start_date)
                )),
                ["Yes", "No"],
            )?;
            if sure.as_deref() != Some("Yes") {
                continue; // re-ask the list
            }
        }

        // A resume just continues the session — nothing needs to be reset, and
        // the freeform flow loads the full session itself once it starts.
        return Ok(Some(Menu::FreeformSession(FreeformStart::Resume {
            session_id: session.id,
        })));
    }
}

/// The choice-list label for a session: the day its most recent exercise
/// happened and how long ago, e.g. `"2026/10/20 · (3 hours ago)"`.
fn session_label(info: &SessionInfo, now: NaiveDateTime) -> String {
    match info.last_exercise_at {
        Some(at) => format!("{} · ({})", format_date(at.date()), time_ago(at, now)),
        // A session with nothing logged yet has no moment to name; its start
        // date stands in.
        None => format_date(info.start_date),
    }
}

/// A date the way the session list shows one, e.g. `2026/10/20`.
fn format_date(date: NaiveDate) -> String {
    date.format("%Y/%m/%d").to_string()
}

/// How long ago `last` was, relative to `now`, in human terms: `"just now"`,
/// `"2 minutes ago"`, `"7 hours ago"`, `"15 days ago"`, `"4 months ago"`,
/// `"2 years ago"`. The unit grows coarser the further back it goes; anything
/// under a minute is just "just now".
fn time_ago(last: NaiveDateTime, now: NaiveDateTime) -> String {
    let since = now.signed_duration_since(last);
    if since < TimeDelta::minutes(1) {
        return "just now".to_string();
    }
    let minutes = since.num_minutes();
    if minutes < 60 {
        return format!("{} {} ago", minutes, plural("minute", minutes));
    }
    let hours = since.num_hours();
    if hours < 24 {
        return format!("{} {} ago", hours, plural("hour", hours));
    }
    let days = since.num_days();
    if days < 30 {
        return format!("{} {} ago", days, plural("day", days));
    }
    let months = days / 30;
    if months < 12 {
        return format!("{} {} ago", months, plural("month", months));
    }
    // Guard the year boundary, so a sub-365-day gap never reads "0 years ago".
    let years = (days / 365).max(1);
    format!("{} {} ago", years, plural("year", years))
}

/// `word` when `n` is 1, `word`s otherwise.
fn plural(word: &str, n: i64) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

/// Whether resuming this session at `now` warrants a confirmation: its most
/// recent exercise was logged more than [`CONFIRM_AFTER`] before `now`. A
/// session with nothing logged yet has no last exercise, so it never needs
/// one.
fn needs_confirmation(info: &SessionInfo, now: NaiveDateTime) -> bool {
    info.last_exercise_at
        .is_some_and(|last| now.signed_duration_since(last) > CONFIRM_AFTER)
}

#[cfg(test)]
mod tests {
    use super::{CONFIRM_AFTER, needs_confirmation, session_label, time_ago};
    use crate::domain::SessionInfo;
    use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeDelta};

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn date_time(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        date(y, m, d).and_time(NaiveTime::from_hms_opt(h, min, 0).unwrap())
    }

    /// A session on 2026-10-20 whose most recent exercise was at `last_at`.
    fn info(last_at: Option<NaiveDateTime>) -> SessionInfo {
        SessionInfo {
            id: 1,
            start_date: date(2026, 10, 20),
            last_exercise_at: last_at,
        }
    }

    #[test]
    fn label_shows_the_date_and_how_long_ago() {
        let now = date_time(2026, 10, 21, 15, 0);
        let hours_ago = now - TimeDelta::hours(11); // same day
        assert_eq!(
            session_label(&info(Some(hours_ago)), now),
            "2026/10/21 · (11 hours ago)"
        );
        // The date is the last exercise's day, not today's.
        let days_ago = now - TimeDelta::days(15);
        assert_eq!(
            session_label(&info(Some(days_ago)), now),
            "2026/10/06 · (15 days ago)"
        );
        // Nothing logged yet: just the start date, same format.
        assert_eq!(session_label(&info(None), now), "2026/10/20");
    }

    #[test]
    fn time_ago_speaks_in_human_units() {
        let now = date_time(2026, 10, 21, 15, 0);

        assert_eq!(time_ago(now - TimeDelta::seconds(30), now), "just now");
        assert_eq!(time_ago(now - TimeDelta::minutes(1), now), "1 minute ago");
        assert_eq!(time_ago(now - TimeDelta::minutes(5), now), "5 minutes ago");
        assert_eq!(time_ago(now - TimeDelta::hours(1), now), "1 hour ago");
        assert_eq!(time_ago(now - TimeDelta::hours(5), now), "5 hours ago");
        assert_eq!(time_ago(now - TimeDelta::days(1), now), "1 day ago");
        assert_eq!(time_ago(now - TimeDelta::days(15), now), "15 days ago");
        assert_eq!(time_ago(now - TimeDelta::days(45), now), "1 month ago");
        assert_eq!(time_ago(now - TimeDelta::days(300), now), "10 months ago");
        assert_eq!(time_ago(now - TimeDelta::days(400), now), "1 year ago");
        assert_eq!(time_ago(now - TimeDelta::days(800), now), "2 years ago");
    }

    #[test]
    fn confirmation_needed_when_past_six_hours() {
        let now = date_time(2026, 10, 21, 15, 0);
        let recent = date_time(2026, 10, 21, 9, 30);
        let stale = date_time(2026, 10, 20, 12, 0);

        assert!(!needs_confirmation(&info(Some(recent)), now));
        assert!(needs_confirmation(&info(Some(stale)), now));

        // Exactly six hours is not "over" six hours.
        let borderline = now - CONFIRM_AFTER;
        assert!(!needs_confirmation(&info(Some(borderline)), now));
    }

    #[test]
    fn session_with_no_last_exercise_never_needs_confirmation() {
        let now = date_time(2026, 10, 21, 15, 0);
        assert!(!needs_confirmation(&info(None), now));
    }
}
