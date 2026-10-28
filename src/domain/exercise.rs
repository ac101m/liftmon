//! Exercises — the reference implementation of the domain entity pattern
//! described in `src/domain/mod.rs`.

use rusqlite::{Connection, params};

use crate::domain::DomainError;

/// A single exercise. Backed by the `exercises` table (migration v2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exercise {
    /// Database row id; assigned by [`Exercise::create`] (or a future loader).
    pub id: i64,
    /// Unique exercise name, e.g. "bench press".
    pub name: String,
    /// Free-form description; may be empty.
    pub description: String,
}

impl Exercise {
    /// Insert a new exercise row and return the persisted entity.
    pub fn create(
        conn: &Connection,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let name = name.into();
        let description = description.into();

        match conn.execute(
            "INSERT INTO exercises (name, description) VALUES (?1, ?2)",
            params![name, description],
        ) {
            Ok(_) => {}
            // exercise.name is the only UNIQUE column in exercises, so any
            // unique-constraint violation in this INSERT is a duplicate name.
            Err(err)
                if matches!(
                    &err,
                    rusqlite::Error::SqliteFailure(ffi_err, _)
                        if ffi_err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                ) =>
            {
                return Err(DomainError::NameTaken {
                    kind: "exercise",
                    name,
                });
            }
            Err(err) => return Err(err.into()),
        }

        Ok(Self {
            id: conn.last_insert_rowid(),
            name,
            description,
        })
    }

    /// Write this exercise's current `name` and `description` to its row.
    ///
    /// A rename that collides with another exercise's name (the only UNIQUE
    /// column) surfaces [`DomainError::NameTaken`] instead of the raw SQLite
    /// error, so the editor can log it and re-ask.
    pub fn save(&self, conn: &Connection) -> Result<(), DomainError> {
        let changed = match conn.execute(
            "UPDATE exercises SET name = ?1, description = ?2 WHERE id = ?3",
            params![self.name, self.description, self.id],
        ) {
            Ok(changed) => changed,
            Err(err)
                if matches!(
                    &err,
                    rusqlite::Error::SqliteFailure(ffi_err, _)
                        if ffi_err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                ) =>
            {
                return Err(DomainError::NameTaken {
                    kind: "exercise",
                    name: self.name.clone(),
                });
            }
            Err(err) => return Err(err.into()),
        };
        if changed == 0 {
            return Err(DomainError::MissingRow {
                kind: "exercise",
                id: self.id,
            });
        }
        Ok(())
    }

    /// Remove this exercise's row. Refused with [`DomainError::InUse`] when
    /// other rows still reference it — `exercise_performances` and `sets` hold
    /// the exercise with `ON DELETE RESTRICT` (migration v3), so deleting a
    /// used exercise fails at the constraint level rather than corrupting
    /// history. Errors with [`DomainError::MissingRow`] when the row is already
    /// gone.
    pub fn delete(&self, conn: &Connection) -> Result<(), DomainError> {
        let changed = match conn.execute("DELETE FROM exercises WHERE id = ?1", params![self.id]) {
            Ok(changed) => changed,
            // exercises is the parent of both workout-record tables' exercise
            // FKs, so a used exercise trips ON DELETE RESTRICT — surface it as
            // a friendly InUse, mirroring NameTaken, not a raw database error.
            // (A RESTRICT/NO ACTION violation is raised through the FK-trigger
            // mechanism, so it reports SQLITE_CONSTRAINT_TRIGGER; 787 is the
            // plain foreign-key code.)
            Err(err)
                if matches!(
                    &err,
                    rusqlite::Error::SqliteFailure(ffi_err, _)
                        if matches!(
                            ffi_err.extended_code,
                            rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY
                                | rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER
                        )
                ) =>
            {
                return Err(DomainError::InUse {
                    kind: "exercise",
                    name: self.name.clone(),
                });
            }
            Err(err) => return Err(err.into()),
        };
        if changed == 0 {
            return Err(DomainError::MissingRow {
                kind: "exercise",
                id: self.id,
            });
        }
        Ok(())
    }

    /// How many distinct sessions have a performance of this exercise — the
    /// workout records that stop [`Self::delete`] from working.
    pub fn used_in_sessions(&self, conn: &Connection) -> Result<usize, DomainError> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT session_id) FROM exercise_performances WHERE exercise_id = ?1",
            [self.id],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Load the exercise with the given unique `name`, if one exists.
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Option<Self>, DomainError> {
        match conn.query_row(
            "SELECT id, name, description FROM exercises WHERE name = ?1",
            [name],
            |row| {
                Ok(Self {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                })
            },
        ) {
            Ok(exercise) => Ok(Some(exercise)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Load the exercise with the given `id`, if one exists.
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>, DomainError> {
        match conn.query_row(
            "SELECT id, name, description FROM exercises WHERE id = ?1",
            [id],
            |row| {
                Ok(Self {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                })
            },
        ) {
            Ok(exercise) => Ok(Some(exercise)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Load every exercise, in name order.
    pub fn all(conn: &Connection) -> Result<Vec<Self>, DomainError> {
        let mut stmt = conn.prepare("SELECT id, name, description FROM exercises ORDER BY name")?;
        let rows = stmt.query_map([], |row| {
            Ok(Self {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Load every exercise that trains the given muscle group as a *primary
    /// mover* — its `'high'` reference — which is what the "exercises by
    /// muscle group" browse shows, so the list matches the picker's
    /// per-group count ([`MuscleGroup::with_high_exercises`]: the count and
    /// the browse agree on the same `'high'`-only set). A `'medium'`/`'low'`
    /// reference does not surface an exercise under that group. In name
    /// order; `[]` when no exercise trains the group at `'high'`.
    pub fn for_muscle_group(
        conn: &Connection,
        muscle_group_id: i64,
    ) -> Result<Vec<Self>, DomainError> {
        let mut stmt = conn.prepare(
            "SELECT e.id, e.name, e.description \
             FROM exercises e \
             JOIN muscle_group_reference r ON r.exercise_id = e.id \
             WHERE r.muscle_group_id = ?1 AND r.stimulus_type = 'high' \
             ORDER BY e.name",
        )?;
        let rows = stmt.query_map([muscle_group_id], |row| {
            Ok(Self {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Load every exercise with no `'high'` muscle-group reference — the set
    /// the by-group picker's synthetic "Unassigned" row lists. An exercise
    /// referenced only at `'medium'`/`'low'`, or not at all, is unassigned; one
    /// with any `'high'` reference is not. In name order; `[]` when every
    /// exercise is assigned somewhere at `'high'`.
    pub fn without_high_reference(conn: &Connection) -> Result<Vec<Self>, DomainError> {
        // NOT EXISTS (… stimulus_type = 'high') — the anti-join spelling of "no
        // primary mover", complementing for_muscle_group's INNER JOIN.
        let mut stmt = conn.prepare(
            "SELECT e.id, e.name, e.description \
             FROM exercises e \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM muscle_group_reference r \
                 WHERE r.exercise_id = e.id AND r.stimulus_type = 'high' \
             ) \
             ORDER BY e.name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Self {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{apply_schema, seed_muscle_groups};
    use crate::domain::{
        ExercisePerformance, MuscleGroup, MuscleGroupReference, Session, Stimulus,
    };
    use chrono::{DateTime, NaiveDate, NaiveTime, Utc};

    /// An in-memory database with the full schema applied.
    fn test_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply_schema(&mut conn).unwrap();
        conn
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// A UTC instant from a fixed wall-clock date and time.
    fn dt(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        date(y, m, d)
            .and_time(NaiveTime::from_hms_opt(h, mi, 0).unwrap())
            .and_utc()
    }

    #[test]
    fn create_inserts_row_and_assigns_id() {
        let conn = test_db();
        let exercise = Exercise::create(&conn, "bench press", "barbell chest press").unwrap();

        assert!(exercise.id > 0);
        assert_eq!(exercise.name, "bench press");
        assert_eq!(exercise.description, "barbell chest press");

        // ...and the row is really there.
        let (name, description): (String, String) = conn
            .query_row(
                "SELECT name, description FROM exercises WHERE id = ?1",
                [exercise.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (name.as_str(), description.as_str()),
            ("bench press", "barbell chest press")
        );
    }

    #[test]
    fn create_rejects_duplicate_name() {
        let conn = test_db();
        Exercise::create(&conn, "squat", "heavy").unwrap();
        let err = Exercise::create(&conn, "squat", "a different description").unwrap_err();
        assert!(matches!(
            err,
            DomainError::NameTaken {
                kind: "exercise",
                ..
            }
        ));
    }

    #[test]
    fn save_writes_current_state_to_row() {
        let conn = test_db();
        let mut exercise = Exercise::create(&conn, "bench press", "flat").unwrap();

        exercise.name = "incline bench press".into();
        exercise.description = "bar supported, 30-45 degrees".into();
        exercise.save(&conn).unwrap();

        let (name, description): (String, String) = conn
            .query_row(
                "SELECT name, description FROM exercises WHERE id = ?1",
                [exercise.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (name.as_str(), description.as_str()),
            ("incline bench press", "bar supported, 30-45 degrees")
        );
    }

    #[test]
    fn save_only_touches_own_row() {
        let conn = test_db();
        let mut first = Exercise::create(&conn, "bench press", "flat").unwrap();
        let second = Exercise::create(&conn, "squat", "back squat").unwrap();

        first.name = "close-grip bench press".into();
        first.save(&conn).unwrap();

        let name: String = conn
            .query_row(
                "SELECT name FROM exercises WHERE id = ?1",
                [second.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "squat");
    }

    #[test]
    fn save_missing_row_errors() {
        let conn = test_db();
        let ghost = Exercise {
            id: 999,
            name: "ghost".into(),
            description: String::new(),
        };
        assert!(matches!(
            ghost.save(&conn),
            Err(DomainError::MissingRow {
                kind: "exercise",
                id: 999
            })
        ));
    }

    #[test]
    fn rename_rejects_a_name_already_taken() {
        let conn = test_db();
        Exercise::create(&conn, "bench press", "one").unwrap();
        let mut squat = Exercise::create(&conn, "squat", "two").unwrap();

        squat.name = "bench press".into();
        assert!(matches!(
            squat.save(&conn),
            Err(DomainError::NameTaken {
                kind: "exercise",
                ..
            })
        ));
        // ...and the row is left untouched.
        let name: String = conn
            .query_row(
                "SELECT name FROM exercises WHERE id = ?1",
                [squat.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "squat");
    }

    #[test]
    fn delete_removes_the_row() {
        let conn = test_db();
        let exercise = Exercise::create(&conn, "bench press", "flat").unwrap();

        exercise.delete(&conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM exercises", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn delete_missing_row_errors() {
        let conn = test_db();
        let ghost = Exercise {
            id: 999,
            name: "ghost".into(),
            description: String::new(),
        };
        assert!(matches!(
            ghost.delete(&conn),
            Err(DomainError::MissingRow {
                kind: "exercise",
                id: 999
            })
        ));
    }

    #[test]
    fn delete_refuses_an_exercise_that_is_still_used() {
        let conn = test_db();
        let exercise = Exercise::create(&conn, "bench press", "flat").unwrap();
        let session = Session::create(&conn, dt(2026, 10, 20, 9, 0)).unwrap();
        ExercisePerformance::create(&conn, session.id, exercise.id, dt(2026, 10, 20, 9, 0))
            .unwrap();

        assert!(matches!(
            exercise.delete(&conn),
            Err(DomainError::InUse { kind: "exercise", ref name }) if name == "bench press"
        ));
        // ...and the row is still there to be used.
        assert!(Exercise::find_by_id(&conn, exercise.id).unwrap().is_some());
    }

    #[test]
    fn used_in_sessions_counts_sessions_performing_the_exercise() {
        let conn = test_db();
        let exercise = Exercise::create(&conn, "bench press", "flat").unwrap();
        let unused = Exercise::create(&conn, "deadlift", "conventional").unwrap();

        // One session with a single performance, another with two: two distinct
        // sessions have this exercise logged.
        let first = Session::create(&conn, dt(2026, 10, 19, 9, 0)).unwrap();
        ExercisePerformance::create(&conn, first.id, exercise.id, dt(2026, 10, 19, 9, 0)).unwrap();
        let second = Session::create(&conn, dt(2026, 10, 20, 8, 0)).unwrap();
        ExercisePerformance::create(&conn, second.id, exercise.id, dt(2026, 10, 20, 8, 0)).unwrap();
        ExercisePerformance::create(&conn, second.id, exercise.id, dt(2026, 10, 20, 9, 0)).unwrap();

        assert_eq!(exercise.used_in_sessions(&conn).unwrap(), 2);
        assert_eq!(unused.used_in_sessions(&conn).unwrap(), 0);
    }

    #[test]
    fn find_by_name_returns_the_row_or_none() {
        let conn = test_db();
        Exercise::create(&conn, "bench press", "flat").unwrap();

        let found = Exercise::find_by_name(&conn, "bench press")
            .unwrap()
            .unwrap();
        assert_eq!(found.description, "flat");
        assert!(
            Exercise::find_by_name(&conn, "no such name")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn find_by_id_returns_the_row_or_none() {
        let conn = test_db();
        let exercise = Exercise::create(&conn, "bench press", "flat").unwrap();

        let found = Exercise::find_by_id(&conn, exercise.id).unwrap().unwrap();
        assert_eq!(found.name, "bench press");
        assert_eq!(found.description, "flat");
        assert!(Exercise::find_by_id(&conn, 999).unwrap().is_none());
    }

    #[test]
    fn all_returns_rows_in_name_order() {
        let conn = test_db();
        Exercise::create(&conn, "squat", "back squat").unwrap();
        Exercise::create(&conn, "bench press", "flat bench").unwrap();
        Exercise::create(&conn, "deadlift", "conventional").unwrap();

        let all = Exercise::all(&conn).unwrap();
        assert_eq!(
            all.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["bench press", "deadlift", "squat"],
            "rows come back sorted by name"
        );
        assert_eq!(all[0].description, "flat bench");
        assert!(all.iter().all(|e| e.id > 0));
    }

    #[test]
    fn all_on_empty_db_returns_empty() {
        let conn = test_db();
        assert!(Exercise::all(&conn).unwrap().is_empty());
    }

    #[test]
    fn for_muscle_group_lists_only_the_exercises_that_train_it_at_high() {
        let mut conn = test_db();
        seed_muscle_groups(&mut conn).unwrap();
        let bench = Exercise::create(&conn, "bench press", "flat").unwrap();
        let deadlift = Exercise::create(&conn, "deadlift", "conventional").unwrap();
        let squat = Exercise::create(&conn, "squat", "back squat").unwrap();

        let pecs = MuscleGroup::find_by_name(&conn, "Pecs").unwrap().unwrap();
        let lats = MuscleGroup::find_by_name(&conn, "Lats").unwrap().unwrap();
        let calves = MuscleGroup::find_by_name(&conn, "Calves").unwrap().unwrap();
        // Bench and Deadlift both train Pecs at 'high' (their write order
        // differs from name order, proving the query sorts); Deadlift trains
        // Lats at 'high'. Squat references Pecs at 'medium' and Lats at 'low'
        // — none of it a primary mover — so it must not surface under either
        // group, exactly as [`MuscleGroup::with_high_exercises`]'s count (and
        // the picker's drop-zero rule) would not credit those groups with it.
        MuscleGroupReference::set_for_exercise(&conn, bench.id, &[(pecs.id, Stimulus::High)])
            .unwrap();
        MuscleGroupReference::set_for_exercise(
            &conn,
            deadlift.id,
            &[(pecs.id, Stimulus::High), (lats.id, Stimulus::High)],
        )
        .unwrap();
        MuscleGroupReference::set_for_exercise(
            &conn,
            squat.id,
            &[(pecs.id, Stimulus::Medium), (lats.id, Stimulus::Low)],
        )
        .unwrap();

        let pecs_exercises = Exercise::for_muscle_group(&conn, pecs.id).unwrap();
        assert_eq!(
            pecs_exercises
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["bench press", "deadlift"],
            "only the 'high' exercises come back, in name order, no duplicates"
        );

        let lats_exercises = Exercise::for_muscle_group(&conn, lats.id).unwrap();
        assert_eq!(
            lats_exercises
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["deadlift"]
        );

        assert!(
            Exercise::for_muscle_group(&conn, calves.id)
                .unwrap()
                .is_empty(),
            "a group no exercise trains at 'high' yields an empty list"
        );
    }

    #[test]
    fn without_high_reference_lists_exercises_with_no_high_reference() {
        let mut conn = test_db();
        seed_muscle_groups(&mut conn).unwrap();
        let bench = Exercise::create(&conn, "bench press", "flat").unwrap();
        let deadlift = Exercise::create(&conn, "deadlift", "conventional").unwrap();
        let squat = Exercise::create(&conn, "squat", "back squat").unwrap();
        // Row has no references at all — created purely for its DB row.
        let _ = Exercise::create(&conn, "row", "bent over row").unwrap();

        let pecs = MuscleGroup::find_by_name(&conn, "Pecs").unwrap().unwrap();
        let lats = MuscleGroup::find_by_name(&conn, "Lats").unwrap().unwrap();
        // Bench trains Pecs at 'high' → assigned, so excluded. Deadlift
        // references Pecs at 'medium' and Lats at 'low' — neither is a primary
        // mover — and Row and Squat have no references at all, so all three
        // count as unassigned, even though their write order differs from name
        // order (proving the query sorts).
        MuscleGroupReference::set_for_exercise(&conn, bench.id, &[(pecs.id, Stimulus::High)])
            .unwrap();
        MuscleGroupReference::set_for_exercise(
            &conn,
            deadlift.id,
            &[(pecs.id, Stimulus::Medium), (lats.id, Stimulus::Low)],
        )
        .unwrap();

        let unassigned = Exercise::without_high_reference(&conn).unwrap();
        assert_eq!(
            unassigned
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["deadlift", "row", "squat"],
            "exercises without any 'high' reference come back in name order: \
             deadlift has only medium/low refs, row and squat have none at all, \
             and bench (high) is excluded"
        );

        // Assign squat a 'high' reference → it leaves the unassigned set, and
        // a fresh query reflects the change.
        MuscleGroupReference::set_for_exercise(&conn, squat.id, &[(pecs.id, Stimulus::High)])
            .unwrap();
        let unassigned = Exercise::without_high_reference(&conn).unwrap();
        assert_eq!(
            unassigned
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["deadlift", "row"],
            "assigning a high ref to another exercise shrinks the unassigned set"
        );
    }
}
