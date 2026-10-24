//! Muscle groups — the fixed set of muscle groups an exercise can reference,
//! and (once references exist) how strongly each exercise stimulates each.

use rusqlite::Connection;

use crate::domain::DomainError;

/// A single muscle group an exercise can reference. Backed by the
/// `muscle_group` table (migration v5); rows are seeded from
/// `db::muscle_groups::INITIAL_MUSCLE_GROUPS`.
// waits on the muscle-groups UI.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuscleGroup {
    /// Database row id.
    pub id: i64,
    /// Unique muscle group name, e.g. "Pecs".
    pub name: String,
    /// Free-form description.
    pub description: String,
}

impl MuscleGroup {
    /// Load the muscle group with the given `id`, if one exists.
    // waits on the muscle-groups UI.
    #[allow(dead_code)]
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>, DomainError> {
        match conn.query_row(
            "SELECT id, name, description FROM muscle_group WHERE id = ?1",
            [id],
            |row| {
                Ok(Self {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                })
            },
        ) {
            Ok(group) => Ok(Some(group)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Load every muscle group, in name order.
    // waits on the muscle-groups UI.
    #[allow(dead_code)]
    pub fn all(conn: &Connection) -> Result<Vec<Self>, DomainError> {
        let mut stmt =
            conn.prepare("SELECT id, name, description FROM muscle_group ORDER BY name")?;
        let rows = stmt.query_map([], |row| {
            Ok(Self {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{apply_schema, seed_muscle_groups};

    /// An in-memory database with the full schema applied and the default
    /// muscle groups seeded.
    fn test_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply_schema(&mut conn).unwrap();
        seed_muscle_groups(&mut conn).unwrap();
        conn
    }

    #[test]
    fn all_loads_every_default_muscle_group_in_name_order() {
        let conn = test_db();

        let all = MuscleGroup::all(&conn).unwrap();
        assert_eq!(all.len(), 16);
        assert_eq!(
            all.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
            [
                "Abs",
                "Biceps",
                "Calves",
                "Forearms",
                "Front delts",
                "Glutes",
                "Hamstrings",
                "Lats",
                "Obliques",
                "Pecs",
                "Quads",
                "Rear delts",
                "Side delts",
                "Spinal erectors",
                "Traps",
                "Triceps",
            ],
            "every default group comes back, sorted by name"
        );
        assert!(
            all.iter().all(|g| !g.description.is_empty()),
            "every default group carries a filled-in description"
        );
        assert!(all.iter().all(|g| g.id > 0));
    }

    #[test]
    fn find_by_id_returns_the_group_or_none() {
        let conn = test_db();

        let first = MuscleGroup::all(&conn).unwrap();
        let found = MuscleGroup::find_by_id(&conn, first[0].id)
            .unwrap()
            .unwrap();
        assert_eq!(found, first[0]);
        assert!(MuscleGroup::find_by_id(&conn, 999).unwrap().is_none());
    }
}
