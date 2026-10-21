//! Sessions — a workout session, made up of exercise performances.

use chrono::{NaiveDate, NaiveDateTime};
use rusqlite::{Connection, params};

use crate::domain::DomainError;
use crate::domain::exercise_performance::ExercisePerformance;
use crate::domain::set::Set;

/// A workout session: the day it started and the performances logged during
/// it. Backed by the `sessions` table (migration v3).
///
/// Sessions have no terminal state — there is no end date, so a session is
/// always resumable. The resume screen treats every session the same, warning
/// only when its most recent exercise is long past ([`crate::ui::menu::resume_session`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// Database row id; assigned by [`Session::create`].
    pub id: i64,
    /// The day the session started.
    pub start_date: NaiveDate,
    /// The performances logged during the session, in date/time order. Empty
    /// in a freshly created entity; the loaders populate it.
    pub performances: Vec<ExercisePerformance>,
}

/// The resume screen's lightweight view of a session: its own row's columns
/// plus the moment of its most recent exercise — but not the performances/sets
/// tree. Loaded by [`Session::list_info`] as one grouped query that never
/// joins `sets`, so listing a long history costs one row per session rather
/// than one row per set.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    /// Database row id.
    pub id: i64,
    /// The day the session started.
    pub start_date: NaiveDate,
    /// The date and time of its most recent exercise. [`None`] for a session
    /// with nothing logged yet.
    pub last_exercise_at: Option<NaiveDateTime>,
}

#[allow(dead_code)] // delete waits on the session management UI
impl Session {
    /// Insert a new session row and return the persisted entity, with no
    /// performances yet.
    ///
    /// The date is passed in rather than read from the clock, keeping the
    /// domain deterministic — the caller (the UI) supplies "today".
    pub fn create(conn: &Connection, start_date: NaiveDate) -> Result<Self, DomainError> {
        conn.execute(
            "INSERT INTO sessions (start_date) VALUES (?1)",
            params![start_date],
        )?;

        Ok(Self {
            id: conn.last_insert_rowid(),
            start_date,
            performances: Vec::new(),
        })
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

    /// Load one [`SessionInfo`] per session — its own columns plus the
    /// date/time of its most recent exercise — most recent first. Unlike
    /// [`Session::all`], this never joins `sets` (and returns no set data at
    /// all), so its cost tracks the number of sessions.
    pub fn list_info(conn: &Connection) -> Result<Vec<SessionInfo>, DomainError> {
        let mut stmt = conn.prepare(
            "SELECT s.id, s.start_date,
                    MAX(p.date || 'T' || p.time_of_day)
             FROM sessions s
             LEFT JOIN exercise_performances p ON p.session_id = s.id
             GROUP BY s.id
             ORDER BY s.start_date DESC, s.id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let last_at: Option<String> = row.get(2)?;
            Ok(SessionInfo {
                id: row.get(0)?,
                start_date: row.get(1)?,
                // The date/time stamps are stored as fixed-width ISO text
                // (with a fractional-seconds suffix when non-zero), so the
                // concatenation sorts chronologically and MAX(...) picks the
                // most recent exercise.
                last_exercise_at: last_at.as_deref().and_then(|stamp| {
                    NaiveDateTime::parse_from_str(stamp, "%Y-%m-%dT%H:%M:%S%.f").ok()
                }),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Remove this session and everything under it — its performances and
    /// their sets. The schema deletes the sub-tree (`ON DELETE CASCADE`, so
    /// performances and sets go with their session, migration v3), which makes
    /// one statement enough. Errors with [`DomainError::MissingRow`] when the
    /// session is already gone.
    pub fn delete(&self, conn: &Connection) -> Result<(), DomainError> {
        let changed = conn.execute("DELETE FROM sessions WHERE id = ?1", [self.id])?;
        if changed == 0 {
            return Err(DomainError::MissingRow {
                kind: "session",
                id: self.id,
            });
        }
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
        "SELECT s.id, s.start_date,
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
                performances: Vec::new(),
            });
        }
        let Some(performance_id) = row.get::<_, Option<i64>>(2)? else {
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
                session_id: row.get(3)?,
                exercise_id: row.get(4)?,
                date: row.get(5)?,
                time_of_day: row.get(6)?,
                sets: Vec::new(),
            });
        }
        let Some(set_id) = row.get::<_, Option<i64>>(7)? else {
            continue; // this performance has no sets
        };
        let performance = session.performances.last_mut().unwrap();
        performance.sets.push(Set {
            id: set_id,
            exercise_performance_id: row.get(8)?,
            exercise_id: row.get(9)?,
            weight_kg: row.get(10)?,
            reps: row.get(11)?,
            to_failure: row.get(12)?,
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
    fn create_inserts_row_and_leaves_performances_empty() {
        let conn = test_db();
        let session = Session::create(&conn, date(2026, 10, 20)).unwrap();

        assert!(session.id > 0);
        assert_eq!(session.start_date, date(2026, 10, 20));
        assert!(session.performances.is_empty());

        // ...and the row is really there.
        let start_date: NaiveDate = conn
            .query_row(
                "SELECT start_date FROM sessions WHERE id = ?1",
                [session.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(start_date, date(2026, 10, 20));
    }

    #[test]
    fn all_returns_sessions_with_full_performances_sets_tree() {
        let conn = test_db();
        let exercise = Exercise::create(&conn, "bench press", "flat").unwrap();

        // Older session: two performances — the first with three sets, the
        // second (later in the day) with none.
        let old = Session::create(&conn, date(2026, 10, 19)).unwrap();
        let p1 =
            ExercisePerformance::create(&conn, old.id, exercise.id, date(2026, 10, 19), time(9, 0))
                .unwrap();
        Set::create(&conn, p1.id, exercise.id, 100.0, 5, true).unwrap();
        Set::create(&conn, p1.id, exercise.id, 102.5, 4, true).unwrap();
        Set::create(&conn, p1.id, exercise.id, 105.0, 3, true).unwrap();
        ExercisePerformance::create(&conn, old.id, exercise.id, date(2026, 10, 19), time(10, 0))
            .unwrap();

        // Newer session: one performance with no sets.
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

        // The fresh session: one set-less performance.
        assert_eq!(all[0].performances.len(), 1);
        assert!(all[0].performances[0].sets.is_empty());

        // The old session: two performances, sets intact and ordered.
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
        assert_eq!(loaded.start_date, date(2026, 10, 20));
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
    fn list_info_returns_metadata_and_most_recent_exercise_without_sets() {
        let conn = test_db();
        let exercise = Exercise::create(&conn, "bench press", "flat").unwrap();

        // Older session: two performances (09:00 then 10:00), the first laden
        // with sets that the info view must not surface.
        let old = Session::create(&conn, date(2026, 10, 19)).unwrap();
        let p1 =
            ExercisePerformance::create(&conn, old.id, exercise.id, date(2026, 10, 19), time(9, 0))
                .unwrap();
        Set::create(&conn, p1.id, exercise.id, 100.0, 5, true).unwrap();
        Set::create(&conn, p1.id, exercise.id, 102.5, 4, true).unwrap();
        Set::create(&conn, p1.id, exercise.id, 105.0, 3, true).unwrap();
        ExercisePerformance::create(&conn, old.id, exercise.id, date(2026, 10, 19), time(10, 0))
            .unwrap();

        // Newer session: one performance, one set.
        let fresh = Session::create(&conn, date(2026, 10, 20)).unwrap();
        let p2 = ExercisePerformance::create(
            &conn,
            fresh.id,
            exercise.id,
            date(2026, 10, 20),
            time(7, 30),
        )
        .unwrap();
        Set::create(&conn, p2.id, exercise.id, 60.0, 10, false).unwrap();

        // A third session with nothing logged at all.
        let empty = Session::create(&conn, date(2026, 10, 18)).unwrap();

        let infos = Session::list_info(&conn).unwrap();
        assert_eq!(infos.len(), 3);

        // Most recent first.
        assert_eq!(infos[0].id, fresh.id);
        assert_eq!(infos[1].id, old.id);
        assert_eq!(infos[2].id, empty.id);

        // Metadata only — no set data anywhere on the object.
        assert_eq!(infos[2].start_date, date(2026, 10, 18));

        // The most recent exercise's moment — the later 10:00, not 09:00.
        assert_eq!(
            infos[1].last_exercise_at,
            Some(date(2026, 10, 19).and_time(time(10, 0)))
        );
        assert_eq!(
            infos[0].last_exercise_at,
            Some(date(2026, 10, 20).and_time(time(7, 30)))
        );
        assert_eq!(infos[2].last_exercise_at, None);
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
