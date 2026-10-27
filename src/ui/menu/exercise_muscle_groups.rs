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

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
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
        Stimulus::Low => Level::Low,
        Stimulus::Medium => Level::Medium,
        Stimulus::High => Level::High,
    }
}

/// The stimulus a selected picker level carries, or `None` for an unselected
/// row (which stores nothing).
fn stimulus(level: Level) -> Option<Stimulus> {
    match level {
        Level::None => None,
        Level::Low => Some(Stimulus::Low),
        Level::Medium => Some(Stimulus::Medium),
        Level::High => Some(Stimulus::High),
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

/// Append a bold `Muscle groups:` heading and each assigned group as
/// `  {name} · {marker}`, the marker word coloured like the difficulty markers
/// in the session summary (only the word carries the colour), in name order —
/// the info screen shows it between the description and personal records. `  none
/// assigned` when the exercise references no groups. The heading always shows,
/// mirroring how "Personal records:" always shows (with `  none yet`).
pub(super) fn append_muscle_groups(
    lines: &mut Vec<Line<'static>>,
    conn: &Connection,
    exercise_id: i64,
) -> Result<(), UiError> {
    let assigned: Vec<(String, Level)> = rows_for(conn, exercise_id)?
        .into_iter()
        .filter(|(_, level)| *level != Level::None)
        .collect();
    // Bold goes on the heading's span, not its line, so only the heading word
    // reads bold while the rest of the line stays plain — the same trick the
    // exercise info screen's "Personal records:" heading uses.
    lines.push(Line::from(vec![Span::styled(
        "Muscle groups:".to_string(),
        Style::new().add_modifier(Modifier::BOLD),
    )]));
    if assigned.is_empty() {
        lines.push(Line::from("  none assigned"));
        return Ok(());
    }
    for (name, level) in assigned {
        lines.push(muscle_group_line(&name, level));
    }
    Ok(())
}

/// The display line for one assigned group: `  {name} · {marker}` with only the
/// marker word coloured, at the level's stimulus colour — shaped like a set's
/// difficulty marker in the session summary (`100 kg × 5 · easy`).
fn muscle_group_line(name: &str, level: Level) -> Line<'static> {
    let (label, colour) = match (level.label(), level.colour()) {
        (Some(label), Some(colour)) => (label, colour),
        // The None rows are filtered out before this is called, so a level here
        // always carries a label and a colour.
        _ => unreachable!("assigned groups always have a label and colour"),
    };
    Line::from(vec![
        Span::raw(format!("  {name} · ")),
        Span::styled(label, Style::new().fg(colour)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{apply_schema, seed_muscle_groups};
    use ratatui::style::{Color, Modifier};

    /// An in-memory database with the full schema, the default muscle groups
    /// seeded, and one exercise to attach references to.
    fn test_db() -> (Connection, Exercise) {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply_schema(&mut conn).unwrap();
        seed_muscle_groups(&mut conn).unwrap();
        let exercise = Exercise::create(&conn, "bench press", "flat barbell").unwrap();
        (conn, exercise)
    }

    /// A line's plain text, spans concatenated.
    fn text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn stimulus_pairs_drops_unselected_rows_and_maps_the_rest_in_order() {
        let rows = vec![
            ("Abs".to_string(), Level::None),
            ("Biceps".to_string(), Level::Low),
            ("Front delts".to_string(), Level::Medium),
            ("Pecs".to_string(), Level::High),
            ("Lats".to_string(), Level::None),
        ];
        let pairs = stimulus_pairs(&rows);
        assert_eq!(
            pairs,
            vec![
                ("Biceps".to_string(), Stimulus::Low),
                ("Front delts".to_string(), Stimulus::Medium),
                ("Pecs".to_string(), Stimulus::High),
            ],
            "unselected rows contribute nothing, and row order is preserved"
        );
    }

    #[test]
    fn muscle_group_line_colours_only_the_marker_word() {
        // `  Abs · ` is plain; only the "low" marker carries the level's colour
        // — the difficulty-marker shape from the session summary.
        let line = muscle_group_line("Abs", Level::Low);
        assert_eq!(line.spans.len(), 2);
        assert_eq!(text(&line), "  Abs · low");
        assert_eq!(line.spans[1].style.fg, Some(Color::Green));
        assert_eq!(line.spans[0].style.fg, None, "the name is plain");

        let medium = muscle_group_line("Biceps", Level::Medium);
        assert_eq!(text(&medium), "  Biceps · medium");
        assert_eq!(medium.spans[1].style.fg, Some(Color::Yellow));

        let high = muscle_group_line("Pecs", Level::High);
        assert_eq!(text(&high), "  Pecs · high");
        assert_eq!(high.spans[1].style.fg, Some(Color::Red));
    }

    #[test]
    fn append_muscle_groups_shows_none_assigned_when_no_references() {
        let (conn, exercise) = test_db();
        let mut lines: Vec<Line<'static>> = Vec::new();
        append_muscle_groups(&mut lines, &conn, exercise.id).unwrap();

        assert_eq!(lines.len(), 2, "bold heading plus the none-assigned line");
        assert_eq!(text(&lines[0]), "Muscle groups:");
        assert!(
            lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD),
            "the heading is bold on its own span, the rest of the line plain"
        );
        assert_eq!(text(&lines[1]), "  none assigned");
    }

    #[test]
    fn append_muscle_groups_lists_assigned_groups_in_name_order() {
        let (conn, exercise) = test_db();
        let abs = MuscleGroup::find_by_name(&conn, "Abs").unwrap().unwrap();
        let pecs = MuscleGroup::find_by_name(&conn, "Pecs").unwrap().unwrap();
        MuscleGroupReference::set_for_exercise(
            &conn,
            exercise.id,
            &[(abs.id, Stimulus::Low), (pecs.id, Stimulus::High)],
        )
        .unwrap();

        let mut lines: Vec<Line<'static>> = Vec::new();
        append_muscle_groups(&mut lines, &conn, exercise.id).unwrap();

        assert_eq!(
            lines.len(),
            3,
            "bold heading plus one line per assigned group"
        );
        assert_eq!(text(&lines[1]), "  Abs · low");
        assert_eq!(lines[1].spans[1].style.fg, Some(Color::Green));
        assert_eq!(text(&lines[2]), "  Pecs · high");
        assert_eq!(lines[2].spans[1].style.fg, Some(Color::Red));
    }
}
