//! Sessions — a workout session, made up of exercise performances.

use chrono::NaiveDate;
use rusqlite::{Connection, params};

use crate::domain::DomainError;
use crate::domain::exercise_performance::ExercisePerformance;
use crate::domain::set::Set;

/// A workout session: the day it started, the day it ended (once it is over),
/// and the performances logged during it. Backed by the `sessions` table
/// (migration v3).
///
/// `start_date` is mandatory; `end_date` stays [`None`] until the session is
/// ended with [`Session::end`].
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// Database row id; assigned by [`Session::create`].
    pub id: i64,
    /// The day the session started.
    pub start_date: NaiveDate,
    /// The day the session ended; set and persisted by [`Session::end`].
    pub end_date: Option<NaiveDate>,
    /// The performances logged during the session, in date/time order. Empty
    /// in a freshly created entity; the loaders populate it.
    pub performances: Vec<ExercisePerformance>,
}

#[allow(dead_code)] // delete waits on the session management UI
impl Session {
    /// Insert a new session row and return the persisted entity, with no
    /// performances and no end date yet.
    ///
    /// The date is passed in rather than read from the clock, keeping the
    /// domain deterministic — the caller (a future UI) supplies "today".
    pub fn create(conn: &Connection, start_date: NaiveDate) -> Result<Self, DomainError> {
        conn.execute(
            "INSERT INTO sessions (start_date) VALUES (?1)",
            params![start_date],
        )?;

        Ok(Self {
            id: conn.last_insert_rowid(),
            start_date,
            end_date: None,
            performances: Vec::new(),
        })
    }

    /// Mark the session as ended on `end_date` (overwriting any earlier end
    /// date) and persist it.
    pub fn end(&mut self, conn: &Connection, end_date: NaiveDate) -> Result<(), DomainError> {
        self.end_date = Some(end_date);
        let changed = conn.execute(
            "UPDATE sessions SET end_date = ?1 WHERE id = ?2",
            params![self.end_date, self.id],
        )?;
        if changed == 0 {
            return Err(DomainError::MissingRow {
                kind: "session",
                id: self.id,
            });
        }
        Ok(())
    }

    /// Load the session with the given `id`, with its whole performances →
    /// sets tree, if one exists.
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>, DomainError> {
        Ok(tree(conn, Some(id))?.into_iter().next())
    }

    /// Load every session with its whole tree, most recent first.
    pub fn all(conn: &Connection) -> Result<Vec<Self>, DomainError> {
        tree(conn, None)
    }

    /// Remove this session and everything under it — its performances and
    /// their sets. Errors with [`DomainError::MissingRow`] when the session is
    /// already gone. Runs in one transaction so a failure leaves the tree
    /// intact.
    pub fn delete(&self, conn: &Connection) -> Result<(), DomainError> {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM sets WHERE exercise_performance_id IN
             (SELECT id FROM exercise_performances WHERE session_id = ?1)",
            [self.id],
        )?;
        tx.execute(
            "DELETE FROM exercise_performances WHERE session_id = ?1",
            [self.id],
        )?;
        let changed = tx.execute("DELETE FROM sessions WHERE id = ?1", [self.id])?;
        if changed == 0 {
            return Err(DomainError::MissingRow {
                kind: "session",
                id: self.id,
            });
        }
        tx.commit()?;
        Ok(())
    }
}

/// Load sessions — optionally just the one with id `only` — with their whole
/// performances → sets tree, in a single query.
///
/// The three-level LEFT JOIN yields one row per set (plus one row per
/// performance with no sets, plus one per session with no performances), and
/// the ORDER BY keeps every child contiguous under its parent, so one pass
/// flattens the rows into the nested structs. [`Session::all`] and
/// [`Session::find_by_id`] both go through here: loading a session's tree
/// costs exactly one query, however large it is.
fn tree(conn: &Connection, only: Option<i64>) -> Result<Vec<Session>, DomainError> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.start_date, s.end_date,
                p.id, p.session_id, p.exercise_id, p.date, p.time_of_day,
                st.id, st.exercise_performance_id, st.exercise_id,
                st.weight_kg, st.reps, st.to_failure
         FROM sessions s
         LEFT JOIN exercise_performances p ON p.session_id = s.id
         LEFT JOIN sets st ON st.exercise_performance_id = p.id
         WHERE (?1 IS NULL OR s.id = ?1)
         ORDER BY s.start_date DESC, s.id DESC, p.date, p.time_of_day, p.id, st.id",
    )?;
    let mut rows = stmt.query(params![only])?;

    let mut sessions: Vec<Session> = Vec::new();
    while let Some(row) = rows.next()? {
        let session_id: i64 = row.get(0)?;
        if sessions.last().is_none_or(|s| s.id != session_id) {
            sessions.push(Session {
                id: session_id,
                start_date: row.get(1)?,
                end_date: row.get(2)?,
                performances: Vec::new(),
            });
        }
        let Some(performance_id) = row.get::<_, Option<i64>>(3)? else {
            continue; // this session has no performances
        };
        let session = sessions.last_mut().unwrap();
        if session
            .performances
            .last()
            .is_none_or(|p| p.id != performance_id)
        {
            session.performances.push(ExercisePerformance {
                id: performance_id,
                session_id: row.get(4)?,
                exercise_id: row.get(5)?,
                date: row.get(6)?,
                time_of_day: row.get(7)?,
                sets: Vec::new(),
            });
        }
        let Some(set_id) = row.get::<_, Option<i64>>(8)? else {
            continue; // this performance has no sets
        };
        let performance = session.performances.last_mut().unwrap();
        performance.sets.push(Set {
            id: set_id,
            exercise_performance_id: row.get(9)?,
            exercise_id: row.get(10)?,
            weight_kg: row.get(11)?,
            reps: row.get(12)?,
            to_failure: row.get(13)?,
        });
    }
    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::apply_schema;
    use crate::domain::{Exercise, ExercisePerformance, Set};
    use chrono::NaiveTime;

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

    #[test]
    fn create_inserts_row_and_leaves_end_date_null() {
        let conn = test_db();
        let session = Session::create(&conn, date(2026, 10, 20)).unwrap();

        assert!(session.id > 0);
        assert_eq!(session.start_date, date(2026, 10, 20));
        assert_eq!(session.end_date, None);
        assert!(session.performances.is_empty());

        // ...and the row is really there.
        let (start_date, end_date): (NaiveDate, Option<NaiveDate>) = conn
            .query_row(
                "SELECT start_date, end_date FROM sessions WHERE id = ?1",
                [session.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(start_date, date(2026, 10, 20));
        assert_eq!(end_date, None);
    }

    #[test]
    fn end_sets_and_persists_end_date() {
        let conn = test_db();
        let mut session = Session::create(&conn, date(2026, 10, 20)).unwrap();

        session.end(&conn, date(2026, 10, 21)).unwrap();

        assert_eq!(session.end_date, Some(date(2026, 10, 21)));
        let stored: Option<NaiveDate> = conn
            .query_row(
                "SELECT end_date FROM sessions WHERE id = ?1",
                [session.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, Some(date(2026, 10, 21)));
    }

    #[test]
    fn end_missing_row_errors() {
        let conn = test_db();
        let mut ghost = Session {
            id: 999,
            start_date: date(2026, 10, 20),
            end_date: None,
            performances: Vec::new(),
        };
        assert!(matches!(
            ghost.end(&conn, date(2026, 10, 21)),
            Err(DomainError::MissingRow {
                kind: "session",
                id: 999
            })
        ));
    }

    #[test]
    fn all_returns_sessions_with_full_performances_sets_tree() {
        let conn = test_db();
        let exercise = Exercise::create(&conn, "bench press", "flat").unwrap();

        // Older session: ended, two performances — the first with three sets,
        // the second (later in the day) with none.
        let mut old = Session::create(&conn, date(2026, 10, 19)).unwrap();
        let p1 =
            ExercisePerformance::create(&conn, old.id, exercise.id, date(2026, 10, 19), time(9, 0))
                .unwrap();
        Set::create(&conn, p1.id, exercise.id, 100.0, 5, true).unwrap();
        Set::create(&conn, p1.id, exercise.id, 102.5, 4, true).unwrap();
        Set::create(&conn, p1.id, exercise.id, 105.0, 3, true).unwrap();
        ExercisePerformance::create(&conn, old.id, exercise.id, date(2026, 10, 19), time(10, 0))
            .unwrap();
        old.end(&conn, date(2026, 10, 19)).unwrap();

        // Newer session: still open, one performance with no sets.
        let fresh = Session::create(&conn, date(2026, 10, 20)).unwrap();
        ExercisePerformance::create(
            &conn,
            fresh.id,
            exercise.id,
            date(2026, 10, 20),
            time(7, 30),
        )
        .unwrap();

        let all = Session::all(&conn).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, fresh.id, "most recent session first");
        assert_eq!(all[1].id, old.id);

        // The fresh session: open, one set-less performance.
        assert_eq!(all[0].end_date, None);
        assert_eq!(all[0].performances.len(), 1);
        assert!(all[0].performances[0].sets.is_empty());

        // The old session: ended, two performances, sets intact and ordered.
        assert_eq!(all[1].end_date, Some(date(2026, 10, 19)));
        assert_eq!(all[1].performances.len(), 2);
        assert_eq!(all[1].performances[0].time_of_day, time(9, 0));
        assert_eq!(all[1].performances[0].sets.len(), 3);
        assert_eq!(all[1].performances[0].sets[0].reps, 5);
        assert_eq!(all[1].performances[0].sets[2].weight_kg, 105.0);
        assert!(all[1].performances[0].sets[1].to_failure);
        assert_eq!(all[1].performances[1].time_of_day, time(10, 0));
        assert!(all[1].performances[1].sets.is_empty());
    }

    #[test]
    fn all_returns_empty_on_empty_db() {
        let conn = test_db();
        assert!(Session::all(&conn).unwrap().is_empty());
    }

    #[test]
    fn find_by_id_returns_the_session_with_its_tree_or_none() {
        let conn = test_db();
        let exercise = Exercise::create(&conn, "bench press", "flat").unwrap();
        let session = Session::create(&conn, date(2026, 10, 20)).unwrap();
        let p = ExercisePerformance::create(
            &conn,
            session.id,
            exercise.id,
            date(2026, 10, 20),
            time(8, 0),
        )
        .unwrap();
        let set = Set::create(&conn, p.id, exercise.id, 100.0, 5, true).unwrap();

        let loaded = Session::find_by_id(&conn, session.id).unwrap().unwrap();
        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.end_date, None);
        assert_eq!(loaded.performances.len(), 1);
        assert_eq!(loaded.performances[0].sets[0].id, set.id);

        assert!(Session::find_by_id(&conn, 999).unwrap().is_none());
    }

    #[test]
    fn find_by_id_handles_a_session_with_no_performances() {
        let conn = test_db();
        let session = Session::create(&conn, date(2026, 10, 20)).unwrap();

        let loaded = Session::find_by_id(&conn, session.id).unwrap().unwrap();
        assert_eq!(loaded.id, session.id);
        assert!(loaded.performances.is_empty());
    }

    #[test]
    fn delete_removes_performances_sets_and_the_session() {
        let conn = test_db();
        let exercise = Exercise::create(&conn, "bench press", "flat").unwrap();
        let doomed = Session::create(&conn, date(2026, 10, 19)).unwrap();
        let p = ExercisePerformance::create(
            &conn,
            doomed.id,
            exercise.id,
            date(2026, 10, 19),
            time(9, 0),
        )
        .unwrap();
        Set::create(&conn, p.id, exercise.id, 100.0, 5, true).unwrap();
        let survivor = Session::create(&conn, date(2026, 10, 20)).unwrap();
        ExercisePerformance::create(
            &conn,
            survivor.id,
            exercise.id,
            date(2026, 10, 20),
            time(8, 0),
        )
        .unwrap();

        doomed.delete(&conn).unwrap();

        // All three levels of `doomed` are gone; a survivor's performance stays.
        let count_sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count_sessions, 1);
        let count_performances: i64 = conn
            .query_row("SELECT COUNT(*) FROM exercise_performances", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count_performances, 1);
        let count_sets: i64 = conn
            .query_row("SELECT COUNT(*) FROM sets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count_sets, 0);

        // ...and the survivor still loads.
        assert_eq!(Session::all(&conn).unwrap()[0].id, survivor.id);
    }

    #[test]
    fn delete_missing_row_errors() {
        let conn = test_db();
        let ghost = Session {
            id: 999,
            start_date: date(2026, 10, 20),
            end_date: None,
            performances: Vec::new(),
        };
        assert!(matches!(
            ghost.delete(&conn),
            Err(DomainError::MissingRow {
                kind: "session",
                id: 999
            })
        ));
    }
}
