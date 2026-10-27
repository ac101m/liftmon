//! Muscle groups — the fixed set of muscle groups an exercise can reference,
//! and (once references exist) how strongly each exercise stimulates each.

use rusqlite::Connection;

use crate::domain::DomainError;

/// A single muscle group an exercise can reference. Backed by the
/// `muscle_group` table (migration v5); rows are seeded from
/// `db::muscle_groups::INITIAL_MUSCLE_GROUPS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuscleGroup {
    /// Database row id.
    pub id: i64,
    /// Unique muscle group name, e.g. "Pecs".
    pub name: String,
    /// Free-form description.
    pub description: String,
}

/// A muscle group paired with how many exercises currently reference it at
/// `'high'` stimulus — the row the exercise-selection group pickers show, so
/// each group's count is visible without a per-group query.
pub struct MuscleGroupCount {
    /// The group itself.
    pub group: MuscleGroup,
    /// Exercises with a `'high'` reference to this group.
    pub high_exercise_count: i64,
}

impl MuscleGroup {
    /// Load the muscle group with the given unique `name`, if one exists.
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Option<Self>, DomainError> {
        match conn.query_row(
            "SELECT id, name, description FROM muscle_group WHERE name = ?1",
            [name],
            |row| {
                Ok(Self {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                })
            },
        ) {
            Ok(group) => Ok(Some(group)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Load the muscle group with the given `id`, if one exists. Not used by
    /// the group screens, which look groups up by name — kept for a future
    /// caller, and exercised by its test.
    #[allow(dead_code)]
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>, DomainError> {
        match conn.query_row(
            "SELECT id, name, description FROM muscle_group WHERE id = ?1",
            [id],
            |row| {
                Ok(Self {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                })
            },
        ) {
            Ok(group) => Ok(Some(group)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Load every muscle group, in name order.
    pub fn all(conn: &Connection) -> Result<Vec<Self>, DomainError> {
        let mut stmt =
            conn.prepare("SELECT id, name, description FROM muscle_group ORDER BY name")?;
        let rows = stmt.query_map([], |row| {
            Ok(Self {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Load every muscle group that at least one exercise trains as a primary
    /// mover — in name order, with how many exercises reference it at `'high'`
    /// stimulus. The join itself drops groups with no `'high'` reference (a
    /// group referenced only at `'medium'`/`'low'`, or not at all, does not
    /// appear), and the count is a live aggregate over
    /// `muscle_group_reference` — never stored. The exercise-selection group
    /// pickers show exactly these rows (the "Muscle groups" main-menu picker
    /// shows plain names).
    pub fn with_high_exercises(conn: &Connection) -> Result<Vec<MuscleGroupCount>, DomainError> {
        // The INNER JOIN (with the stimulus filter in the ON) keeps exactly the
        // groups a 'high' reference points to; COUNT of the joined exercise_id
        // counts those references. One aggregate over the reference table, in
        // the same single pass as all pickers.
        let mut stmt = conn.prepare(
            "SELECT mg.id, mg.name, mg.description, COUNT(mgr.exercise_id) \
             FROM muscle_group mg \
             JOIN muscle_group_reference mgr \
                  ON mgr.muscle_group_id = mg.id AND mgr.stimulus_type = 'high' \
             GROUP BY mg.id, mg.name, mg.description \
             ORDER BY mg.name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(MuscleGroupCount {
                group: Self {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                },
                high_exercise_count: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{apply_schema, seed_muscle_groups};

    /// An in-memory database with the full schema applied and the default
    /// muscle groups seeded.
    fn test_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply_schema(&mut conn).unwrap();
        seed_muscle_groups(&mut conn).unwrap();
        conn
    }

    #[test]
    fn all_loads_every_default_muscle_group_in_name_order() {
        let conn = test_db();

        let all = MuscleGroup::all(&conn).unwrap();
        assert_eq!(all.len(), 16);
        assert_eq!(
            all.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
            [
                "Abs",
                "Biceps",
                "Calves",
                "Forearms",
                "Front delts",
                "Glutes",
                "Hamstrings",
                "Lats",
                "Obliques",
                "Pecs",
                "Quads",
                "Rear delts",
                "Side delts",
                "Spinal erectors",
                "Traps",
                "Triceps",
            ],
            "every default group comes back, sorted by name"
        );
        assert!(
            all.iter().all(|g| !g.description.is_empty()),
            "every default group carries a filled-in description"
        );
        assert!(all.iter().all(|g| g.id > 0));
    }

    #[test]
    fn find_by_name_returns_the_group_or_none() {
        let conn = test_db();

        let found = MuscleGroup::find_by_name(&conn, "Pecs").unwrap().unwrap();
        assert_eq!(found.name, "Pecs");
        assert!(
            MuscleGroup::find_by_name(&conn, "no such group")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn find_by_id_returns_the_group_or_none() {
        let conn = test_db();

        let first = MuscleGroup::all(&conn).unwrap();
        let found = MuscleGroup::find_by_id(&conn, first[0].id)
            .unwrap()
            .unwrap();
        assert_eq!(found, first[0]);
        assert!(MuscleGroup::find_by_id(&conn, 999).unwrap().is_none());
    }

    #[test]
    fn with_high_exercises_lists_only_primary_mover_groups_with_their_counts() {
        let conn = test_db();

        // Two exercises; the first trains Abs at 'high' and Biceps at 'medium'
        // (which must be ignored), the second trains Abs at 'high' and Biceps
        // at 'high' too — so Abs counts 2 and Biceps 1, and the other fourteen
        // groups (no high reference at all) do not appear.
        conn.execute(
            "INSERT INTO exercises (name, description) VALUES ('bench', ''), ('squat', '')",
            [],
        )
        .unwrap();
        let abs_id: i64 = conn
            .query_row(
                "SELECT id FROM muscle_group WHERE name = ?1",
                ["Abs"],
                |r| r.get(0),
            )
            .unwrap();
        let biceps_id: i64 = conn
            .query_row(
                "SELECT id FROM muscle_group WHERE name = ?1",
                ["Biceps"],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO muscle_group_reference (exercise_id, muscle_group_id, stimulus_type) \
             VALUES (1, ?1, 'high'), (1, ?2, 'medium'), (2, ?1, 'high'), (2, ?2, 'high')",
            [abs_id, biceps_id],
        )
        .unwrap();

        let counts = MuscleGroup::with_high_exercises(&conn).unwrap();
        let pairs: Vec<(&str, i64)> = counts
            .iter()
            .map(|c| (c.group.name.as_str(), c.high_exercise_count))
            .collect();
        assert_eq!(
            pairs,
            [("Abs", 2), ("Biceps", 1)],
            "only groups with high references appear, in name order, counting only the high refs"
        );
    }
}
