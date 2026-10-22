//! Sets — one set within an exercise performance.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::domain::DomainError;

/// A single set within an [`crate::domain::ExercisePerformance`]: how much was
/// lifted, how many reps, whether it was a hard set, the Epley e1RM it
/// produces, and when it was done. Backed by the `sets` table (migration v3
/// plus the v4 `epley_e1rm` column).
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
    /// The set's Epley estimated one-rep max (`weight_kg × (1 + reps / 30)`):
    /// the internal scalar that ranks sets and decides personal bests. Stored
    /// so ordering needs no recompute, but the UI always surfaces weight and
    /// reps, never this number.
    #[allow(dead_code)] // set, read back and ORDER BY'd by SQL; the UI shows weight and reps
    pub epley_e1rm: f64,
}

/// The Epley formula's reps divisor: `e1rm = weight_kg × (1 + reps / 30)`.
/// The same `30.0` appears inline in the migration's backfill SQL — SQLite
/// can't call Rust — so the two must stay in sync.
const EPLEY_E1RM_REPS_SCALE: f64 = 30.0;

/// The Epley estimated one-rep max for a set of `weight_kg` × `reps`.
fn epley_e1rm(weight_kg: f64, reps: u32) -> f64 {
    weight_kg * (1.0 + reps as f64 / EPLEY_E1RM_REPS_SCALE)
}

#[allow(dead_code)] // create/best_set are used by the screens; for_performance/delete wait on the session list UI
impl Set {
    /// Insert a new set row, and return the persisted entity together with
    /// whether it is a personal best: its Epley e1RM strictly exceeds the best
    /// of every prior set of the same exercise. An exercise's first set always
    /// is one (nothing precedes it); an equal e1RM is not (strictly `>`).
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
    ) -> Result<(Self, bool), DomainError> {
        // The e1RM decides the flag and is stored on the row, so ranking and
        // personal-best history need no recompute later.
        let epley = epley_e1rm(weight_kg, reps);
        // Every already-existing set is prior to this insert, so the best of
        // the exercise so far is one MAX query; COALESCE makes a never-
        // performed exercise compare against 0, so its first set wins.
        let prior_best: f64 = conn.query_row(
            "SELECT COALESCE(MAX(epley_e1rm), 0.0) FROM sets WHERE exercise_id = ?1",
            [exercise_id],
            |row| row.get(0),
        )?;
        let is_personal_best = epley > prior_best;
        conn.execute(
            "INSERT INTO sets
             (exercise_performance_id, exercise_id, created_at, weight_kg, reps, to_failure, epley_e1rm)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                exercise_performance_id,
                exercise_id,
                created_at,
                weight_kg,
                reps,
                to_failure,
                epley
            ],
        )?;

        Ok((
            Self {
                id: conn.last_insert_rowid(),
                exercise_performance_id,
                exercise_id,
                created_at,
                weight_kg,
                reps,
                to_failure,
                epley_e1rm: epley,
            },
            is_personal_best,
        ))
    }

    /// Load every set of the given performance, in insertion order.
    pub fn for_performance(
        conn: &Connection,
        exercise_performance_id: i64,
    ) -> Result<Vec<Self>, DomainError> {
        let mut stmt = conn.prepare(
            "SELECT id, exercise_performance_id, exercise_id, created_at, weight_kg, reps, to_failure, epley_e1rm
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
                epley_e1rm: row.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// The current best set of an exercise — the one with the highest Epley
    /// e1RM — with a deterministic tie-break when two sets rank equal (equal
    /// value → heavier, then newer). [`None`] when the exercise has never been
    /// performed.
    pub fn best_set(conn: &Connection, exercise_id: i64) -> Result<Option<Self>, DomainError> {
        match conn.query_row(
            "SELECT id, exercise_performance_id, exercise_id, created_at, weight_kg, reps, to_failure, epley_e1rm
             FROM sets
             WHERE exercise_id = ?1
             ORDER BY epley_e1rm DESC, weight_kg DESC, created_at DESC, id DESC
             LIMIT 1",
            [exercise_id],
            |row| {
                Ok(Self {
                    id: row.get(0)?,
                    exercise_performance_id: row.get(1)?,
                    exercise_id: row.get(2)?,
                    created_at: row.get(3)?,
                    weight_kg: row.get(4)?,
                    reps: row.get(5)?,
                    to_failure: row.get(6)?,
                    epley_e1rm: row.get(7)?,
                })
            },
        ) {
            Ok(set) => Ok(Some(set)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
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

        let (set, is_personal_best) = Set::create(
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
        assert!(
            is_personal_best,
            "an exercise's first set starts its record"
        );

        // ...and the row is really there, the boolean stored as 0/1 and the
        // e1RM computed from the Epley formula.
        let (created_at, weight_kg, reps, to_failure, stored_e1rm): (String, f64, i64, i64, f64) =
            conn.query_row(
                "SELECT created_at, weight_kg, reps, to_failure, epley_e1rm FROM sets WHERE id = ?1",
                [set.id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            (
                created_at.as_str(),
                weight_kg,
                reps,
                to_failure,
                stored_e1rm
            ),
            (
                "2026-10-20 09:15:00+00:00",
                100.0,
                5,
                1,
                epley_e1rm(100.0, 5)
            )
        );
    }

    #[test]
    fn create_links_the_performance() {
        let conn = test_db();
        let performance_id = seed_performance(&conn);

        // A second performance exists too, so the link is unambiguous.
        let (other, _) = Set::create(
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
        let (second, _) = Set::create(
            &conn,
            performance_id,
            1,
            95.0,
            6,
            true,
            dt(2026, 10, 20, 9, 30),
        )
        .unwrap();
        let (first, _) = Set::create(
            &conn,
            performance_id,
            1,
            90.0,
            8,
            false,
            dt(2026, 10, 20, 9, 25),
        )
        .unwrap();
        let (third, _) = Set::create(
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
    fn create_flags_a_personal_best_only_when_it_beats_prior_sets() {
        let conn = test_db();
        let perf1 = seed_performance(&conn); // a session for exercise 1

        // A progression on one exercise: the first set starts the record, a
        // heavier-but-fewer-reps set does not beat it (102.5×3 → 112.75
        // < 100×5 → 116.67), adding reps does (100×6 → 120), a light set
        // doesn't, more weight and reps does (110×5 → 128.33), and an
        // equal-e1RM repeat of an earlier set is not a best (strictly `>`).
        let (s1, pb1) =
            Set::create(&conn, perf1, 1, 100.0, 5, false, dt(2026, 10, 20, 9, 0)).unwrap();
        assert!(pb1, "first set of an exercise is a personal best");
        assert_eq!(s1.epley_e1rm, epley_e1rm(100.0, 5));

        let (_, pb2) =
            Set::create(&conn, perf1, 1, 102.5, 3, false, dt(2026, 10, 20, 9, 1)).unwrap();
        assert!(!pb2, "heavier at fewer reps can rank lower");

        let (_, pb3) =
            Set::create(&conn, perf1, 1, 100.0, 6, false, dt(2026, 10, 20, 9, 2)).unwrap();
        assert!(pb3);

        let (_, pb4) =
            Set::create(&conn, perf1, 1, 60.0, 10, false, dt(2026, 10, 20, 9, 3)).unwrap();
        assert!(!pb4);

        let (s5, pb5) =
            Set::create(&conn, perf1, 1, 110.0, 5, false, dt(2026, 10, 20, 9, 4)).unwrap();
        assert!(pb5);
        assert_eq!(s5.epley_e1rm, epley_e1rm(110.0, 5));

        let (_, pb6) =
            Set::create(&conn, perf1, 1, 100.0, 5, false, dt(2026, 10, 20, 9, 5)).unwrap();
        assert!(!pb6, "an equal e1RM ties, and a tie is not a new best");

        // A different exercise — even one started later, with the first
        // exercise's record already set — starts its own: comparison is per
        // exercise, across sessions. (Its own name, since exercise names are
        // unique and seed_performance would clash.)
        let squat = crate::domain::Exercise::create(&conn, "squat", "").unwrap();
        let second_session =
            crate::domain::Session::create(&conn, dt(2026, 10, 20, 10, 0)).unwrap();
        let perf2 = crate::domain::ExercisePerformance::create(
            &conn,
            second_session.id,
            squat.id,
            dt(2026, 10, 20, 10, 0),
        )
        .unwrap();
        let (_, pb_other) = Set::create(
            &conn,
            perf2.id,
            squat.id,
            40.0,
            12,
            false,
            dt(2026, 10, 20, 10, 1),
        )
        .unwrap();
        assert!(pb_other, "a fresh exercise independently starts its record");
    }

    #[test]
    fn best_set_returns_the_highest_e1rm_with_tiebreak() {
        let conn = test_db();
        let perf = seed_performance(&conn);
        // 100×6 and 80×15 tie on e1RM (both 120): the heavier one must win.
        Set::create(&conn, perf, 1, 80.0, 15, false, dt(2026, 10, 20, 9, 0)).unwrap();
        Set::create(&conn, perf, 1, 100.0, 6, false, dt(2026, 10, 20, 9, 1)).unwrap();
        Set::create(&conn, perf, 1, 50.0, 10, false, dt(2026, 10, 20, 9, 2)).unwrap();

        let best = Set::best_set(&conn, 1).unwrap().expect("a best exists");
        assert_eq!(
            (best.weight_kg, best.reps),
            (100.0, 6),
            "tie breaks toward the heavier set"
        );
        assert_eq!(best.epley_e1rm, epley_e1rm(100.0, 6));

        // An exercise with no sets has no best.
        assert!(Set::best_set(&conn, 999).unwrap().is_none());
    }

    #[test]
    fn delete_removes_the_row() {
        let conn = test_db();
        let performance_id = seed_performance(&conn);
        let (set, _) = Set::create(
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
            epley_e1rm: 0.0,
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
