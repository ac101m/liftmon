//! The per-muscle-group exercise browse: pick a muscle group with the plain
//! list, then one of that group's exercises. It is the "exercises by muscle
//! group" arm of the shared "Select exercise" submenu
//! ([`super::select_exercise`]).
//!
//! The muscle-group step is the same plain (non-filtering) list the "Muscle
//! groups" main-menu item uses, except it lists only groups that at least one
//! exercise trains as a primary mover, each row showing that count
//! (e.g. "Abs (2 exercises)"). The exercise step is a filter picker, prefilled
//! with just this group's exercises. Escape backs out one level at a time: off
//! the exercise list you are offered another group, and off the group list you
//! return to the submenu, which the caller re-asks.

use rusqlite::Connection;

use crate::domain::{Exercise, MuscleGroup, MuscleGroupCount};

use crate::ui::utils::{Shell, UiError};

/// Pick a muscle group with the plain list, then one of that group's exercises
/// with the filter picker. Only groups that at least one exercise trains as a
/// primary mover are listed — each with its count — so a picked group always
/// has exercises. `None` means the user backed out of the group list.
pub(super) fn pick_exercise(
    shell: &mut Shell,
    conn: &Connection,
) -> Result<Option<Exercise>, UiError> {
    let mut selected = 0;
    loop {
        let groups = MuscleGroup::with_high_exercises(conn)?;
        if groups.is_empty() {
            shell.log("No exercises under a muscle group yet");
            return Ok(None);
        }
        // Each row shows the group's name with how many exercises activate it
        // at 'high' stimulus, e.g. "Abs (2 exercises)".
        let group_labels: Vec<String> = groups.iter().map(high_count_label).collect();
        // A group no exercise activates re-offers this picker, so the re-ask
        // keeps the highlight on the group just picked (like any menu that
        // stays open after an action) instead of springing back to the top.
        let (picked, picked_here) = shell.ask_selected(
            Some("Select muscle group".to_string()),
            group_labels,
            selected,
        )?;
        let Some(picked) = picked else {
            return Ok(None); // Escape: backed out of the flow
        };
        selected = picked_here;
        // The picker returns the rendered label, so the picked row is the one
        // whose label matches it exactly.
        let group = &groups
            .iter()
            .find(|count| high_count_label(count) == picked)
            .expect("the picked label came from this list a moment ago")
            .group;

        let exercises = Exercise::for_muscle_group(conn, group.id)?;
        let exercise_names: Vec<String> = exercises.iter().map(|e| e.name.clone()).collect();
        let Some(chosen) = shell.pick(
            Some(format!("Select exercise · {}", group.name)),
            Some("type to filter".to_string()),
            exercise_names,
        )?
        else {
            continue; // Escape: back to the group picker
        };
        // Names are UNIQUE, so the picked name identifies exactly one exercise.
        let exercise = exercises
            .into_iter()
            .find(|e| e.name == chosen)
            .expect("picked a listed exercise");
        return Ok(Some(exercise));
    }
}

/// The group picker's row for `count`: the name with how many exercises
/// activate it at 'high' stimulus, e.g. "Pecs (3 exercises)" — one stays
/// singular. [`Shell::ask_selected`] hands the rendered string back as the
/// picked value, which is why [`pick_exercise`] matches the row by recomputing
/// this label.
fn high_count_label(count: &MuscleGroupCount) -> String {
    let unit = if count.high_exercise_count == 1 {
        "exercise"
    } else {
        "exercises"
    };
    format!(
        "{} ({} {})",
        count.group.name, count.high_exercise_count, unit
    )
}
