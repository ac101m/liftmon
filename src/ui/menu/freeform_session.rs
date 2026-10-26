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
//! * "next exercise" breaks back to picking the next exercise. Beginning a
//!   session and moving on both ask first how to pick — a submenu offering the
//!   all-exercises list or the per-muscle-group browse — then run that picker.
//!   Picking the exercise that is already the current one does not start a new
//!   performance: it is called out and the menu is re-asked, so a repeated
//!   pick never doubles an exercise up in the session summary,
//! * "exit session" leaves the workout and returns to the session's options
//!   screen. Sessions are never "ended" — there is no end date, so exiting
//!   records nothing and the session can be resumed again later.
//!
//! Each moment a real-world event happens gets its `created_at` timestamp: the
//! session when "new session" is chosen, a set when it is logged — and a
//! performance stamped with the same instant as its first set. An exercise
//! performance is created only when the exercise's first set is completed, so
//! picking an exercise and moving on without logging a set never leaves an
//! empty performance behind; a set row is created for every completed set
//! (Escape anywhere in the set form cancels that set).

use chrono::Utc;
use ratatui::text::{Line, Span};
use rusqlite::Connection;

use crate::domain::{Difficulty, Exercise, ExercisePerformance, Session, Set};

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::Menu;
use super::difficulty_style;
use super::select_exercise_by_group::pick_exercise;
use super::select_session::{format_date, local_date};

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
    // Leaving the flow — backing out of the submenu or a picker, or exiting
    // the session — lands back on this session's options screen.
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
    // A resume — the session already holds work, so it has a current exercise
    // to continue — announces where the session started and which exercise the
    // user is mid-flow in, as plain lines beneath one another. Beginning a
    // brand-new session (nothing performed yet) reads as a fresh start and
    // logs nothing.
    if let Some((exercise, _)) = &current {
        shell.log(format!(
            "Resumed freeform session from {}",
            format_date(local_date(session.created_at))
        ));
        shell.log(format!("Current exercise '{}'", exercise.name));
    }
    // The active exercise and its performance, driven by one loop: ask a set
    // (unless freshly resumed onto an existing performance, which shows the
    // menu first), then re-ask the performance menu. "next exercise" picks the
    // following exercise in place, so picking the one the user is already on
    // can be caught before anything advances — it is called out and the menu
    // re-asked, rather than starting a second performance for the same
    // exercise. The performance is `None` until its first set is logged: no
    // row exists for a picked-but-unworked exercise.
    let (mut exercise, mut performance, mut menu_first) = match current.take() {
        Some((ex, perf)) => (ex, Some(perf), true),
        None => {
            let Some(ex) = begin_exercise(shell, conn)? else {
                return Ok(Some(return_to)); // backed out of the submenu/pickers
            };
            (ex, None, false)
        }
    };
    'active: loop {
        if !menu_first {
            ask_set(shell, conn, session.id, &mut performance, &exercise)?;
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
                // Pick the next exercise.
                Some("next exercise") => match pick_next_exercise(shell, conn)? {
                    // Backed out of the submenu (or a picker, after re-asking):
                    // leave the flow.
                    None => return Ok(Some(return_to)),
                    // The exercise the user is already on: no new performance,
                    // just say so and re-ask the menu.
                    Some(picked) if picked.id == exercise.id => {
                        shell.log(format!("'{}' is already selected", exercise.name));
                    }
                    // A genuinely new exercise: move to it and ask a set. Its
                    // performance is still deferred until the first set.
                    Some(picked) => {
                        shell.log(format!("Moved to next exercise '{}'", picked.name));
                        performance = None;
                        exercise = picked;
                        continue 'active;
                    }
                },
                // "exit session": leave the workout, session intact, and
                // land back on this session's options screen.
                Some("exit session") => return Ok(Some(return_to)),
                // Escape: no back option here — re-ask the menu.
                _ => {}
            }
        }
    }
}

/// Pick the next exercise. Its performance is deliberately NOT created here —
/// that is deferred until the first set is completed ([`ask_set`]), so picking
/// an exercise and then moving on without logging a set leaves no empty
/// performance behind. How it is picked is the submenu's call
/// ([`pick_next_exercise`]). `None` means the user backed out (of the submenu
/// or a picker), or there is nothing to pick, and the caller returns to where
/// the flow was launched from.
fn begin_exercise(shell: &mut Shell, conn: &Connection) -> Result<Option<Exercise>, UiError> {
    let Some(exercise) = pick_next_exercise(shell, conn)? else {
        return Ok(None);
    };
    // A picked exercise begins with a plain notice — not a framed section — so
    // a session's log reads as a contiguous stream of set lines with a small
    // marker where each exercise starts.
    shell.log(format!("Moved to next exercise '{}'", exercise.name));
    Ok(Some(exercise))
}

/// Ask how the next exercise is picked, then run the chosen picker: the
/// all-exercises filter list, or the per-muscle-group browse shared with the
/// exercises screen. Escape backs out one level at a time — off a picker you
/// are offered the other way to pick, and off the submenu you leave the flow.
/// There is no point offering the submenu when nothing at all exists to pick,
/// so the empty-exercises case logs and leaves. `None` means the user backed
/// out of the submenu.
fn pick_next_exercise(shell: &mut Shell, conn: &Connection) -> Result<Option<Exercise>, UiError> {
    if Exercise::all(conn)?.is_empty() {
        shell.log("No exercises yet — add one first");
        return Ok(None);
    }
    loop {
        match shell
            .ask(
                Some("Next exercise".to_string()),
                ["all exercises", "exercises by muscle group"],
            )?
            .as_deref()
        {
            None => return Ok(None), // Escape: leave the flow
            Some("all exercises") => {
                if let Some(exercise) = pick_any_exercise(shell, conn)? {
                    return Ok(Some(exercise));
                }
                // Escape in the picker: re-ask how to pick.
            }
            Some("exercises by muscle group") => {
                if let Some(exercise) = pick_exercise(shell, conn)? {
                    return Ok(Some(exercise));
                }
                // Escape at the group list: re-ask how to pick.
            }
            _ => unreachable!("the submenu only offers the two pickers"),
        }
    }
}

/// The all-exercises picker: every exercise in one filter list. `None` when
/// the user backed out with Escape.
fn pick_any_exercise(shell: &mut Shell, conn: &Connection) -> Result<Option<Exercise>, UiError> {
    let exercises = Exercise::all(conn)?;
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
    Ok(Some(exercise))
}

/// Ask for one set — weight, then rep count, then how tough it was — and
/// record it with `created_at` stamped when it is logged. The set's performance
/// is created here too, on the first completed set, so a performance never
/// precedes its first set: an exercise that is merely picked has no row until a
/// set is actually completed. Escape anywhere in the set form cancels the set
/// (nothing is created) and the caller shows the performance menu.
fn ask_set(
    shell: &mut Shell,
    conn: &Connection,
    session_id: i64,
    performance: &mut Option<ExercisePerformance>,
    exercise: &Exercise,
) -> Result<(), UiError> {
    let Some(weight_kg) = ask_weight(shell)? else {
        return Ok(());
    };
    let Some(reps) = ask_reps(shell)? else {
        return Ok(());
    };
    let Some(choice) = shell.ask(
        Some("How tough was that?".to_string()),
        Difficulty::ALL.map(Difficulty::menu_label),
    )?
    else {
        return Ok(());
    };
    let difficulty = Difficulty::from_label(&choice);
    // A freshly picked exercise has no performance row yet: it is created on
    // the first completed set — stamped with the same instant the set is
    // logged — and the performance this leaves behind carries every later set.
    // So a performance exists only when its exercise's first set is actually
    // completed, and picking an exercise without logging a set never leaves an
    // empty performance in the session (or in its summary).
    let created_at = Utc::now();
    let performance_id = match performance.as_ref().map(|p| p.id) {
        Some(id) => id,
        None => {
            let created = ExercisePerformance::create(conn, session_id, exercise.id, created_at)?;
            let id = created.id;
            *performance = Some(created);
            id
        }
    };
    let (set, is_personal_best) = Set::create(
        conn,
        performance_id,
        exercise.id,
        weight_kg,
        reps,
        difficulty,
        created_at,
    )?;
    // The set as the session log shows it — the e1RM behind a personal best
    // never appears; only the weight, reps and how tough it was do, the
    // difficulty marker coloured (green easy, yellow hard, red failure) like
    // the session summary's. A personal best rides at the end of the line
    // rather than on its own.
    let mut spans = vec![
        Span::raw(format!("{} kg × {} · ", set.weight_kg, set.reps)),
        Span::styled(set.difficulty.as_str(), difficulty_style(set.difficulty)),
    ];
    if is_personal_best {
        spans.push(Span::raw(" · New PR!"));
    }
    shell.log(Line::from(spans));
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
