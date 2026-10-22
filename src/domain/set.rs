//! Sets — one set within an exercise performance.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::domain::DomainError;

/// A single set within an [`crate::domain::ExercisePerformance`]: how much was
/// lifted, how many reps, whether it was a hard set, and when it was done.
/// Backed by the `sets` table (migration v3).
#[derive(Debug, Clone, PartialEq)]
pub struct Set {
    /// Database row id; assigned by [`Set::create`].
    pub id: i64,
    /// The exercise performance this set belongs to.
    pub exercise_performance_id: i64,
    /// The exercise this set is for. Reachable through the performance too,
    /// but kept as an explicit column (the schema requires it).
    pub exercise_id: i64,
    /// The moment the set was logged (UTC).
    pub created_at: DateTime<Utc>,
    /// The weight lifted, in kilograms.
    pub weight_kg: f64,
    /// The number of repetitions performed in this set.
    pub reps: u32,
    /// Whether the set was taken to (or near) failure — i.e. a hard set.
    pub to_failure: bool,
}

#[allow(dead_code)] // create is used by the freeform screen; for_performance/delete wait on the session list UI
impl Set {
    /// Insert a new set row and return the persisted entity.
    ///
    /// A foreign key to a performance or exercise that does not exist surfaces
    /// as [`DomainError::Sqlite`]; the schema enforces it.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        conn: &Connection,
        exercise_performance_id: i64,
        exercise_id: i64,
        weight_kg: f64,
        reps: u32,
        to_failure: bool,
        created_at: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        conn.execute(
            "INSERT INTO sets
             (exercise_performance_id, exercise_id, created_at, weight_kg, reps, to_failure)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                exercise_performance_id,
                exercise_id,
                created_at,
                weight_kg,
                reps,
                to_failure
            ],
        )?;

        Ok(Self {
            id: conn.last_insert_rowid(),
            exercise_performance_id,
            exercise_id,
            created_at,
            weight_kg,
            reps,
            to_failure,
        })
    }

    /// Load every set of the given performance, in insertion order.
    pub fn for_performance(
        conn: &Connection,
        exercise_performance_id: i64,
    ) -> Result<Vec<Self>, DomainError> {
        let mut stmt = conn.prepare(
            "SELECT id, exercise_performance_id, exercise_id, created_at, weight_kg, reps, to_failure
             FROM sets
             WHERE exercise_performance_id = ?1
             ORDER BY id",
        )?;
        let rows = stmt.query_map(params![exercise_performance_id], |row| {
            Ok(Self {
                id: row.get(0)?,
                exercise_performance_id: row.get(1)?,
                exercise_id: row.get(2)?,
                created_at: row.get(3)?,
                weight_kg: row.get(4)?,
                reps: row.get(5)?,
                to_failure: row.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Remove this set's row. Errors with [`DomainError::MissingRow`] when the
    /// row is already gone.
    pub fn delete(&self, conn: &Connection) -> Result<(), DomainError> {
        let changed = conn.execute("DELETE FROM sets WHERE id = ?1", params![self.id])?;
        if changed == 0 {
            return Err(DomainError::MissingRow {
                kind: "set",
                id: self.id,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::apply_schema;
    use chrono::{DateTime, NaiveDate, NaiveTime, Utc};

    /// An in-memory database with the full schema applied and foreign keys
    /// enforced, mirroring [`crate::db::Database`].
    fn test_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply_schema(&mut conn).unwrap();
        conn
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn time(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    /// A UTC instant from a fixed wall-clock date and time.
    fn dt(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        date(y, m, d).and_time(time(h, mi)).and_utc()
    }

    /// Seed an exercise and return its id.
    fn seed_exercise(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO exercises (name, description) VALUES ('bench press', '')",
            [],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// Seed a session and return its id.
    fn seed_session(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO sessions (created_at) VALUES ('2026-10-20 09:00:00+00:00')",
            [],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// Seed an exercise, a session and a performance, and return the
    /// performance's id.
    fn seed_performance(conn: &Connection) -> i64 {
        let exercise_id = seed_exercise(conn);
        let session_id = seed_session(conn);
        conn.execute(
            "INSERT INTO exercise_performances (session_id, exercise_id, created_at)
             VALUES (?1, ?2, '2026-10-20 09:00:00+00:00')",
            params![session_id, exercise_id],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn create_inserts_row_and_assigns_id() {
        let conn = test_db();
        let performance_id = seed_performance(&conn);
        let exercise_id = 1;

        let set = Set::create(
            &conn,
            performance_id,
            exercise_id,
            100.0,
            5,
            true,
            dt(2026, 10, 20, 9, 15),
        )
        .unwrap();

        assert!(set.id > 0);
        assert_eq!(set.exercise_performance_id, performance_id);
        assert_eq!(set.exercise_id, exercise_id);
        assert_eq!(set.created_at, dt(2026, 10, 20, 9, 15));
        assert_eq!(set.weight_kg, 100.0);
        assert_eq!(set.reps, 5);
        assert!(set.to_failure);

        // ...and the row is really there, the boolean stored as 0/1.
        let (created_at, weight_kg, reps, to_failure): (String, f64, i64, i64) = conn
            .query_row(
                "SELECT created_at, weight_kg, reps, to_failure FROM sets WHERE id = ?1",
                [set.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (created_at.as_str(), weight_kg, reps, to_failure),
            ("2026-10-20 09:15:00+00:00", 100.0, 5, 1)
        );
    }

    #[test]
    fn create_links_the_performance() {
        let conn = test_db();
        let performance_id = seed_performance(&conn);

        // A second performance exists too, so the link is unambiguous.
        let other = Set::create(
            &conn,
            performance_id,
            1,
            60.0,
            10,
            false,
            dt(2026, 10, 20, 9, 20),
        )
        .unwrap();

        let performance_id_in_db: i64 = conn
            .query_row(
                "SELECT exercise_performance_id FROM sets WHERE id = ?1",
                [other.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(performance_id_in_db, performance_id);
    }

    #[test]
    fn create_rejects_unknown_performance() {
        let conn = test_db();
        let exercise_id = seed_exercise(&conn);
        seed_session(&conn); // a session exists; no performance does

        assert!(matches!(
            Set::create(
                &conn,
                999,
                exercise_id,
                50.0,
                5,
                false,
                dt(2026, 10, 20, 9, 30)
            )
            .map(|_| true),
            Err(DomainError::Sqlite(_))
        ));
    }

    #[test]
    fn create_rejects_unknown_exercise() {
        let conn = test_db();
        let performance_id = seed_performance(&conn);

        assert!(matches!(
            Set::create(
                &conn,
                performance_id,
                999,
                50.0,
                5,
                false,
                dt(2026, 10, 20, 9, 30)
            )
            .map(|_| true),
            Err(DomainError::Sqlite(_))
        ));
    }

    #[test]
    fn for_performance_returns_ordered_sets() {
        let conn = test_db();
        let performance_id = seed_performance(&conn);

        // Created in a scrambled order (second before first); rows must come
        // back keyed to the id SQLite assigned at insertion, i.e. creation
        // order.
        let second = Set::create(
            &conn,
            performance_id,
            1,
            95.0,
            6,
            true,
            dt(2026, 10, 20, 9, 30),
        )
        .unwrap();
        let first = Set::create(
            &conn,
            performance_id,
            1,
            90.0,
            8,
            false,
            dt(2026, 10, 20, 9, 25),
        )
        .unwrap();
        let third = Set::create(
            &conn,
            performance_id,
            1,
            100.0,
            5,
            false,
            dt(2026, 10, 20, 9, 35),
        )
        .unwrap();

        let sets = Set::for_performance(&conn, performance_id).unwrap();
        assert_eq!(
            sets.iter().map(|s| s.id).collect::<Vec<_>>(),
            [second.id, first.id, third.id]
        );
        assert_eq!(sets[0].created_at, dt(2026, 10, 20, 9, 30));
        assert!(sets[0].to_failure);
        assert_eq!(sets[1].reps, 8);
        assert_eq!(sets[2].weight_kg, 100.0);
    }

    #[test]
    fn for_performance_returns_empty_for_unknown() {
        let conn = test_db();
        assert!(Set::for_performance(&conn, 999).unwrap().is_empty());
    }

    #[test]
    fn delete_removes_the_row() {
        let conn = test_db();
        let performance_id = seed_performance(&conn);
        let set = Set::create(
            &conn,
            performance_id,
            1,
            100.0,
            5,
            false,
            dt(2026, 10, 20, 9, 30),
        )
        .unwrap();

        set.delete(&conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn delete_missing_row_errors() {
        let conn = test_db();
        let ghost = Set {
            id: 999,
            exercise_performance_id: 1,
            exercise_id: 1,
            created_at: dt(2026, 10, 20, 9, 0),
            weight_kg: 0.0,
            reps: 0,
            to_failure: false,
        };
        assert!(matches!(
            ghost.delete(&conn),
            Err(DomainError::MissingRow {
                kind: "set",
                id: 999
            })
        ));
    }
}
