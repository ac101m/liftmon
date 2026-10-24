//! The muscle-group picker's data plumbing: turning the domain's stored
//! references into picker rows and back.
//!
//! Not a screen of its own — this module serves the two call sites inside
//! existing screens (the add-exercise form and the per-exercise options menu),
//! so it never appears in the [`super::Menu`] dispatcher. It translates between
//! the two enums: the picker's four-state [`Level`] (including "not selected")
//! lives in the domain-free UI layer, and the persisted three-state
//! [`Stimulus`] lives in the domain. Name → id resolution happens here too,
//! since muscle groups are picked and shown by name but stored by id.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::domain::{Exercise, MuscleGroup, MuscleGroupReference, Stimulus};

use crate::ui::utils::{CycleOutcome, Level, Shell, UiError};

/// One row per muscle group, in name order, all unselected — the create-flow
/// picker's starting point.
pub(super) fn unselected_rows(conn: &Connection) -> Result<Vec<(String, Level)>, UiError> {
    Ok(MuscleGroup::all(conn)?
        .into_iter()
        .map(|group| (group.name, Level::None))
        .collect())
}

/// The picker's rows for an exercise that already has references: every group
/// in name order, with the stored ones overlaid at their saved level — the
/// options-menu picker's starting point.
pub(super) fn rows_for(
    conn: &Connection,
    exercise_id: i64,
) -> Result<Vec<(String, Level)>, UiError> {
    let refs = MuscleGroupReference::for_exercise(conn, exercise_id)?;
    let by_id: HashMap<i64, Stimulus> = refs
        .into_iter()
        .map(|reference| (reference.muscle_group_id, reference.stimulus))
        .collect();
    Ok(MuscleGroup::all(conn)?
        .into_iter()
        .map(|group| {
            let level = by_id
                .get(&group.id)
                .map(|&stimulus| level(stimulus))
                .unwrap_or(Level::None);
            (group.name, level)
        })
        .collect())
}

/// The stimulus the stored [`Stimulus`] reads back as, for the picker.
fn level(stimulus: Stimulus) -> Level {
    match stimulus {
        Stimulus::Auxiliary => Level::Auxiliary,
        Stimulus::Secondary => Level::Secondary,
        Stimulus::Primary => Level::Primary,
    }
}

/// The stimulus a selected picker level carries, or `None` for an unselected
/// row (which stores nothing).
fn stimulus(level: Level) -> Option<Stimulus> {
    match level {
        Level::None => None,
        Level::Auxiliary => Some(Stimulus::Auxiliary),
        Level::Secondary => Some(Stimulus::Secondary),
        Level::Primary => Some(Stimulus::Primary),
    }
}

/// The selected rows of a submitted picker as `(name, stimulus)` pairs, in row
/// order; unselected rows are dropped. Pure, so it is unit-testable.
pub(super) fn stimulus_pairs(rows: &[(String, Level)]) -> Vec<(String, Stimulus)> {
    rows.iter()
        .filter_map(|(name, level)| stimulus(*level).map(|stimulus| (name.clone(), stimulus)))
        .collect()
}

/// Write `rows` as the exercise's muscle-group references, replacing any that
/// were there before (the submitted selection is authoritative). Returns how
/// many references were saved, for the log line.
pub(super) fn save_references(
    conn: &Connection,
    exercise_id: i64,
    rows: &[(String, Level)],
) -> Result<usize, UiError> {
    // Muscle group names are unique, so the id lookup is exact; only the names
    // the user actually selected need resolving.
    let ids: HashMap<String, i64> = MuscleGroup::all(conn)?
        .into_iter()
        .map(|group| (group.name, group.id))
        .collect();
    let pairs = stimulus_pairs(rows);
    let mut resolved = Vec::with_capacity(pairs.len());
    for (name, stimulus) in pairs {
        // Every name came from MuscleGroup::all moments ago, so it always
        // resolves; a group deleted out from under us is skipped rather than
        // failing the whole save.
        if let Some(&id) = ids.get(&name) {
            resolved.push((id, stimulus));
        }
    }
    MuscleGroupReference::set_for_exercise(conn, exercise_id, &resolved)?;
    Ok(resolved.len())
}

/// The options-menu entry point: pick the exercise's muscle groups, then write
/// them. A submitted selection is saved with a log line; backing out changes
/// nothing (like Edit description).
pub(super) fn edit_muscle_groups(
    shell: &mut Shell,
    conn: &Connection,
    exercise: &Exercise,
) -> Result<(), UiError> {
    match shell.cycle_pick(
        Some("Muscle groups".to_string()),
        rows_for(conn, exercise.id)?,
    )? {
        CycleOutcome::Saved(page) => {
            let saved = save_references(conn, exercise.id, &page)?;
            shell.log(format!(
                "Saved {saved} muscle group(s) for '{}'",
                exercise.name
            ));
        }
        CycleOutcome::Cancelled { .. } => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stimulus_pairs_drops_unselected_rows_and_maps_the_rest_in_order() {
        let rows = vec![
            ("Abs".to_string(), Level::None),
            ("Biceps".to_string(), Level::Auxiliary),
            ("Front delts".to_string(), Level::Secondary),
            ("Pecs".to_string(), Level::Primary),
            ("Lats".to_string(), Level::None),
        ];
        let pairs = stimulus_pairs(&rows);
        assert_eq!(
            pairs,
            vec![
                ("Biceps".to_string(), Stimulus::Auxiliary),
                ("Front delts".to_string(), Stimulus::Secondary),
                ("Pecs".to_string(), Stimulus::Primary),
            ],
            "unselected rows contribute nothing, and row order is preserved"
        );
    }
}
