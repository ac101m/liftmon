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
    pub fn save(&self, conn: &Connection) -> Result<(), DomainError> {
        let changed = conn.execute(
            "UPDATE exercises SET name = ?1, description = ?2 WHERE id = ?3",
            params![self.name, self.description, self.id],
        )?;
        if changed == 0 {
            return Err(DomainError::MissingRow {
                kind: "exercise",
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

    /// An in-memory database with the full schema applied.
    fn test_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_schema(&mut conn).unwrap();
        conn
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
}
