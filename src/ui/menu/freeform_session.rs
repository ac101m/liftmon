//! The freeform-session screen: the whole workout flow, driven from one screen
//! until the user pauses or ends it.
//!
//! A session lands here one way: from its options screen's "resume", whether
//! it is brand new (created on the session screen, which drops it onto its
//! options screen) or a continued one. A session's row — with `created_at`
//! stamping the moment "new session" was chosen — already exists by the time
//! this flow is reached; it only ever resumes one. The flow is an outer loop
//! over exercises (pick one, log sets against it) and an inner loop over the
//! set entry plus the performance menu:
//!
//! ```text
//! pick exercise ─► log sets ─► (next set | next exercise | exit session)
//! ```
//!
//! * "next set" starts another set for the same exercise,
//! * "next exercise" breaks back to the picker, moving on to the next
//!   exercise,
//! * "exit session" leaves the workout and returns to the session's options
//!   screen. Sessions are never "ended" — there is no end date, so exiting
//!   records nothing and the session can be resumed again later.
//!
//! Each moment a real-world event happens gets its `created_at` timestamp: the
//! session when "new session" is chosen, a performance when its exercise is
//! picked, and a set when it is logged. An exercise performance is created for
//! every picked exercise, and a set row for every completed set (Escape
//! anywhere in the set form cancels that set).

use chrono::Utc;
use rusqlite::Connection;

use crate::domain::{Exercise, ExercisePerformance, Session, Set};

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::Menu;

/// Drive the session until the user leaves it, then land back on its options
/// screen. The session must already exist — the session screen creates the row
/// when "new session" is chosen, and this flow only ever resumes it.
pub(super) fn run(
    shell: &mut Shell,
    conn: &Connection,
    session_id: i64,
) -> Result<Option<Menu>, UiError> {
    let Some(session) = Session::find_by_id(conn, session_id)? else {
        shell.log("That session is no longer there");
        return Ok(Some(Menu::Sessions));
    };
    // Leaving the flow — backing out of the picker, or exiting the session —
    // lands back on this session's options screen.
    let return_to = Menu::SessionOptions { session_id };
    // A session starts at its most recent exercise; the entry is consumed
    // once, after which the flow behaves like a fresh one. An empty session (a
    // new one the user hasn't worked yet) has no last performance, so it falls
    // into the pick flow and carries on adding to this same session.
    let mut current: Option<(Exercise, ExercisePerformance)> = match session.performances.last() {
        Some(performance) => {
            let Some(exercise) = Exercise::find_by_id(conn, performance.exercise_id)? else {
                shell.log("That exercise is no longer there");
                return Ok(Some(return_to));
            };
            Some((exercise, performance.clone()))
        }
        None => None,
    };
    'exercises: loop {
        // A session with a last exercise lands on that exercise's performance
        // menu rather than straight into the weight form, so the user chooses
        // whether to add a set, move on, or exit. Every later (picked)
        // exercise keeps asking a set first, as before.
        let (exercise, performance, mut menu_first) = match current.take() {
            Some(seeded) => (seeded.0, seeded.1, true),
            None => {
                let Some(pair) = begin_exercise(shell, conn, &session)? else {
                    return Ok(Some(return_to)); // backed out of the picker
                };
                (pair.0, pair.1, false)
            }
        };
        loop {
            if !menu_first {
                ask_set(shell, conn, &exercise, &performance)?;
            }
            menu_first = false;
            // The performance menu. There is no "back" option on it, so Escape
            // just re-asks the menu rather than starting another set.
            loop {
                match shell
                    .ask(
                        Some(exercise.name.clone()),
                        ["next set", "next exercise", "exit session"],
                    )?
                    .as_deref()
                {
                    // Another set for the same exercise.
                    Some("next set") => break,
                    // Break back to the picker for the next exercise.
                    Some("next exercise") => continue 'exercises,
                    // "exit session": leave the workout, session intact, and
                    // land back on this session's options screen.
                    Some("exit session") => return Ok(Some(return_to)),
                    // Escape: no back option here — re-ask the menu.
                    _ => {}
                }
            }
        }
    }
}

/// Pick the next exercise and start a performance for it — its `created_at`
/// stamps the moment the exercise was selected. `None` means the user backed
/// out of the picker (or there is nothing to pick), and the caller returns to
/// where the flow was launched from.
fn begin_exercise(
    shell: &mut Shell,
    conn: &Connection,
    session: &Session,
) -> Result<Option<(Exercise, ExercisePerformance)>, UiError> {
    let exercises = Exercise::all(conn)?;
    if exercises.is_empty() {
        shell.log("No exercises yet — add one first");
        return Ok(None);
    }
    let names: Vec<String> = exercises.iter().map(|e| e.name.clone()).collect();
    let Some(chosen) = shell.pick(
        Some("Select exercise".to_string()),
        Some("type to filter".to_string()),
        names,
    )?
    else {
        return Ok(None); // Escape: backed out
    };
    // Names are UNIQUE, so the picked name identifies exactly one exercise.
    let exercise = exercises
        .into_iter()
        .find(|e| e.name == chosen)
        .expect("picked a listed exercise");
    let performance = ExercisePerformance::create(conn, session.id, exercise.id, Utc::now())?;
    super::log_exercise(shell, &exercise);
    Ok(Some((exercise, performance)))
}

/// Ask for one set — weight, then rep count, then a Yes/No on whether it was
/// taken to failure — and record it with `created_at` stamped when it is
/// logged. Escape anywhere in the set form cancels the set (nothing is
/// created) and the caller shows the performance menu.
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
        Utc::now(),
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
        match shell.read_line(
            Some("Weight (kg)".to_string()),
            Some("e.g. 100".to_string()),
            &pending,
        )? {
            ReadOutcome::Value(raw) => match parse_weight(&raw) {
                Some(weight) => return Ok(Some(weight)),
                None => {
                    shell.log("Enter the weight as a number, e.g. 100 or 102.5");
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
        match shell.read_line(
            Some("Reps".to_string()),
            Some("e.g. 5".to_string()),
            &pending,
        )? {
            ReadOutcome::Value(raw) => match parse_reps(&raw) {
                Some(reps) => return Ok(Some(reps)),
                None => {
                    shell.log("Enter the rep count as a whole number, e.g. 5");
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
