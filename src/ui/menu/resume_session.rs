//! The resume-session screen: pick a session to carry on with.
//!
//! Any session is resumable — not just open ones (there is no pause any more,
//! a session is either ongoing or ended, and either can be picked up again).
//! A session whose most recent exercise was more than 6 hours ago is probably
//! not the workout the user is actually mid-flow in, so picking one asks for a
//! Yes/No confirmation first. Resuming a finished session reopens it (its end
//! date is cleared), so the new work is logged into an open session.
//!
//! The list is a plain choice list (no filter, unlike the exercise picker),
//! ordered most recent first, each entry named by when its most recent
//! exercise happened. It is built from the lightweight [`SessionInfo`] rows
//! rather than full sessions — no `sets` join, one row per session — so
//! listing a long history stays cheap. Selecting one hands the session to the
//! freeform flow, which resumes at the most recent exercise.

use chrono::{Local, NaiveDateTime, TimeDelta};
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

    loop {
        let Some(chosen) = shell.ask(
            Some("Resume session".to_string()),
            sessions.iter().map(session_label).collect::<Vec<_>>(),
        )?
        else {
            return Ok(Some(Menu::Sessions)); // Escape: backed out
        };
        let session = sessions
            .iter()
            .find(|s| session_label(s) == chosen)
            .expect("picked a listed session");

        // A session idle for over 6 hours probably belongs to an earlier
        // workout — check before reopening it. A fresh session (resumed within
        // `CONFIRM_AFTER`) flows straight on.
        if needs_confirmation(session, Local::now().naive_local()) {
            shell.log("That session's last exercise was over 6 hours ago.");
            let sure = shell.ask(
                Some(format!("Resume session from {}?", session.start_date)),
                ["Yes", "No"],
            )?;
            if sure.as_deref() != Some("Yes") {
                continue; // re-ask the list
            }
        }

        // Resuming a finished session reopens it, so newly logged sets never
        // land after its old end date. Only the id is needed — the freeform
        // flow loads the full session itself once a resume happens.
        if session.end_date.is_some() {
            Session::reopen_by_id(conn, session.id)?;
        }
        return Ok(Some(Menu::FreeformSession(FreeformStart::Resume {
            session_id: session.id,
        })));
    }
}

/// The choice-list label for a session: when its most recent exercise
/// happened, and (while still open) that it is, e.g.
/// `"2026-10-20 19:41 · open"`.
fn session_label(info: &SessionInfo) -> String {
    let when = match info.last_exercise_at {
        Some(at) => at.format("%Y-%m-%d %H:%M").to_string(),
        // A session with nothing logged yet has no moment to name; fall back
        // to its start date.
        None => info.start_date.to_string(),
    };
    let open = if info.end_date.is_none() {
        " · open"
    } else {
        ""
    };
    format!("{when}{open}")
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
    use super::{CONFIRM_AFTER, needs_confirmation, session_label};
    use crate::domain::SessionInfo;
    use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn date_time(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        date(y, m, d).and_time(NaiveTime::from_hms_opt(h, min, 0).unwrap())
    }

    /// A session on 2026-10-20 whose most recent exercise was at `last_at`,
    /// ended or still open.
    fn info(last_at: Option<NaiveDateTime>, ended: bool) -> SessionInfo {
        SessionInfo {
            id: 1,
            start_date: date(2026, 10, 20),
            end_date: if ended {
                Some(date(2026, 10, 20))
            } else {
                None
            },
            last_exercise_at: last_at,
        }
    }

    #[test]
    fn label_names_when_the_last_exercise_happened() {
        let at = date_time(2026, 10, 20, 19, 41);
        assert_eq!(session_label(&info(Some(at), true)), "2026-10-20 19:41");
        // An open session says so, so the list tells them apart.
        assert_eq!(
            session_label(&info(Some(at), false)),
            "2026-10-20 19:41 · open"
        );
        // Nothing logged yet: fall back to the start date.
        assert_eq!(session_label(&info(None, false)), "2026-10-20 · open");
    }

    #[test]
    fn confirmation_needed_when_past_six_hours() {
        let now = date_time(2026, 10, 21, 15, 0);
        let recent = date_time(2026, 10, 21, 9, 30);
        let stale = date_time(2026, 10, 20, 12, 0);

        assert!(!needs_confirmation(&info(Some(recent), false), now));
        assert!(needs_confirmation(&info(Some(stale), false), now));

        // Exactly six hours is not "over" six hours.
        let borderline = now - CONFIRM_AFTER;
        assert!(!needs_confirmation(&info(Some(borderline), false), now));
    }

    #[test]
    fn session_with_no_last_exercise_never_needs_confirmation() {
        let now = date_time(2026, 10, 21, 15, 0);
        assert!(!needs_confirmation(&info(None, false), now));
    }
}
