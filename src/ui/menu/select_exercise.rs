//! The "Select exercise" chooser: ask how the exercise is picked — the
//! all-exercises filter list or the per-muscle-group browse — then run that
//! picker. Shared by the exercises screen's "Browse exercises" option and the
//! freeform session's "next exercise" step. Escape backs out one level at a
//! time: off a picker you are offered the other way to pick, and off the
//! submenu you get [`None`] (the caller backs out of the flow).

use rusqlite::Connection;

use crate::domain::Exercise;

use crate::ui::utils::{Shell, UiError};

use super::select_exercise_by_group::pick_exercise;

/// Ask how the exercise is picked, then run the chosen picker. Returns the
/// picked exercise, or [`None`] when the user backed out (of the submenu) or
/// there is nothing to pick. A re-asked submenu keeps its highlight on the
/// method just used; either picker's Escape re-asks rather than leaving.
pub(super) fn pick(shell: &mut Shell, conn: &Connection) -> Result<Option<Exercise>, UiError> {
    // There is no point offering the submenu when nothing at all exists to
    // pick, so the empty-exercises case logs and leaves.
    if Exercise::all(conn)?.is_empty() {
        shell.log("No exercises yet — add one first");
        return Ok(None);
    }
    // Escaping a picker re-asks this submenu; the highlight stays on the
    // method that was just used rather than jumping back to "all exercises".
    let mut selected = 0;
    loop {
        let (choice, selected_here) = shell.ask_selected(
            Some("Select exercise".to_string()),
            ["all exercises", "exercises by muscle group"],
            selected,
        )?;
        selected = selected_here;
        match choice.as_deref() {
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
