//! Items — the first entity of the macronutrient tracker, following the
//! domain entity pattern described in `src/domain/mod.rs` (see
//! `src/domain/exercise.rs` for the reference implementation).

use rusqlite::{Connection, params};

use crate::domain::DomainError;

/// A single item. Backed by the `items` table (migration v7).
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Database row id; assigned by [`Item::create`] (or a future loader).
    pub id: i64,
    /// Unique item name, e.g. "whole milk".
    pub name: String,
    /// Unsaturated fats in grams per 100 g.
    pub unsaturated_fats: f64,
    /// Of which saturated fats, in grams per 100 g.
    pub saturated_fats: f64,
    /// Complex carbohydrates in grams per 100 g.
    pub complex_carbohydrates: f64,
    /// Of which sugars, in grams per 100 g.
    pub sugars: f64,
    /// Protein in grams per 100 g.
    pub protein: f64,
    /// Calories (energy) in kcal per 100 g.
    pub calories: f64,
}

impl Item {
    /// Insert a new item row and return the persisted entity.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        conn: &Connection,
        name: impl Into<String>,
        unsaturated_fats: f64,
        saturated_fats: f64,
        complex_carbohydrates: f64,
        sugars: f64,
        protein: f64,
        calories: f64,
    ) -> Result<Self, DomainError> {
        let name = name.into();

        match conn.execute(
            "INSERT INTO items \
                 (name, unsaturated_fats, saturated_fats, complex_carbohydrates, sugars, protein, calories) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![name, unsaturated_fats, saturated_fats, complex_carbohydrates, sugars, protein, calories],
        ) {
            Ok(_) => {}
            // items.name is the only UNIQUE column, so any unique-
            // constraint violation in this INSERT is a duplicate name.
            Err(err)
                if matches!(
                    &err,
                    rusqlite::Error::SqliteFailure(ffi_err, _)
                        if ffi_err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                ) =>
            {
                return Err(DomainError::NameTaken { kind: "item", name });
            }
            Err(err) => return Err(err.into()),
        }

        Ok(Self {
            id: conn.last_insert_rowid(),
            name,
            unsaturated_fats,
            saturated_fats,
            complex_carbohydrates,
            sugars,
            protein,
            calories,
        })
    }

    /// Load the item with the given unique `name`, if one exists — the
    /// check the add-item form makes before asking for the macros, so a
    /// duplicate name surfaces before any quantities are typed.
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Option<Self>, DomainError> {
        match conn.query_row(
            "SELECT id, name, unsaturated_fats, saturated_fats, complex_carbohydrates, sugars, protein, calories \
             FROM items WHERE name = ?1",
            [name],
            row_mapper,
        ) {
            Ok(item) => Ok(Some(item)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Load every item, in name order — the list the show-items picker
    /// offers, browsable with the filter box.
    pub fn all(conn: &Connection) -> Result<Vec<Self>, DomainError> {
        let mut stmt = conn.prepare(
            "SELECT id, name, unsaturated_fats, saturated_fats, complex_carbohydrates, sugars, protein, calories \
             FROM items ORDER BY name",
        )?;
        let rows = stmt.query_map([], row_mapper)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

/// Read one `items` row into an [`Item`].
fn row_mapper(row: &rusqlite::Row<'_>) -> rusqlite::Result<Item> {
    Ok(Item {
        id: row.get(0)?,
        name: row.get(1)?,
        unsaturated_fats: row.get(2)?,
        saturated_fats: row.get(3)?,
        complex_carbohydrates: row.get(4)?,
        sugars: row.get(5)?,
        protein: row.get(6)?,
        calories: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::apply_schema;

    /// An in-memory database with the full schema applied.
    fn test_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply_schema(&mut conn).unwrap();
        conn
    }

    #[test]
    fn create_inserts_row_and_assigns_id() {
        let conn = test_db();
        let item = Item::create(&conn, "whole milk", 3.6, 2.3, 4.7, 4.7, 3.3, 64.0).unwrap();

        assert!(item.id > 0);
        assert_eq!(item.name, "whole milk");
        assert_eq!(
            (
                item.unsaturated_fats,
                item.saturated_fats,
                item.complex_carbohydrates
            ),
            (3.6, 2.3, 4.7)
        );
        assert_eq!((item.sugars, item.protein, item.calories), (4.7, 3.3, 64.0));

        // ...and the row is really there.
        let (name, unsaturated_fats, protein, calories): (String, f64, f64, f64) = conn
            .query_row(
                "SELECT name, unsaturated_fats, protein, calories FROM items WHERE id = ?1",
                [item.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (name.as_str(), unsaturated_fats, protein, calories),
            ("whole milk", 3.6, 3.3, 64.0)
        );
    }

    #[test]
    fn create_rejects_duplicate_name() {
        let conn = test_db();
        Item::create(&conn, "egg", 9.5, 3.1, 0.7, 0.7, 12.6, 143.0).unwrap();
        let err = Item::create(&conn, "egg", 0.0, 0.0, 0.0, 0.0, 0.0, 0.0).unwrap_err();
        assert!(matches!(err, DomainError::NameTaken { kind: "item", .. }));
    }

    #[test]
    fn find_by_name_returns_the_row_or_none() {
        let conn = test_db();
        Item::create(&conn, "whole milk", 3.6, 2.3, 4.7, 4.7, 3.3, 64.0).unwrap();

        let found = Item::find_by_name(&conn, "whole milk").unwrap().unwrap();
        assert_eq!(found.calories, 64.0);
        assert!(Item::find_by_name(&conn, "no such name").unwrap().is_none());
    }

    #[test]
    fn all_returns_rows_in_name_order() {
        let conn = test_db();
        Item::create(&conn, "egg", 9.5, 3.1, 0.7, 0.7, 12.6, 143.0).unwrap();
        Item::create(&conn, "whole milk", 3.6, 2.3, 4.7, 4.7, 3.3, 64.0).unwrap();
        Item::create(&conn, "chicken breast", 3.6, 1.0, 0.0, 0.0, 31.0, 165.0).unwrap();

        let all = Item::all(&conn).unwrap();
        assert_eq!(
            all.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
            ["chicken breast", "egg", "whole milk"],
            "rows come back sorted by name"
        );
        assert_eq!(all[1].protein, 12.6);
        assert!(all.iter().all(|i| i.id > 0));
    }

    #[test]
    fn all_on_empty_db_returns_empty() {
        let conn = test_db();
        assert!(
            Item::all(&conn).unwrap().is_empty(),
            "no items on a fresh database"
        );
    }
}
