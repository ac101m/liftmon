//! Items — the first entity of the macronutrient tracker, following the
//! domain entity pattern described in `src/domain/mod.rs` (see
//! `src/domain/exercise.rs` for the reference implementation).
//!
//! An item can be a *leaf* (its macros are the six stored columns) or a
//! *composite* (it has rows in `item_reference`, and its macros are derived
//! from that graph by [`Item::resolved_macros`]) — see `src/domain/
//! item_reference.rs` for the composition DAG.

use std::collections::HashSet;

use rusqlite::{Connection, params};

use crate::domain::{DomainError, ItemReference};

/// A single item. Backed by the `items` table (migration v7).
///
/// The six macro fields are in **whatever unit the user chose** (e.g. per
/// 100 g, per serving, per whole item) — the app imposes no scale, and an
/// item's composition (migration v8) scales them by a plain factor. When the
/// item is a composite its stored macros are unused (left zero): `resolved_macros`
/// derives the real values from the `item_reference` graph instead.
///
/// # Composition key integrity
///
/// Each item's `id` is the composite key of `item_reference`
/// (`parent_id`/`child_id`), and the schema's `item_reference_no_cycles`
/// trigger — the guard that keeps the composition DAG acyclic — only fires on
/// INSERT. **Never add an edit path that UPDATEs `item_reference`'s key
/// columns**: a key rewrite would slip past the cycle check and could quietly
/// reintroduce a loop. Composition changes always go through
/// `ItemReference::set_for_item`'s delete + re-insert.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Database row id; assigned by [`Item::create`] (or a future loader).
    pub id: i64,
    /// Unique item name, e.g. "whole milk".
    pub name: String,
    /// Unsaturated fats.
    pub unsaturated_fats: f64,
    /// Of which saturated fats.
    pub saturated_fats: f64,
    /// Complex carbohydrates.
    pub complex_carbohydrates: f64,
    /// Of which sugars.
    pub sugars: f64,
    /// Protein.
    pub protein: f64,
    /// Calories (energy).
    pub calories: f64,
}

/// An item's macro values as a plain value: a leaf's stored macros, or — for
/// a composite — the sum its composition resolves to (see
/// [`Item::resolved_macros`]). The fields mirror the `items` columns.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)] // waits on the composition UI
pub struct Macros {
    pub unsaturated_fats: f64,
    pub saturated_fats: f64,
    pub complex_carbohydrates: f64,
    pub sugars: f64,
    pub protein: f64,
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

    /// Load the item with the given row `id`, if one exists — how the
    /// composition resolver looks its components up.
    #[allow(dead_code)] // waits on the composition UI
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>, DomainError> {
        match conn.query_row(
            "SELECT id, name, unsaturated_fats, saturated_fats, complex_carbohydrates, sugars, protein, calories \
             FROM items WHERE id = ?1",
            [id],
            row_mapper,
        ) {
            Ok(item) => Ok(Some(item)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
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

    /// The macros an item resolves to — the "define any item in terms of any
    /// other item" promise of the composition DAG. A leaf (no `item_reference`
    /// rows) resolves to its stored macros; a composite resolves to the sum
    /// over its composition of `quantity` × the child's resolved macros,
    /// recursing down to leaves. `set_for_item`'s pre-check and the schema's
    /// `item_reference_no_cycles` trigger keep the graph acyclic, so the
    /// recursion terminates; a visited set turns any hand-edited loop into a
    /// [`DomainError::CyclicComposition`] rather than a hang.
    #[allow(dead_code)] // waits on the composition UI
    pub fn resolved_macros(conn: &Connection, id: i64) -> Result<Macros, DomainError> {
        resolve(conn, id, &mut HashSet::new())
    }
}

/// One step of the resolver: an item's macro value is its stored columns when
/// it is a leaf, else the sum over its composition of `quantity` × the
/// child's resolved value. `visited` holds the items on the current
/// resolution path so a cycle — possible only via hand-edited data, since
/// both the domain and the schema guard against it — errors instead of
/// recursing forever; each branch removes its item afterwards, so a DAG that
/// reaches the same descendant by several routes counts it on each route.
#[allow(dead_code)] // waits on the composition UI
fn resolve(conn: &Connection, id: i64, visited: &mut HashSet<i64>) -> Result<Macros, DomainError> {
    // The composition children always exist (the schema restricts deletes of
    // used items), but a raw-deleted row would surface here rather than
    // panic.
    let item = Item::find_by_id(conn, id)?.ok_or(DomainError::MissingRow { kind: "item", id })?;
    let references = ItemReference::for_item(conn, id)?;
    if references.is_empty() {
        // A leaf: its own stored macros.
        return Ok(Macros {
            unsaturated_fats: item.unsaturated_fats,
            saturated_fats: item.saturated_fats,
            complex_carbohydrates: item.complex_carbohydrates,
            sugars: item.sugars,
            protein: item.protein,
            calories: item.calories,
        });
    }

    let mut macros = Macros {
        unsaturated_fats: 0.0,
        saturated_fats: 0.0,
        complex_carbohydrates: 0.0,
        sugars: 0.0,
        protein: 0.0,
        calories: 0.0,
    };
    for reference in references {
        if !visited.insert(reference.child_id) {
            let child = Item::find_by_id(conn, reference.child_id)?
                .map(|child| child.name)
                .unwrap_or_else(|| reference.child_id.to_string());
            return Err(DomainError::CyclicComposition {
                parent: item.name,
                child,
            });
        }
        let child_macros = resolve(conn, reference.child_id, visited)?;
        macros.unsaturated_fats += reference.quantity * child_macros.unsaturated_fats;
        macros.saturated_fats += reference.quantity * child_macros.saturated_fats;
        macros.complex_carbohydrates += reference.quantity * child_macros.complex_carbohydrates;
        macros.sugars += reference.quantity * child_macros.sugars;
        macros.protein += reference.quantity * child_macros.protein;
        macros.calories += reference.quantity * child_macros.calories;
        visited.remove(&reference.child_id);
    }
    Ok(macros)
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

    #[test]
    fn find_by_id_returns_the_row_or_none() {
        let conn = test_db();
        let item = Item::create(&conn, "whole milk", 3.6, 2.3, 4.7, 4.7, 3.3, 64.0).unwrap();
        assert_eq!(
            Item::find_by_id(&conn, item.id).unwrap().unwrap().name,
            "whole milk"
        );
        assert!(Item::find_by_id(&conn, item.id + 999).unwrap().is_none());
    }

    #[test]
    fn resolved_macros_of_a_leaf_are_its_stored_macros() {
        let conn = test_db();
        let leaf = Item::create(&conn, "egg", 1.0, 0.5, 2.0, 0.25, 3.0, 10.0).unwrap();
        let macros = Item::resolved_macros(&conn, leaf.id).unwrap();
        assert_eq!(
            (
                macros.unsaturated_fats,
                macros.saturated_fats,
                macros.complex_carbohydrates,
                macros.sugars,
                macros.protein,
                macros.calories,
            ),
            (1.0, 0.5, 2.0, 0.25, 3.0, 10.0)
        );
    }

    #[test]
    fn resolved_macros_scales_and_sums_single_level() {
        let conn = test_db();
        let b = Item::create(&conn, "B", 10.0, 4.0, 2.0, 0.0, 0.0, 50.0).unwrap();
        let c = Item::create(&conn, "C", 2.0, 1.0, 0.0, 2.0, 3.0, 10.0).unwrap();
        let a = Item::create(&conn, "A", 0.0, 0.0, 0.0, 0.0, 0.0, 0.0).unwrap();
        ItemReference::set_for_item(&conn, a.id, &[(b.id, 2.0), (c.id, 0.5)]).unwrap();

        let macros = Item::resolved_macros(&conn, a.id).unwrap();
        assert_eq!(
            (
                macros.unsaturated_fats,
                macros.saturated_fats,
                macros.complex_carbohydrates,
                macros.sugars,
                macros.protein,
                macros.calories,
            ),
            // 2×B + 0.5×C.
            (21.0, 8.5, 4.0, 1.0, 1.5, 105.0)
        );
    }

    #[test]
    fn resolved_macros_resolves_transitively_and_ignores_a_composites_stored_macros() {
        let conn = test_db();
        let flour = Item::create(&conn, "flour", 0.25, 0.0, 22.0, 0.0, 4.0, 80.0).unwrap();
        let cheese = Item::create(&conn, "cheese", 0.5, 0.25, 1.0, 0.5, 2.0, 30.0).unwrap();
        // Bread is a composite whose *stored* macros are zero — only its
        // composition counts.
        let bread = Item::create(&conn, "bread", 0.0, 0.0, 0.0, 0.0, 0.0, 0.0).unwrap();
        let sandwich = Item::create(&conn, "sandwich", 0.0, 0.0, 0.0, 0.0, 0.0, 0.0).unwrap();
        ItemReference::set_for_item(&conn, bread.id, &[(flour.id, 1.0)]).unwrap();
        ItemReference::set_for_item(&conn, sandwich.id, &[(bread.id, 2.0), (cheese.id, 1.0)])
            .unwrap();

        // bread resolves to flour; sandwich = 2×bread + 1×cheese = 2×flour +
        // cheese.
        let macros = Item::resolved_macros(&conn, sandwich.id).unwrap();
        assert_eq!(
            (
                macros.unsaturated_fats,
                macros.saturated_fats,
                macros.complex_carbohydrates,
                macros.sugars,
                macros.protein,
                macros.calories,
            ),
            (1.0, 0.25, 45.0, 0.5, 10.0, 190.0)
        );
    }

    #[test]
    fn shared_descendants_count_via_each_path() {
        let conn = test_db();
        let d = Item::create(&conn, "D", 1.0, 1.0, 1.0, 1.0, 1.0, 1.0).unwrap();
        let b = Item::create(&conn, "B", 0.0, 0.0, 0.0, 0.0, 0.0, 0.0).unwrap();
        let c = Item::create(&conn, "C", 0.0, 0.0, 0.0, 0.0, 0.0, 0.0).unwrap();
        let a = Item::create(&conn, "A", 0.0, 0.0, 0.0, 0.0, 0.0, 0.0).unwrap();
        ItemReference::set_for_item(&conn, b.id, &[(d.id, 1.0)]).unwrap();
        ItemReference::set_for_item(&conn, c.id, &[(d.id, 1.0)]).unwrap();
        ItemReference::set_for_item(&conn, a.id, &[(b.id, 1.0), (c.id, 1.0)]).unwrap();

        // A is a DAG, not a tree: D is reached via both B and C, so it counts
        // twice.
        let macros = Item::resolved_macros(&conn, a.id).unwrap();
        assert_eq!(
            (
                macros.unsaturated_fats,
                macros.complex_carbohydrates,
                macros.calories,
            ),
            (2.0, 2.0, 2.0)
        );
    }

    #[test]
    fn a_hand_planted_cycle_errors_instead_of_hanging() {
        let conn = test_db();
        let x = Item::create(&conn, "X", 1.0, 0.0, 0.0, 0.0, 0.0, 1.0).unwrap();
        let y = Item::create(&conn, "Y", 0.0, 0.0, 0.0, 0.0, 0.0, 1.0).unwrap();

        // The trigger refuses cycles by design, so plant one by dropping it
        // first, then put it back off the path — the resolver must catch the
        // loop via its visited set, not via the database.
        conn.execute_batch("DROP TRIGGER item_reference_no_cycles;")
            .unwrap();
        conn.execute(
            "INSERT INTO item_reference (parent_id, child_id, quantity) VALUES (?1, ?2, 1.0)",
            rusqlite::params![x.id, y.id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO item_reference (parent_id, child_id, quantity) VALUES (?1, ?2, 1.0)",
            rusqlite::params![y.id, x.id],
        )
        .unwrap();

        let err = Item::resolved_macros(&conn, x.id).unwrap_err();
        assert!(
            matches!(err, DomainError::CyclicComposition { .. }),
            "a planted cycle must surface as an error, not hang: {err}"
        );
    }
}
