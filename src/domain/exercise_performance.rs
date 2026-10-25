//! Exercise performances — one exercise's performance within a session.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::domain::DomainError;
use crate::domain::set::Set;

/// One exercise's performance within a [`crate::domain::Session`]: when it was
/// done, and the sets performed. Backed by the `exercise_performances` table
/// (migration v3).
#[derive(Debug, Clone, PartialEq)]
pub struct ExercisePerformance {
    /// Database row id; assigned by [`ExercisePerformance::create`].
    pub id: i64,
    /// The session this performance belongs to.
    pub session_id: i64,
    /// The exercise performed.
    pub exercise_id: i64,
    /// The moment of the performance's first set — a performance is created
    /// only when its first set is logged (UTC).
    pub created_at: DateTime<Utc>,
    /// The sets performed, in insertion order. Empty in a freshly created
    /// entity; only the session tree loader in [`crate::domain::Session`]
    /// populates it.
    pub sets: Vec<Set>,
}

impl ExercisePerformance {
    /// Insert a new performance row and return the persisted entity.
    ///
    /// The `created_at` instant is the moment of the performance's first set —
    /// the caller creates a performance only when that set is completed, never
    /// for an exercise that was merely selected, so no empty performance is
    /// left behind. A foreign key to a session or exercise that does not exist
    /// surfaces as [`DomainError::Sqlite`]; the schema enforces it.
    pub fn create(
        conn: &Connection,
        session_id: i64,
        exercise_id: i64,
        created_at: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        conn.execute(
            "INSERT INTO exercise_performances (session_id, exercise_id, created_at)
             VALUES (?1, ?2, ?3)",
            params![session_id, exercise_id, created_at],
        )?;

        Ok(Self {
            id: conn.last_insert_rowid(),
            session_id,
            exercise_id,
            created_at,
            sets: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::apply_schema;
    use crate::domain::{Exercise, Session};
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

    /// Seed an exercise and a session, returning their ids.
    fn seed(conn: &Connection) -> (i64, i64) {
        let exercise = Exercise::create(conn, "bench press", "flat").unwrap();
        let session = Session::create(conn, dt(2026, 10, 20, 8, 0)).unwrap();
        (exercise.id, session.id)
    }

    #[test]
    fn create_inserts_row_and_assigns_id() {
        let conn = test_db();
        let (exercise_id, session_id) = seed(&conn);

        let performance =
            ExercisePerformance::create(&conn, session_id, exercise_id, dt(2026, 10, 20, 9, 0))
                .unwrap();

        assert!(performance.id > 0);
        assert_eq!(performance.session_id, session_id);
        assert_eq!(performance.exercise_id, exercise_id);
        assert_eq!(performance.created_at, dt(2026, 10, 20, 9, 0));
        assert!(performance.sets.is_empty());

        // ...and the row is really there, as a UTC datetime string.
        let created_at: DateTime<Utc> = conn
            .query_row(
                "SELECT created_at FROM exercise_performances WHERE id = ?1",
                [performance.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(created_at, dt(2026, 10, 20, 9, 0));
    }

    #[test]
    fn create_rejects_unknown_session() {
        let conn = test_db();
        let (exercise_id, _) = seed(&conn);

        assert!(matches!(
            ExercisePerformance::create(&conn, 999, exercise_id, dt(2026, 10, 20, 8, 0)),
            Err(DomainError::Sqlite(_))
        ));
    }

    #[test]
    fn create_rejects_unknown_exercise() {
        let conn = test_db();
        let (_, session_id) = seed(&conn);

        assert!(matches!(
            ExercisePerformance::create(&conn, session_id, 999, dt(2026, 10, 20, 8, 0)),
            Err(DomainError::Sqlite(_))
        ));
    }
}
