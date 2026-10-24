//! An exercise's muscle-group references: which groups it activates, and how
//! strongly. Backed by the `muscle_group_reference` table (migration v5) — the
//! cross-reference between exercises and the seeded `muscle_group` rows. The
//! stored `stimulus_type` is one of `primary`, `secondary`, `auxiliary`; the
//! picker's fourth "not selected" state has no row here, so a saved selection
//! stores exactly one row per assigned group.
//!
//! Writing replaces the whole set for an exercise ([`MuscleGroupReference::set_for_exercise`]),
//! matching the schema's compound `PRIMARY KEY (exercise_id, muscle_group_id)`:
//! the pairs are deleted and re-inserted so the submitted selection is
//! authoritative, with no upsert bookkeeping. Each statement autocommits —
//! rusqlite transactions need `&mut Connection` and the menus only ever hold
//! a shared `&Connection` (the crate's one transaction lives in the migration
//! upgrade, which has the connection mutably), so a crash mid-write could leave
//! a partial set; the writes are tiny and the refs are cosmetic, so this matches
//! the crate's statement-at-a-time persistence everywhere else.

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, Value, ValueRef};
use rusqlite::{Connection, params};

use crate::domain::DomainError;

/// How strongly an exercise stimulates a muscle group. The stored spelling —
/// `primary`, `secondary`, `auxiliary` — is the schema's `stimulus_type` CHECK
/// text, and the labels the cycle picker shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stimulus {
    Auxiliary,
    Secondary,
    Primary,
}

impl Stimulus {
    /// The stored spelling in `muscle_group_reference.stimulus_type`.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Stimulus::Auxiliary => "auxiliary",
            Stimulus::Secondary => "secondary",
            Stimulus::Primary => "primary",
        }
    }
}

// The `stimulus_type` column is TEXT, so the stored value round-trips through
// SQLite as text: `row.get(i)` reads a Stimulus straight out of a row, and
// `params![..., stimulus, ...]` binds one.
impl FromSql for Stimulus {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "auxiliary" => Ok(Stimulus::Auxiliary),
            "secondary" => Ok(Stimulus::Secondary),
            "primary" => Ok(Stimulus::Primary),
            other => Err(FromSqlError::Other(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown stimulus stored as {other:?}"),
            )))),
        }
    }
}

impl ToSql for Stimulus {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(Value::Text(self.as_str().to_owned())))
    }
}

/// One reference row: exercise `exercise_id` stimulates muscle group
/// `muscle_group_id` at `stimulus` strength. The `(exercise_id, muscle_group_id)`
/// pair is unique in the schema, so at most one row can exist per pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuscleGroupReference {
    /// The exercise that stimulates the group.
    pub exercise_id: i64,
    /// The stimulated muscle group (its `muscle_group.id`).
    pub muscle_group_id: i64,
    /// How strongly the exercise stimulates it.
    pub stimulus: Stimulus,
}

impl MuscleGroupReference {
    /// Load every reference for one exercise, in muscle-group id order; `[]`
    /// when the exercise activates no groups.
    pub fn for_exercise(conn: &Connection, exercise_id: i64) -> Result<Vec<Self>, DomainError> {
        let mut stmt = conn.prepare(
            "SELECT exercise_id, muscle_group_id, stimulus_type \
             FROM muscle_group_reference WHERE exercise_id = ?1 \
             ORDER BY muscle_group_id",
        )?;
        let rows = stmt.query_map([exercise_id], |row| {
            Ok(Self {
                exercise_id: row.get(0)?,
                muscle_group_id: row.get(1)?,
                stimulus: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Replace all of an exercise's references with exactly `pairs` — the
    /// submitted selection is authoritative (an empty `pairs` clears the
    /// exercise's groups). The pairs are `(muscle_group_id, stimulus)`; the
    /// delete comes first so the compound primary key never blocks an insert.
    pub fn set_for_exercise(
        conn: &Connection,
        exercise_id: i64,
        pairs: &[(i64, Stimulus)],
    ) -> Result<(), DomainError> {
        conn.execute(
            "DELETE FROM muscle_group_reference WHERE exercise_id = ?1",
            [exercise_id],
        )?;
        let mut stmt = conn.prepare(
            "INSERT INTO muscle_group_reference (exercise_id, muscle_group_id, stimulus_type) \
             VALUES (?1, ?2, ?3)",
        )?;
        for (muscle_group_id, stimulus) in pairs {
            stmt.execute(params![exercise_id, muscle_group_id, stimulus])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{apply_schema, seed_muscle_groups};
    use crate::domain::Exercise;

    /// An in-memory database with the full schema applied, the default muscle
    /// groups seeded, and one exercise to attach references to.
    fn test_db() -> (Connection, Exercise) {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply_schema(&mut conn).unwrap();
        seed_muscle_groups(&mut conn).unwrap();
        let exercise = Exercise::create(&conn, "bench press", "flat barbell").unwrap();
        (conn, exercise)
    }

    #[test]
    fn set_and_for_exercise_round_trip_each_stimulus() {
        let (conn, exercise) = test_db();
        let groups = crate::domain::MuscleGroup::all(&conn).unwrap();
        let mut pairs = [
            (groups[0].id, Stimulus::Primary),
            (groups[5].id, Stimulus::Secondary),
            (groups[9].id, Stimulus::Auxiliary),
        ];
        // `for_exercise` reads back in muscle-group id order, so assert against
        // the pairs that way too (the write order is irrelevant).
        pairs.sort_by_key(|(id, _)| *id);
        MuscleGroupReference::set_for_exercise(&conn, exercise.id, &pairs).unwrap();

        let refs = MuscleGroupReference::for_exercise(&conn, exercise.id).unwrap();
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].exercise_id, exercise.id);
        assert_eq!(refs[0].muscle_group_id, pairs[0].0);
        assert_eq!(refs[0].stimulus, pairs[0].1);
        assert_eq!(refs[1].muscle_group_id, pairs[1].0);
        assert_eq!(refs[1].stimulus, pairs[1].1);
        assert_eq!(refs[2].muscle_group_id, pairs[2].0);
        assert_eq!(refs[2].stimulus, pairs[2].1);
    }

    #[test]
    fn set_replaces_the_previous_selection() {
        let (conn, exercise) = test_db();
        let groups = crate::domain::MuscleGroup::all(&conn).unwrap();
        MuscleGroupReference::set_for_exercise(
            &conn,
            exercise.id,
            &[
                (groups[0].id, Stimulus::Primary),
                (groups[1].id, Stimulus::Secondary),
            ],
        )
        .unwrap();

        // Dropping one group and changing the other leaves exactly the new set.
        MuscleGroupReference::set_for_exercise(
            &conn,
            exercise.id,
            &[(groups[1].id, Stimulus::Primary)],
        )
        .unwrap();

        let refs = MuscleGroupReference::for_exercise(&conn, exercise.id).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].muscle_group_id, groups[1].id);
        assert_eq!(refs[0].stimulus, Stimulus::Primary);
    }

    #[test]
    fn an_empty_selection_clears_all_references() {
        let (conn, exercise) = test_db();
        let groups = crate::domain::MuscleGroup::all(&conn).unwrap();
        MuscleGroupReference::set_for_exercise(
            &conn,
            exercise.id,
            &[(groups[0].id, Stimulus::Primary)],
        )
        .unwrap();

        MuscleGroupReference::set_for_exercise(&conn, exercise.id, &[]).unwrap();

        assert!(
            MuscleGroupReference::for_exercise(&conn, exercise.id)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn for_exercise_is_empty_for_an_exercise_with_no_references() {
        let (conn, exercise) = test_db();
        assert!(
            MuscleGroupReference::for_exercise(&conn, exercise.id)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn refs_follow_their_exercise_down_when_it_is_deleted() {
        let (conn, exercise) = test_db();
        let groups = crate::domain::MuscleGroup::all(&conn).unwrap();
        MuscleGroupReference::set_for_exercise(
            &conn,
            exercise.id,
            &[(groups[0].id, Stimulus::Primary)],
        )
        .unwrap();

        exercise.delete(&conn).unwrap();

        // The schema cascades the delete (migration v5).
        assert!(
            MuscleGroupReference::for_exercise(&conn, exercise.id)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn from_sql_rejects_text_outside_the_three_stimuli() {
        use rusqlite::types::ValueRef;
        let err = Stimulus::column_result(ValueRef::Text(b"everything")).unwrap_err();
        assert!(
            err.to_string().contains("unknown stimulus"),
            "unexpected error text: {err}"
        );
    }
}
