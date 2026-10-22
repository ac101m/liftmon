//! Sessions — a workout session, made up of exercise performances.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::domain::DomainError;
use crate::domain::exercise_performance::ExercisePerformance;
use crate::domain::set::Set;

/// A workout session: when it started and the performances logged during it.
/// Backed by the `sessions` table (migration v3).
///
/// Sessions have no terminal state — there is no end date, so a session is
/// always resumable. The resume screen treats every session the same, warning
/// only when its most recent activity is long past ([`crate::ui::menu::session_options`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// Database row id; assigned by [`Session::create`].
    pub id: i64,
    /// The moment the session was created — when the user started it (UTC).
    pub created_at: DateTime<Utc>,
    /// The performances logged during the session, in date/time order. Empty
    /// in a freshly created entity; the loaders populate it.
    pub performances: Vec<ExercisePerformance>,
}

/// The resume screen's lightweight view of a session: its own row's columns
/// plus the moment of its most recent activity — but not the performances/sets
/// tree. Loaded by [`Session::list_info`] as one query that never joins `sets`
/// wholesale, so listing a long history costs one row per session rather than
/// one row per set.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    /// Database row id.
    pub id: i64,
    /// The moment the session was created (UTC).
    pub created_at: DateTime<Utc>,
    /// How many exercises it holds performances for so far — 0 means a session
    /// that has not been used yet (its options screen says "begin" rather than
    /// "resume").
    pub performance_count: i64,
    /// The moment of its most recent activity: the latest set's `created_at`,
    /// or the session's own `created_at` when nothing is logged yet. Never
    /// `None`, thanks to the fallback.
    pub last_activity_at: DateTime<Utc>,
}

#[allow(dead_code)] // delete waits on the session management UI
impl Session {
    /// Insert a new session row and return the persisted entity, with no
    /// performances yet.
    ///
    /// The instant is passed in rather than read from the clock, keeping the
    /// domain deterministic — the caller (the UI) supplies "now".
    pub fn create(conn: &Connection, created_at: DateTime<Utc>) -> Result<Self, DomainError> {
        conn.execute(
            "INSERT INTO sessions (created_at) VALUES (?1)",
            params![created_at],
        )?;

        Ok(Self {
            id: conn.last_insert_rowid(),
            created_at,
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

    /// Load one [`SessionInfo`] per session — its own columns plus how many
    /// performances it holds and the moment of its most recent activity — most
    /// recent first. Unlike [`Session::all`], this returns no performances/sets
    /// at all, so its cost tracks the number of sessions.
    pub fn list_info(conn: &Connection) -> Result<Vec<SessionInfo>, DomainError> {
        info(conn, None)
    }

    /// Load the [`SessionInfo`] for the session with the given `id` — its own
    /// columns plus how many performances it holds and the moment of its most
    /// recent activity — if one exists.
    pub fn info_by_id(conn: &Connection, id: i64) -> Result<Option<SessionInfo>, DomainError> {
        Ok(info(conn, Some(id))?.into_iter().next())
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

/// Load [`SessionInfo`] rows — optionally just the one with id `only`. Each row
/// carries the session's own columns plus its running summary: how many
/// performances it holds (a correlated subquery), and its most recent activity
/// — the latest set's `created_at` across the session (also a correlated
/// subquery, so this stays one row per session, not per set), falling back to
/// the session's own `created_at` when nothing is logged yet.
/// [`Session::list_info`] and [`Session::info_by_id`] both go through here.
fn info(conn: &Connection, only: Option<i64>) -> Result<Vec<SessionInfo>, DomainError> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.created_at,
                (SELECT COUNT(*) FROM exercise_performances p WHERE p.session_id = s.id),
                COALESCE((SELECT MAX(st.created_at)
                          FROM sets st
                          JOIN exercise_performances p ON p.id = st.exercise_performance_id
                          WHERE p.session_id = s.id), s.created_at)
         FROM sessions s
         WHERE (?1 IS NULL OR s.id = ?1)
         ORDER BY s.created_at DESC, s.id DESC",
    )?;
    let rows = stmt.query_map(params![only], |row| {
        Ok(SessionInfo {
            id: row.get(0)?,
            created_at: row.get(1)?,
            performance_count: row.get(2)?,
            // Timestamps are stored as fixed-width UTC text (rusqlite's chrono
            // feature), so MAX(...) picks the most recent set.
            last_activity_at: row.get(3)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
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
        "SELECT s.id, s.created_at,
                p.id, p.session_id, p.exercise_id, p.created_at,
                st.id, st.exercise_performance_id, st.exercise_id,
                st.created_at, st.weight_kg, st.reps, st.to_failure
         FROM sessions s
         LEFT JOIN exercise_performances p ON p.session_id = s.id
         LEFT JOIN sets st ON st.exercise_performance_id = p.id
         WHERE (?1 IS NULL OR s.id = ?1)
         ORDER BY s.created_at DESC, s.id DESC, p.created_at, p.id, st.created_at, st.id",
    )?;
    let mut rows = stmt.query(params![only])?;

    let mut sessions: Vec<Session> = Vec::new();
    while let Some(row) = rows.next()? {
        let session_id: i64 = row.get(0)?;
        if sessions.last().is_none_or(|s| s.id != session_id) {
            sessions.push(Session {
                id: session_id,
                created_at: row.get(1)?,
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
                created_at: row.get(5)?,
                sets: Vec::new(),
            });
        }
        let Some(set_id) = row.get::<_, Option<i64>>(6)? else {
            continue; // this performance has no sets
        };
        let performance = session.performances.last_mut().unwrap();
        performance.sets.push(Set {
            id: set_id,
            exercise_performance_id: row.get(7)?,
            exercise_id: row.get(8)?,
            created_at: row.get(9)?,
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

    #[test]
    fn create_inserts_row_and_leaves_performances_empty() {
        let conn = test_db();
        let session = Session::create(&conn, dt(2026, 10, 20, 9, 0)).unwrap();

        assert!(session.id > 0);
        assert_eq!(session.created_at, dt(2026, 10, 20, 9, 0));
        assert!(session.performances.is_empty());

        // ...and the row is really there.
        let created_at: DateTime<Utc> = conn
            .query_row(
                "SELECT created_at FROM sessions WHERE id = ?1",
                [session.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(created_at, dt(2026, 10, 20, 9, 0));
    }

    #[test]
    fn all_returns_sessions_with_full_performances_sets_tree() {
        let conn = test_db();
        let exercise = Exercise::create(&conn, "bench press", "flat").unwrap();

        // Older session: two performances — the first with three sets, the
        // second (later in the day) with none.
        let old = Session::create(&conn, dt(2026, 10, 19, 8, 0)).unwrap();
        let p1 = ExercisePerformance::create(&conn, old.id, exercise.id, dt(2026, 10, 19, 9, 0))
            .unwrap();
        Set::create(
            &conn,
            p1.id,
            exercise.id,
            100.0,
            5,
            true,
            dt(2026, 10, 19, 9, 5),
        )
        .unwrap();
        Set::create(
            &conn,
            p1.id,
            exercise.id,
            102.5,
            4,
            true,
            dt(2026, 10, 19, 9, 10),
        )
        .unwrap();
        Set::create(
            &conn,
            p1.id,
            exercise.id,
            105.0,
            3,
            true,
            dt(2026, 10, 19, 9, 15),
        )
        .unwrap();
        ExercisePerformance::create(&conn, old.id, exercise.id, dt(2026, 10, 19, 10, 0)).unwrap();

        // Newer session: one performance with no sets.
        let fresh = Session::create(&conn, dt(2026, 10, 20, 7, 0)).unwrap();
        ExercisePerformance::create(&conn, fresh.id, exercise.id, dt(2026, 10, 20, 7, 30)).unwrap();

        let all = Session::all(&conn).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, fresh.id, "most recent session first");
        assert_eq!(all[1].id, old.id);

        // The fresh session: one set-less performance.
        assert_eq!(all[0].performances.len(), 1);
        assert!(all[0].performances[0].sets.is_empty());

        // The old session: two performances, sets intact and ordered.
        assert_eq!(all[1].performances.len(), 2);
        assert_eq!(all[1].performances[0].created_at, dt(2026, 10, 19, 9, 0));
        assert_eq!(all[1].performances[0].sets.len(), 3);
        assert_eq!(all[1].performances[0].sets[0].reps, 5);
        assert_eq!(all[1].performances[0].sets[2].weight_kg, 105.0);
        assert!(all[1].performances[0].sets[1].to_failure);
        assert_eq!(all[1].performances[1].created_at, dt(2026, 10, 19, 10, 0));
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
        let session = Session::create(&conn, dt(2026, 10, 20, 8, 0)).unwrap();
        let p = ExercisePerformance::create(&conn, session.id, exercise.id, dt(2026, 10, 20, 8, 0))
            .unwrap();
        let set = Set::create(
            &conn,
            p.id,
            exercise.id,
            100.0,
            5,
            true,
            dt(2026, 10, 20, 8, 5),
        )
        .unwrap();

        let loaded = Session::find_by_id(&conn, session.id).unwrap().unwrap();
        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.created_at, dt(2026, 10, 20, 8, 0));
        assert_eq!(loaded.performances.len(), 1);
        assert_eq!(loaded.performances[0].sets[0].id, set.id);

        assert!(Session::find_by_id(&conn, 999).unwrap().is_none());
    }

    #[test]
    fn find_by_id_handles_a_session_with_no_performances() {
        let conn = test_db();
        let session = Session::create(&conn, dt(2026, 10, 20, 8, 0)).unwrap();

        let loaded = Session::find_by_id(&conn, session.id).unwrap().unwrap();
        assert_eq!(loaded.id, session.id);
        assert!(loaded.performances.is_empty());
    }

    #[test]
    fn list_info_returns_metadata_and_most_recent_activity_without_sets() {
        let conn = test_db();
        let exercise = Exercise::create(&conn, "bench press", "flat").unwrap();

        // Older session: two performances (09:00 then 10:00), the first laden
        // with sets that the info view must not surface. The last activity is
        // the latest *set* (09:15), not the later set-less performance (10:00).
        let old = Session::create(&conn, dt(2026, 10, 19, 8, 0)).unwrap();
        let p1 = ExercisePerformance::create(&conn, old.id, exercise.id, dt(2026, 10, 19, 9, 0))
            .unwrap();
        Set::create(
            &conn,
            p1.id,
            exercise.id,
            100.0,
            5,
            true,
            dt(2026, 10, 19, 9, 5),
        )
        .unwrap();
        Set::create(
            &conn,
            p1.id,
            exercise.id,
            102.5,
            4,
            true,
            dt(2026, 10, 19, 9, 10),
        )
        .unwrap();
        Set::create(
            &conn,
            p1.id,
            exercise.id,
            105.0,
            3,
            true,
            dt(2026, 10, 19, 9, 15),
        )
        .unwrap();
        ExercisePerformance::create(&conn, old.id, exercise.id, dt(2026, 10, 19, 10, 0)).unwrap();

        // Newer session: one performance, one set.
        let fresh = Session::create(&conn, dt(2026, 10, 20, 7, 0)).unwrap();
        let p2 = ExercisePerformance::create(&conn, fresh.id, exercise.id, dt(2026, 10, 20, 7, 30))
            .unwrap();
        Set::create(
            &conn,
            p2.id,
            exercise.id,
            60.0,
            10,
            false,
            dt(2026, 10, 20, 7, 35),
        )
        .unwrap();

        // A third session with nothing logged at all: its last activity falls
        // back to its own creation moment.
        let empty = Session::create(&conn, dt(2026, 10, 18, 18, 0)).unwrap();

        let infos = Session::list_info(&conn).unwrap();
        assert_eq!(infos.len(), 3);

        // Most recent first.
        assert_eq!(infos[0].id, fresh.id);
        assert_eq!(infos[1].id, old.id);
        assert_eq!(infos[2].id, empty.id);

        // Metadata only — no set data anywhere on the object.
        assert_eq!(infos[2].created_at, dt(2026, 10, 18, 18, 0));

        // The performance count is the running number of exercises performed.
        assert_eq!(infos[0].performance_count, 1); // fresh
        assert_eq!(infos[1].performance_count, 2); // old
        assert_eq!(infos[2].performance_count, 0); // empty

        // The last activity is the latest set's moment (09:15), not the 10:00
        // set-less performance; fresh's is its set at 07:35; the empty session
        // falls back to its creation time.
        assert_eq!(infos[1].last_activity_at, dt(2026, 10, 19, 9, 15));
        assert_eq!(infos[0].last_activity_at, dt(2026, 10, 20, 7, 35));
        assert_eq!(infos[2].last_activity_at, dt(2026, 10, 18, 18, 0));
    }

    #[test]
    fn info_by_id_returns_metadata_for_a_session_or_none() {
        let conn = test_db();
        let exercise = Exercise::create(&conn, "bench press", "flat").unwrap();
        let session = Session::create(&conn, dt(2026, 10, 20, 8, 0)).unwrap();
        let p = ExercisePerformance::create(&conn, session.id, exercise.id, dt(2026, 10, 20, 8, 0))
            .unwrap();
        Set::create(
            &conn,
            p.id,
            exercise.id,
            60.0,
            10,
            false,
            dt(2026, 10, 20, 8, 10),
        )
        .unwrap();
        // A second session with nothing logged has no set to name; its last
        // activity is its own creation moment.
        let empty = Session::create(&conn, dt(2026, 10, 18, 18, 0)).unwrap();

        let info = Session::info_by_id(&conn, session.id).unwrap().unwrap();
        assert_eq!(info.id, session.id);
        assert_eq!(info.created_at, dt(2026, 10, 20, 8, 0));
        assert_eq!(info.performance_count, 1);
        assert_eq!(info.last_activity_at, dt(2026, 10, 20, 8, 10));

        let empty_info = Session::info_by_id(&conn, empty.id).unwrap().unwrap();
        assert_eq!(empty_info.performance_count, 0);
        assert_eq!(empty_info.last_activity_at, dt(2026, 10, 18, 18, 0));

        assert!(Session::info_by_id(&conn, 999).unwrap().is_none());
    }

    #[test]
    fn delete_removes_performances_sets_and_the_session() {
        let conn = test_db();
        let exercise = Exercise::create(&conn, "bench press", "flat").unwrap();
        let doomed = Session::create(&conn, dt(2026, 10, 19, 8, 0)).unwrap();
        let p = ExercisePerformance::create(&conn, doomed.id, exercise.id, dt(2026, 10, 19, 9, 0))
            .unwrap();
        Set::create(
            &conn,
            p.id,
            exercise.id,
            100.0,
            5,
            true,
            dt(2026, 10, 19, 9, 5),
        )
        .unwrap();
        let survivor = Session::create(&conn, dt(2026, 10, 20, 8, 0)).unwrap();
        ExercisePerformance::create(&conn, survivor.id, exercise.id, dt(2026, 10, 20, 8, 0))
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
            created_at: dt(2026, 10, 20, 8, 0),
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
