//! The "show exercises by muscle group" screen: pick a muscle group with the
//! plain list, then pick one of that group's exercises; selecting one opens
//! its per-exercise actions screen.
//!
//! The muscle-group step is the same plain (non-filtering) list the "Muscle
//! groups" main-menu item uses; the exercise step is the filter picker from
//! "Show all exercises", prefilled with just this group's exercises. The
//! group-first pick itself ([`pick_exercise`]) is also the session's
//! "exercises by muscle group" picker. Escape backs out one level at a time:
//! off the exercise list you are offered another group, and off the group
//! list you leave the flow.

use rusqlite::Connection;

use crate::domain::{Exercise, MuscleGroup};

use crate::ui::utils::{Shell, UiError};

use super::Menu;

/// Ask the user to pick a muscle group, then an exercise that activates it,
/// landing on that exercise's actions screen. Escape leaves the flow with
/// nothing happening.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    Ok(match pick_exercise(shell, conn)? {
        Some(exercise) => Some(Menu::ExerciseOptions {
            name: exercise.name,
        }),
        None => Some(Menu::Exercises), // backed out of the flow
    })
}

/// Pick a muscle group with the plain list, then one of that group's exercises
/// with the filter picker — shared by this screen and the session's
/// "exercises by muscle group" picker. A group no exercise activates logs so
/// and re-offers the group picker; `None` means the user backed out of the
/// group list.
pub(super) fn pick_exercise(
    shell: &mut Shell,
    conn: &Connection,
) -> Result<Option<Exercise>, UiError> {
    loop {
        let groups = MuscleGroup::all(conn)?;
        if groups.is_empty() {
            shell.log("No muscle groups yet");
            return Ok(None);
        }
        let group_names: Vec<String> = groups.iter().map(|g| g.name.clone()).collect();
        let Some(picked) = shell.ask(Some("Select muscle group".to_string()), group_names)? else {
            return Ok(None); // Escape: backed out of the flow
        };
        let group = groups
            .into_iter()
            .find(|g| g.name == picked)
            .expect("the picked name came from this list a moment ago");

        let exercises = Exercise::for_muscle_group(conn, group.id)?;
        if exercises.is_empty() {
            shell.log(format!("No exercises activate '{}'", group.name));
            continue; // back to the group picker for another group
        }
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
