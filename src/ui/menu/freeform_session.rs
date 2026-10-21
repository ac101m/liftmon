//! The freeform-session screen: the whole workout flow, driven from one screen
//! until the user pauses or ends it.
//!
//! Choosing "begin freeform session" (or resuming an unfinished one) lands
//! here. The flow is an outer loop over exercises (pick one, log sets against
//! it) and an inner loop over the set entry plus the performance menu:
//!
//! ```text
//! pick exercise ─► log sets ─► (next set | next exercise | pause session | end session)
//! ```
//!
//! * "next set" starts another set for the same exercise,
//! * "next exercise" breaks back to the picker, moving on to the next
//!   exercise,
//! * "pause session" returns to the session menu leaving the session open
//!   (its `end_date` stays NULL — the resume screen brings it back),
//! * "end session" records today as the end date.
//!
//! A brand-new session's row is created lazily when the first exercise is
//! picked, so backing out of the picker before anything was recorded leaves
//! nothing behind. A resumed session skips the picker on entry and drops
//! straight into the set form for its most recent exercise, carrying on where
//! the user left off. An exercise performance is created for every picked
//! exercise, and a set row for every completed set (Escape in the set form
//! cancels that set).

use chrono::{Local, NaiveDate, NaiveTime};
use rusqlite::Connection;

use crate::domain::{Exercise, ExercisePerformance, Session, Set};

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::{FreeformStart, Menu};

/// Drive the session until the user pauses or ends it, then return to the
/// session screen.
pub(super) fn run(
    shell: &mut Shell,
    conn: &Connection,
    start: FreeformStart,
) -> Result<Option<Menu>, UiError> {
    // A brand-new session is created on the first exercise picked, so an
    // abandoned attempt leaves no empty session behind; a resumed one loads
    // up front.
    let mut session: Option<Session> = match start {
        FreeformStart::New => None,
        FreeformStart::Resume { session_id } => match Session::find_by_id(conn, session_id)? {
            Some(session) => Some(session),
            None => {
                shell.log("that session is no longer there");
                return Ok(Some(Menu::Sessions));
            }
        },
    };
    // A resumed session starts at its most recent exercise; the entry is
    // consumed once, after which the flow behaves like a fresh one.
    let mut current: Option<(Exercise, ExercisePerformance)> = match &session {
        None => None,
        Some(session) => match session.performances.last() {
            Some(performance) => {
                let Some(exercise) = Exercise::find_by_id(conn, performance.exercise_id)? else {
                    shell.log("that exercise is no longer there");
                    return Ok(Some(Menu::Sessions));
                };
                Some((exercise, performance.clone()))
            }
            // An open session with nothing logged yet; not reachable from the
            // resume list today, but fall back to the pick flow, which carries
            // on adding to this same session.
            None => None,
        },
    };
    'exercises: loop {
        let (exercise, performance) = match current.take() {
            Some(seeded) => seeded,
            None => {
                let Some(pair) = begin_exercise(shell, conn, &mut session)? else {
                    return Ok(Some(Menu::Sessions)); // backed out of the picker
                };
                pair
            }
        };
        loop {
            ask_set(shell, conn, &exercise, &performance)?;
            // The performance menu. There is no "back" option on it, so Escape
            // just re-asks the menu rather than starting another set.
            loop {
                match shell
                    .ask(
                        Some(exercise.name.clone()),
                        ["next set", "next exercise", "pause session", "end session"],
                    )?
                    .as_deref()
                {
                    // Another set for the same exercise.
                    Some("next set") => break,
                    // Break back to the picker for the next exercise.
                    Some("next exercise") => continue 'exercises,
                    Some("pause session") => {
                        shell.log("session paused — still open");
                        return Ok(Some(Menu::Sessions));
                    }
                    // "end session": the session is only ever None before the
                    // first exercise, which this loop cannot have reached.
                    Some("end session") => {
                        session.as_mut().expect("started").end(conn, today())?;
                        shell.log("session ended");
                        return Ok(Some(Menu::Sessions));
                    }
                    // Escape: no back option here — re-ask the menu.
                    _ => {}
                }
            }
        }
    }
}

/// Pick the next exercise and start a performance for it, creating the session
/// on the way in when this is the first exercise. `None` means the user backed
/// out of the picker (or there is nothing to pick), and the caller returns to
/// the session screen.
fn begin_exercise(
    shell: &mut Shell,
    conn: &Connection,
    session: &mut Option<Session>,
) -> Result<Option<(Exercise, ExercisePerformance)>, UiError> {
    let exercises = Exercise::all(conn)?;
    if exercises.is_empty() {
        shell.log("no exercises yet — add one first");
        return Ok(None);
    }
    let names: Vec<String> = exercises.iter().map(|e| e.name.clone()).collect();
    let Some(chosen) = shell.pick(Some("Select exercise".to_string()), names)? else {
        return Ok(None); // Escape: backed out
    };
    // Names are UNIQUE, so the picked name identifies exactly one exercise.
    let exercise = exercises
        .into_iter()
        .find(|e| e.name == chosen)
        .expect("picked a listed exercise");
    let session = match session {
        Some(session) => session,
        None => {
            *session = Some(Session::create(conn, today())?);
            session.as_mut().expect("just created")
        }
    };
    let performance = ExercisePerformance::create(conn, session.id, exercise.id, today(), now())?;
    super::log_exercise(shell, &exercise);
    Ok(Some((exercise, performance)))
}

/// Ask for one set — weight, then rep count, then a Yes/No on whether it was
/// taken to failure — and record it. Escape anywhere in the set form cancels
/// the set (nothing is created) and the caller shows the performance menu.
fn ask_set(
    shell: &mut Shell,
    conn: &Connection,
    exercise: &Exercise,
    performance: &ExercisePerformance,
) -> Result<(), UiError> {
    let Some(weight_kg) = ask_weight(shell)? else {
        return Ok(());
    };
    let Some(reps) = ask_reps(shell)? else {
        return Ok(());
    };
    let Some(to_failure) = shell.ask(Some("Taken to failure?".to_string()), ["Yes", "No"])? else {
        return Ok(());
    };
    let to_failure = to_failure == "Yes";
    Set::create(
        conn,
        performance.id,
        exercise.id,
        weight_kg,
        reps,
        to_failure,
    )?;
    shell.log(format!(
        "{weight_kg} kg × {reps}{}",
        if to_failure { " · to failure" } else { "" }
    ));
    Ok(())
}

/// Ask for the set's weight in kg, re-asking (with what was typed prefilled)
/// until it parses. `None` when the user backed out with Escape.
fn ask_weight(shell: &mut Shell) -> Result<Option<f64>, UiError> {
    let mut pending = String::new();
    loop {
        match shell.read_line(Some("Weight (kg)".to_string()), &pending)? {
            ReadOutcome::Value(raw) => match parse_weight(&raw) {
                Some(weight) => return Ok(Some(weight)),
                None => {
                    shell.log("enter the weight as a number, e.g. 100 or 102.5");
                    pending = raw;
                }
            },
            ReadOutcome::Cancelled { .. } => return Ok(None),
        }
    }
}

/// Ask for the set's rep count, re-asking (with what was typed prefilled)
/// until it parses. `None` when the user backed out with Escape.
fn ask_reps(shell: &mut Shell) -> Result<Option<u32>, UiError> {
    let mut pending = String::new();
    loop {
        match shell.read_line(Some("Reps".to_string()), &pending)? {
            ReadOutcome::Value(raw) => match parse_reps(&raw) {
                Some(reps) => return Ok(Some(reps)),
                None => {
                    shell.log("enter the rep count as a whole number, e.g. 5");
                    pending = raw;
                }
            },
            ReadOutcome::Cancelled { .. } => return Ok(None),
        }
    }
}

/// Parse a weight entry: a positive number in kg, e.g. "100" or "102.5".
fn parse_weight(raw: &str) -> Option<f64> {
    let raw = raw.trim();
    match raw.parse::<f64>() {
        Ok(weight) if weight > 0.0 => Some(weight),
        _ => None,
    }
}

/// Parse a rep count: a positive whole number, e.g. "5".
fn parse_reps(raw: &str) -> Option<u32> {
    let raw = raw.trim();
    match raw.parse::<u32>() {
        Ok(reps) if reps > 0 => Some(reps),
        _ => None,
    }
}

/// Today's date — the session's start/end dates and each performance's date.
fn today() -> NaiveDate {
    Local::now().date_naive()
}

/// The current time of day, when a performance is recorded.
fn now() -> NaiveTime {
    Local::now().time()
}

#[cfg(test)]
mod tests {
    use super::{parse_reps, parse_weight};

    #[test]
    fn weight_parses_a_positive_number() {
        assert_eq!(parse_weight("100"), Some(100.0));
        assert_eq!(parse_weight(" 102.5 "), Some(102.5));
        assert_eq!(parse_weight("0"), None);
        assert_eq!(parse_weight("-5"), None);
        assert_eq!(parse_weight("abc"), None);
        assert_eq!(parse_weight(""), None);
    }

    #[test]
    fn reps_parse_a_positive_whole_number() {
        assert_eq!(parse_reps("5"), Some(5));
        assert_eq!(parse_reps(" 12 "), Some(12));
        assert_eq!(parse_reps("0"), None);
        assert_eq!(parse_reps("-3"), None);
        assert_eq!(parse_reps("5.5"), None);
        assert_eq!(parse_reps("abc"), None);
        assert_eq!(parse_reps(""), None);
    }
}
