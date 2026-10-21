//! Exercise performances — one exercise's performance within a session.

use chrono::{NaiveDate, NaiveTime};
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
    /// The date of the performance.
    pub date: NaiveDate,
    /// The time of day the performance happened.
    pub time_of_day: NaiveTime,
    /// The sets performed, in insertion order. Empty in a freshly created
    /// entity; the loaders ([`ExercisePerformance::by_id`], and the session
    /// tree in [`crate::domain::Session`]) populate it.
    pub sets: Vec<Set>,
}

#[allow(dead_code)] // create is used by the freeform screen; by_id/delete wait on the session list UI
impl ExercisePerformance {
    /// Insert a new performance row and return the persisted entity.
    ///
    /// A foreign key to a session or exercise that does not exist surfaces as
    /// [`DomainError::Sqlite`]; the schema enforces it.
    pub fn create(
        conn: &Connection,
        session_id: i64,
        exercise_id: i64,
        date: NaiveDate,
        time_of_day: NaiveTime,
    ) -> Result<Self, DomainError> {
        conn.execute(
            "INSERT INTO exercise_performances (session_id, exercise_id, date, time_of_day)
             VALUES (?1, ?2, ?3, ?4)",
            params![session_id, exercise_id, date, time_of_day],
        )?;

        Ok(Self {
            id: conn.last_insert_rowid(),
            session_id,
            exercise_id,
            date,
            time_of_day,
            sets: Vec::new(),
        })
    }

    /// Load the performance with the given `id`, with its sets, if one exists.
    ///
    /// Two queries (the row, then its sets) — fine for a single entity; the
    /// session tree loads whole sessions in one join instead.
    pub fn by_id(conn: &Connection, id: i64) -> Result<Option<Self>, DomainError> {
        let mut performance = match conn.query_row(
            "SELECT id, session_id, exercise_id, date, time_of_day
             FROM exercise_performances
             WHERE id = ?1",
            [id],
            |row| {
                Ok(Self {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    exercise_id: row.get(2)?,
                    date: row.get(3)?,
                    time_of_day: row.get(4)?,
                    sets: Vec::new(),
                })
            },
        ) {
            Ok(performance) => performance,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        performance.sets = Set::for_performance(conn, id)?;
        Ok(Some(performance))
    }

    /// Remove this performance and its sets. The sets go with it (`ON DELETE
    /// CASCADE`, migration v3), so one statement is enough. Errors with
    /// [`DomainError::MissingRow`] when the row is already gone.
    pub fn delete(&self, conn: &Connection) -> Result<(), DomainError> {
        let changed = conn.execute("DELETE FROM exercise_performances WHERE id = ?1", [self.id])?;
        if changed == 0 {
            return Err(DomainError::MissingRow {
                kind: "exercise performance",
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
    use crate::domain::{Exercise, Session, Set};
    use chrono::{NaiveDate, NaiveTime};

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

    /// Seed an exercise and a session, returning their ids.
    fn seed(conn: &Connection) -> (i64, i64) {
        let exercise = Exercise::create(conn, "bench press", "flat").unwrap();
        let session = Session::create(conn, date(2026, 10, 20)).unwrap();
        (exercise.id, session.id)
    }

    #[test]
    fn create_inserts_row_and_assigns_id() {
        let conn = test_db();
        let (exercise_id, session_id) = seed(&conn);

        let performance = ExercisePerformance::create(
            &conn,
            session_id,
            exercise_id,
            date(2026, 10, 20),
            time(9, 0),
        )
        .unwrap();

        assert!(performance.id > 0);
        assert_eq!(performance.session_id, session_id);
        assert_eq!(performance.exercise_id, exercise_id);
        assert_eq!(performance.date, date(2026, 10, 20));
        assert_eq!(performance.time_of_day, time(9, 0));
        assert!(performance.sets.is_empty());

        // ...and the row is really there.
        let (login_date, login_time): (NaiveDate, NaiveTime) = conn
            .query_row(
                "SELECT date, time_of_day FROM exercise_performances WHERE id = ?1",
                [performance.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((login_date, login_time), (date(2026, 10, 20), time(9, 0)));
    }

    #[test]
    fn create_rejects_unknown_session() {
        let conn = test_db();
        let (exercise_id, _) = seed(&conn);

        assert!(matches!(
            ExercisePerformance::create(&conn, 999, exercise_id, date(2026, 10, 20), time(8, 0)),
            Err(DomainError::Sqlite(_))
        ));
    }

    #[test]
    fn create_rejects_unknown_exercise() {
        let conn = test_db();
        let (_, session_id) = seed(&conn);

        assert!(matches!(
            ExercisePerformance::create(&conn, session_id, 999, date(2026, 10, 20), time(8, 0)),
            Err(DomainError::Sqlite(_))
        ));
    }

    #[test]
    fn by_id_returns_the_performance_with_its_sets() {
        let conn = test_db();
        let (exercise_id, session_id) = seed(&conn);
        let performance = ExercisePerformance::create(
            &conn,
            session_id,
            exercise_id,
            date(2026, 10, 20),
            time(9, 0),
        )
        .unwrap();
        Set::create(&conn, performance.id, exercise_id, 90.0, 8, false).unwrap();
        Set::create(&conn, performance.id, exercise_id, 95.0, 6, true).unwrap();

        let loaded = ExercisePerformance::by_id(&conn, performance.id)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.session_id, session_id);
        assert_eq!(loaded.exercise_id, exercise_id);
        assert_eq!(loaded.date, date(2026, 10, 20));
        assert_eq!(loaded.sets.len(), 2);
        assert_eq!(loaded.sets[0].reps, 8);
        assert!(!loaded.sets[0].to_failure);
        assert_eq!(loaded.sets[1].weight_kg, 95.0);
        assert!(loaded.sets[1].to_failure);
    }

    #[test]
    fn by_id_missing_returns_none() {
        let conn = test_db();
        assert!(ExercisePerformance::by_id(&conn, 999).unwrap().is_none());
    }

    #[test]
    fn delete_removes_sets_then_the_performance() {
        let conn = test_db();
        let (exercise_id, session_id) = seed(&conn);
        let doomed = ExercisePerformance::create(
            &conn,
            session_id,
            exercise_id,
            date(2026, 10, 20),
            time(9, 0),
        )
        .unwrap();
        Set::create(&conn, doomed.id, exercise_id, 90.0, 8, false).unwrap();
        let survivor = ExercisePerformance::create(
            &conn,
            session_id,
            exercise_id,
            date(2026, 10, 20),
            time(10, 0),
        )
        .unwrap();

        doomed.delete(&conn).unwrap();

        let performance_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM exercise_performances", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(performance_count, 1);
        let set_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(set_count, 0);

        // The other performance — and its (empty) set list — survive.
        let loaded = ExercisePerformance::by_id(&conn, survivor.id)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.id, survivor.id);
        assert!(loaded.sets.is_empty());
    }

    #[test]
    fn delete_missing_row_errors() {
        let conn = test_db();
        let ghost = ExercisePerformance {
            id: 999,
            session_id: 1,
            exercise_id: 1,
            date: date(2026, 10, 20),
            time_of_day: time(8, 0),
            sets: Vec::new(),
        };
        assert!(matches!(
            ghost.delete(&conn),
            Err(DomainError::MissingRow {
                kind: "exercise performance",
                id: 999
            })
        ));
    }
}
