//! The "list sessions" screen: pick a session with the filter box below;
//! selecting one opens its per-session options screen.
//!
//! Each entry is named by the day of its most recent activity and how long
//! ago, e.g. `"2026/10/20 · (3 hours ago)"`. The list is built from the
//! lightweight [`SessionInfo`] rows rather than full sessions — one row per
//! session, no performances/sets tree — so listing a long history stays cheap.
//! Selecting one hands the session to [`super::session_options`], which can
//! resume it (with the 6-hour last-activity check) or back out.

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeDelta, Utc};
use rusqlite::Connection;

use crate::domain::{Session, SessionInfo};
use crate::ui::utils::{Shell, UiError};

use super::Menu;

/// Ask the user to pick a session, landing on its options screen. Escape backs
/// out with nothing happening.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    // `Session::list_info` orders newest first too, so the list comes back
    // most recent first.
    let sessions = Session::list_info(conn)?;
    if sessions.is_empty() {
        shell.log("No sessions yet — start one first");
        return Ok(Some(Menu::Sessions));
    }
    // One "now" for the whole screen visit, so every label reads the same.
    let now = Local::now().naive_local();
    let labels: Vec<String> = sessions.iter().map(|s| session_label(s, now)).collect();
    let Some(chosen) = shell.pick(
        Some("Select session".to_string()),
        Some("type to filter".to_string()),
        labels,
    )?
    else {
        return Ok(Some(Menu::Sessions)); // Escape: backed out
    };
    let session = sessions
        .iter()
        .find(|s| session_label(s, now) == chosen)
        .expect("picked a listed session");
    Ok(Some(Menu::SessionOptions {
        session_id: session.id,
    }))
}

/// The choice-list label for a session: the day of its most recent activity
/// and how long ago, e.g. `"2026/10/20 · (3 hours ago)"`. Every session has a
/// last-activity moment — the latest set, or the session's own creation when
/// nothing is logged yet — so there is no bare-date fallback.
fn session_label(info: &SessionInfo, now: NaiveDateTime) -> String {
    let at = info.last_activity_at.with_timezone(&Local);
    format!(
        "{} · ({})",
        format_date(at.date_naive()),
        time_ago(at.naive_local(), now)
    )
}

/// The local calendar date of a UTC instant, for showing moments date-only on
/// the session screens. Shared with [`super::session_options`] so a session
/// reads the same everywhere.
pub(super) fn local_date(dt: DateTime<Utc>) -> NaiveDate {
    dt.with_timezone(&Local).date_naive()
}

/// A date the session screens show one as, e.g. `2026/10/20`. Shared with
/// [`super::session_options`] so a session reads the same everywhere.
pub(super) fn format_date(date: NaiveDate) -> String {
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

#[cfg(test)]
mod tests {
    use super::{format_date, session_label, time_ago};
    use crate::domain::SessionInfo;
    use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, TimeZone, Utc};

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn date_time(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        date(y, m, d).and_time(NaiveTime::from_hms_opt(h, min, 0).unwrap())
    }

    /// A UTC instant built from a local wall clock, so its local date is
    /// exactly `y/m/d` no matter where the tests run.
    fn dt(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Local
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .single()
            .expect("unambiguous local time")
            .with_timezone(&Utc)
    }

    /// A session whose most recent activity was at `last_at` (its own creation
    /// moment, like an empty session).
    fn info(last_at: DateTime<Utc>) -> SessionInfo {
        SessionInfo {
            id: 1,
            created_at: last_at,
            last_activity_at: last_at,
        }
    }

    #[test]
    fn label_shows_the_date_and_how_long_ago() {
        let now = dt(2026, 10, 21, 15, 0);
        // The screen passes `Local::now().naive_local()`; the tests match it.
        let now_local = now.with_timezone(&Local).naive_local();
        let hours_ago = now - TimeDelta::hours(11); // same day
        assert_eq!(
            session_label(&info(hours_ago), now_local),
            format!(
                "{} · (11 hours ago)",
                format_date(hours_ago.with_timezone(&Local).date_naive())
            )
        );
        // The date is the last activity's day, not today's.
        let days_ago = now - TimeDelta::days(15);
        assert_eq!(
            session_label(&info(days_ago), now_local),
            format!(
                "{} · (15 days ago)",
                format_date(days_ago.with_timezone(&Local).date_naive())
            )
        );
        // Nothing logged yet (a fresh session): still date + relative time,
        // reading its creation moment.
        let just_created = now - TimeDelta::seconds(30);
        assert_eq!(
            session_label(&info(just_created), now_local),
            format!(
                "{} · (just now)",
                format_date(just_created.with_timezone(&Local).date_naive())
            )
        );
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
}
