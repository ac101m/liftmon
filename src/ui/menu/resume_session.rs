//! The resume-session screen: pick an unfinished session to continue.
//!
//! Unfinished means the session has no `end_date` — it was paused or
//! interrupted. The list is a plain choice list (no filter, unlike the
//! exercise picker), ordered most recent first. Selecting one hands the
//! session to the freeform flow, which resumes at the most recent exercise.

use rusqlite::Connection;

use crate::domain::Session;
use crate::ui::utils::{Shell, UiError};

use super::{FreeformStart, Menu};

/// Ask which unfinished session to resume, returning the freeform flow seeded
/// with it — or the session screen when there is nothing to resume or the user
/// backs out with Escape.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    // `Session::all` already loads newest first, so the unfinished ones come
    // back most recent first too.
    let unfinished: Vec<Session> = Session::all(conn)?
        .into_iter()
        .filter(|s| s.end_date.is_none())
        .collect();
    if unfinished.is_empty() {
        shell.log("no unfinished sessions to resume");
        return Ok(Some(Menu::Sessions));
    }

    let Some(chosen) = shell.ask(
        Some("Resume session".to_string()),
        unfinished.iter().map(session_label).collect::<Vec<_>>(),
    )?
    else {
        return Ok(Some(Menu::Sessions)); // Escape: backed out
    };
    let session = unfinished
        .into_iter()
        .find(|s| session_label(s) == chosen)
        .expect("picked a listed session");
    Ok(Some(Menu::FreeformSession(FreeformStart::Resume {
        session_id: session.id,
    })))
}

/// The choice-list label for an unfinished session: when it started and how
/// many sets it already holds, e.g. `"2026-10-20 · 5 sets"`.
fn session_label(session: &Session) -> String {
    let sets: usize = session.performances.iter().map(|p| p.sets.len()).sum();
    format!(
        "{} · {} set{}",
        session.start_date,
        sets,
        if sets == 1 { "" } else { "s" }
    )
}

#[cfg(test)]
mod tests {
    use super::session_label;
    use crate::domain::{ExercisePerformance, Session, Set};
    use chrono::{NaiveDate, NaiveTime};

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// A session on 2026-10-20 whose single performance (09:00) holds `sets`.
    fn session_with(sets: Vec<Set>) -> Session {
        Session {
            id: 1,
            start_date: date(2026, 10, 20),
            end_date: None,
            performances: vec![ExercisePerformance {
                id: 1,
                session_id: 1,
                exercise_id: 1,
                date: date(2026, 10, 20),
                time_of_day: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                sets,
            }],
        }
    }

    fn set() -> Set {
        Set {
            id: 1,
            exercise_performance_id: 1,
            exercise_id: 1,
            weight_kg: 100.0,
            reps: 5,
            to_failure: true,
        }
    }

    #[test]
    fn label_names_start_date_and_totals_the_sets() {
        assert_eq!(session_label(&session_with(vec![])), "2026-10-20 · 0 sets");
        assert_eq!(
            session_label(&session_with(vec![set()])),
            "2026-10-20 · 1 set"
        );
        assert_eq!(
            session_label(&session_with(vec![set(), set(), set()])),
            "2026-10-20 · 3 sets"
        );
    }
}
