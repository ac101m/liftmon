//! The per-muscle-group exercise browse: pick a category with the filter
//! list — a muscle group, or "Unassigned" for exercises no group lists as a
//! primary mover — then one of that category's exercises. It is the "exercises
//! by muscle group" arm of the shared "Select exercise" submenu
//! ([`super::select_exercise`]).
//!
//! The category step is a filter picker ("type to filter"), listing only
//! groups that at least one exercise trains as a primary mover, each row
//! showing that count (e.g. "Abs (2 exercises)"), plus the synthetic
//! "Unassigned" row (its own count) whenever at least one exercise has no
//! `'high'` reference. The exercise step is a filter picker of just that
//! category's exercises. Escape backs out one level at a time: off the exercise
//! list you are offered another category, and off the category list you return
//! to the submenu, which the caller re-asks.

use rusqlite::Connection;

use crate::domain::{Exercise, MuscleGroup, MuscleGroupCount};

use crate::ui::utils::{Shell, UiError};

/// The rows the category list shows, in order: the groups that at least one
/// exercise trains at `'high'`, then — only when an exercise falls in it — the
/// synthetic "Unassigned" row. Each carries the exercises it would list.
enum CategoryRow {
    /// A real muscle group, with how many exercises train it at `'high'`.
    Group(MuscleGroupCount),
    /// Exercises no group lists as a primary mover (medium/low refs or none).
    Unassigned(Vec<Exercise>),
}

/// Pick a category with the filter picker, then one of that category's
/// exercises with the filter picker. Only categories holding at least one
/// exercise are listed — each with its count — so a picked category always has
/// exercises. `None` means the user backed out of the category picker.
pub(super) fn pick_exercise(
    shell: &mut Shell,
    conn: &Connection,
) -> Result<Option<Exercise>, UiError> {
    loop {
        let rows = category_rows(conn)?;
        if rows.is_empty() {
            shell.log("No exercises under a muscle group yet");
            return Ok(None);
        }
        // The same filter list the exercise step uses, so a category is found by
        // typing rather than scrolling the whole list. A row's label identifies
        // it unambiguously — group names are unique, and there is at most one
        // "Unassigned" row — so the picked label resolves straight back to its
        // row via the same pure helper that built it.
        let Some(chosen) = shell.pick(
            Some("Select muscle group".to_string()),
            Some("type to filter".to_string()),
            rows.iter().map(row_label),
        )?
        else {
            return Ok(None); // Escape: backed out of the flow
        };
        let picked_here = rows
            .iter()
            .position(|row| row_label(row) == chosen)
            .expect("picked a listed category");
        let (exercises, title) = match &rows[picked_here] {
            CategoryRow::Group(count) => (
                Exercise::for_muscle_group(conn, count.group.id)?,
                format!("Select exercise · {}", count.group.name),
            ),
            CategoryRow::Unassigned(unassigned) => (
                unassigned.clone(),
                "Select exercise · Unassigned".to_string(),
            ),
        };

        let exercise_names: Vec<String> = exercises.iter().map(|e| e.name.clone()).collect();
        let Some(chosen) = shell.pick(
            Some(title),
            Some("type to filter".to_string()),
            exercise_names,
        )?
        else {
            continue; // Escape: back to the category picker
        };
        // Names are UNIQUE, so the picked name identifies exactly one exercise.
        let exercise = exercises
            .into_iter()
            .find(|e| e.name == chosen)
            .expect("picked a listed exercise");
        return Ok(Some(exercise));
    }
}

/// The categories the picker lists, in order: each group that at least one
/// exercise trains at `'high'` (name order, via [`MuscleGroup::with_high_exercises`]),
/// then — only when it holds an exercise — the synthetic "Unassigned" row.
fn category_rows(conn: &Connection) -> Result<Vec<CategoryRow>, crate::domain::DomainError> {
    let mut rows: Vec<CategoryRow> = MuscleGroup::with_high_exercises(conn)?
        .into_iter()
        .map(CategoryRow::Group)
        .collect();
    let unassigned = Exercise::without_high_reference(conn)?;
    if !unassigned.is_empty() {
        rows.push(CategoryRow::Unassigned(unassigned));
    }
    Ok(rows)
}

/// The picker's row for a category: the group's name with how many exercises
/// activate it at `'high'` stimulus, or "Unassigned (N exercises)" for the
/// synthetic row — one stays singular, mirroring [`high_count_label`].
fn row_label(row: &CategoryRow) -> String {
    match row {
        CategoryRow::Group(count) => high_count_label(count),
        CategoryRow::Unassigned(exercises) => {
            let unit = if exercises.len() == 1 {
                "exercise"
            } else {
                "exercises"
            };
            format!("Unassigned ({} {})", exercises.len(), unit)
        }
    }
}

/// The group picker's row for `count`: the name with how many exercises
/// activate it at 'high' stimulus, e.g. "Pecs (3 exercises)" — one stays
/// singular.
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
