//! Sets — one set within an exercise performance.

use chrono::{DateTime, Utc};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, Value, ValueRef};
use rusqlite::{Connection, params};

use crate::domain::DomainError;

/// How tough a set was. The stored spelling — `easy`, `hard`, `failure` — is
/// also the spelling the log and summary lines show; only the "How tough was
/// that?" ask uses the friendlier [`Difficulty::menu_label`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difficulty {
    Easy,
    Hard,
    Failure,
}

impl Difficulty {
    /// The stored spelling, also how log and summary lines render the level.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Difficulty::Easy => "easy",
            Difficulty::Hard => "hard",
            Difficulty::Failure => "failure",
        }
    }

    /// The friendlier wording the "How tough was that?" ask presents — and
    /// returns — for each level.
    pub(crate) fn menu_label(self) -> &'static str {
        match self {
            Difficulty::Easy => "could have done more",
            Difficulty::Hard => "pretty tough",
            Difficulty::Failure => "failure",
        }
    }

    /// All levels, in the order the ask presents them: easy, hard, failure.
    pub(crate) const ALL: [Difficulty; 3] =
        [Difficulty::Easy, Difficulty::Hard, Difficulty::Failure];

    /// The level a chosen ask option stands for.
    pub(crate) fn from_label(label: &str) -> Difficulty {
        match label {
            "could have done more" => Difficulty::Easy,
            "pretty tough" => Difficulty::Hard,
            "failure" => Difficulty::Failure,
            _ => unreachable!("the ask only presents the three difficulty labels"),
        }
    }
}

// The `sets.difficulty` column is TEXT, so the stored value round-trips through
// SQLite as text: `row.get(6)` reads a Difficulty straight out of a row, and
// `params![..., difficulty, ...]` binds one.
impl FromSql for Difficulty {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "easy" => Ok(Difficulty::Easy),
            "hard" => Ok(Difficulty::Hard),
            "failure" => Ok(Difficulty::Failure),
            other => Err(FromSqlError::Other(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown difficulty stored as {other:?}"),
            )))),
        }
    }
}

impl ToSql for Difficulty {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(Value::Text(self.as_str().to_owned())))
    }
}

/// A single set within an [`crate::domain::ExercisePerformance`]: how much was
/// lifted, how many reps, how tough it was, the Epley e1RM it produces, and
/// when it was done. Backed by the `sets` table (migration v3 plus the v4
/// `epley_e1rm` column).
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
    /// How tough the set was: easy, hard, or taken to failure.
    pub difficulty: Difficulty,
    /// The set's Epley estimated one-rep max (`weight_kg × (1 + reps / 30)`):
    /// the internal scalar that ranks sets and decides personal bests. Stored
    /// so ordering needs no recompute, but the UI always surfaces weight and
    /// reps, never this number.
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
        difficulty: Difficulty,
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
             (exercise_performance_id, exercise_id, created_at, weight_kg, reps, difficulty, epley_e1rm)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                exercise_performance_id,
                exercise_id,
                created_at,
                weight_kg,
                reps,
                difficulty,
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
                difficulty,
                epley_e1rm: epley,
            },
            is_personal_best,
        ))
    }

    /// The exercise's personal bests — the sets that beat every strictly
    /// earlier set's Epley e1RM at the time they were done — newest first, up
    /// to `limit`. Because each new best must strictly beat all that came
    /// before, they run in strictly rising e1RM order and the first is the
    /// current best. (An exercise's first-ever set is always one.)
    pub fn personal_bests(
        conn: &Connection,
        exercise_id: i64,
        limit: usize,
    ) -> Result<Vec<Self>, DomainError> {
        let mut stmt = conn.prepare(
            "SELECT id, exercise_performance_id, exercise_id, created_at, weight_kg, reps, difficulty, epley_e1rm
             FROM sets s
             WHERE s.exercise_id = ?1
               AND s.epley_e1rm > COALESCE(
                     (SELECT MAX(epley_e1rm) FROM sets s2
                      WHERE s2.exercise_id = s.exercise_id AND s2.id < s.id), 0.0)
             ORDER BY s.id DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![exercise_id, limit as i64], |row| {
            Ok(Self {
                id: row.get(0)?,
                exercise_performance_id: row.get(1)?,
                exercise_id: row.get(2)?,
                created_at: row.get(3)?,
                weight_kg: row.get(4)?,
                reps: row.get(5)?,
                difficulty: row.get(6)?,
                epley_e1rm: row.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
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
            Difficulty::Failure,
            dt(2026, 10, 20, 9, 15),
        )
        .unwrap();

        assert!(set.id > 0);
        assert_eq!(set.exercise_performance_id, performance_id);
        assert_eq!(set.exercise_id, exercise_id);
        assert_eq!(set.created_at, dt(2026, 10, 20, 9, 15));
        assert_eq!(set.weight_kg, 100.0);
        assert_eq!(set.reps, 5);
        assert_eq!(set.difficulty, Difficulty::Failure);
        assert!(
            is_personal_best,
            "an exercise's first set starts its record"
        );

        // ...and the row is really there, the level stored as its text
        // spelling and the e1RM computed from the Epley formula.
        let (created_at, weight_kg, reps, difficulty, stored_e1rm): (String, f64, i64, String, f64) =
            conn.query_row(
                "SELECT created_at, weight_kg, reps, difficulty, epley_e1rm FROM sets WHERE id = ?1",
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
                difficulty.as_str(),
                stored_e1rm
            ),
            (
                "2026-10-20 09:15:00+00:00",
                100.0,
                5,
                "failure",
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
            Difficulty::Easy,
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
                Difficulty::Easy,
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
                Difficulty::Easy,
                dt(2026, 10, 20, 9, 30)
            )
            .map(|_| true),
            Err(DomainError::Sqlite(_))
        ));
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
        let (s1, pb1) = Set::create(
            &conn,
            perf1,
            1,
            100.0,
            5,
            Difficulty::Easy,
            dt(2026, 10, 20, 9, 0),
        )
        .unwrap();
        assert!(pb1, "first set of an exercise is a personal best");
        assert_eq!(s1.epley_e1rm, epley_e1rm(100.0, 5));

        let (_, pb2) = Set::create(
            &conn,
            perf1,
            1,
            102.5,
            3,
            Difficulty::Easy,
            dt(2026, 10, 20, 9, 1),
        )
        .unwrap();
        assert!(!pb2, "heavier at fewer reps can rank lower");

        let (_, pb3) = Set::create(
            &conn,
            perf1,
            1,
            100.0,
            6,
            Difficulty::Easy,
            dt(2026, 10, 20, 9, 2),
        )
        .unwrap();
        assert!(pb3);

        let (_, pb4) = Set::create(
            &conn,
            perf1,
            1,
            60.0,
            10,
            Difficulty::Easy,
            dt(2026, 10, 20, 9, 3),
        )
        .unwrap();
        assert!(!pb4);

        let (s5, pb5) = Set::create(
            &conn,
            perf1,
            1,
            110.0,
            5,
            Difficulty::Easy,
            dt(2026, 10, 20, 9, 4),
        )
        .unwrap();
        assert!(pb5);
        assert_eq!(s5.epley_e1rm, epley_e1rm(110.0, 5));

        let (_, pb6) = Set::create(
            &conn,
            perf1,
            1,
            100.0,
            5,
            Difficulty::Easy,
            dt(2026, 10, 20, 9, 5),
        )
        .unwrap();
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
            Difficulty::Easy,
            dt(2026, 10, 20, 10, 1),
        )
        .unwrap();
        assert!(pb_other, "a fresh exercise independently starts its record");
    }

    #[test]
    fn personal_bests_lists_the_sets_that_beat_everything_before_newest_first() {
        let conn = test_db();
        let perf = seed_performance(&conn);
        // The same progression as the at-create flag test: of these six sets,
        // the first (100×5), third (100×6) and fifth (110×5) each beat
        // everything before them, so the reader must list exactly those —
        // newest first — and stop at the limit.
        for (w, r) in [
            (100.0, 5),
            (102.5, 3),
            (100.0, 6),
            (60.0, 10),
            (110.0, 5),
            (100.0, 5),
        ] {
            Set::create(
                &conn,
                perf,
                1,
                w,
                r,
                Difficulty::Easy,
                dt(2026, 10, 20, 9, 0),
            )
            .unwrap();
        }

        let bests = Set::personal_bests(&conn, 1, 3).unwrap();
        assert_eq!(
            bests
                .iter()
                .map(|s| (s.weight_kg, s.reps))
                .collect::<Vec<_>>(),
            vec![(110.0, 5), (100.0, 6), (100.0, 5)],
            "the last three personal bests, newest first"
        );
        assert_eq!(
            bests.first().unwrap().epley_e1rm,
            epley_e1rm(110.0, 5),
            "the newest best carries the highest e1RM"
        );

        // The limit is respected: ask for two, get the newer two.
        let two = Set::personal_bests(&conn, 1, 2).unwrap();
        assert_eq!(
            two.iter()
                .map(|s| (s.weight_kg, s.reps))
                .collect::<Vec<_>>(),
            vec![(110.0, 5), (100.0, 6)]
        );

        // An exercise that has never been performed has no bests.
        assert!(Set::personal_bests(&conn, 999, 3).unwrap().is_empty());
    }

    #[test]
    fn difficulty_round_trips_its_words() {
        // The stored spelling — the same one the log and summary lines show —
        // and the menu wording both map back to the level they name.
        assert_eq!(Difficulty::Easy.as_str(), "easy");
        assert_eq!(Difficulty::Hard.as_str(), "hard");
        assert_eq!(Difficulty::Failure.as_str(), "failure");
        for level in Difficulty::ALL {
            assert_eq!(Difficulty::from_label(level.menu_label()), level);
        }
        // The ask order is easy, hard, failure.
        assert_eq!(
            Difficulty::ALL.map(Difficulty::menu_label),
            ["could have done more", "pretty tough", "failure"]
        );
    }
}
