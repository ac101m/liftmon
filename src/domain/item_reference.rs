//! An item's composition: which other items it is made of, and how much of
//! each. Backed by the `item_reference` table (migration v8) — the
//! macronutrient tracker's DAG, where any item can be defined in terms of any
//! other item ("sandwich = bread + cheese + salad + chicken").
//!
//! Each row says the parent item is composed with `quantity` copies of the
//! child item — a straight factor, not a gram amount, since `items` stores its
//! macros in whatever unit the user chose. The graph is multi-level (bread can
//! itself be a composite), and it is kept a **DAG**: the schema's
//! `item_reference_no_cycles` trigger aborts any insert that would close a
//! loop, and [`set_for_item`] pre-checks the whole set first so a rejected
//! edit surfaces a friendly [`DomainError::CyclicComposition`] without
//! touching the parent's existing composition.

use std::collections::HashSet;

use rusqlite::{Connection, params};

use crate::domain::{DomainError, Item};

/// One composition edge: parent item includes `quantity` × child item. The
/// `(parent_id, child_id)` pair is the table's compound primary key, so an
/// item includes each child at most once.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemReference {
    /// The composite item being defined.
    pub parent_id: i64,
    /// The item it is composed of.
    pub child_id: i64,
    /// How many copies of the child the parent is made with — a factor that
    /// scales the child's resolved macros.
    pub quantity: f64,
}

impl ItemReference {
    /// Replace an item's whole composition with exactly `pairs` — the
    /// submitted set is authoritative (an empty `pairs` clears it). The pairs
    /// are `(child_id, quantity)`; each child is checked against the current
    /// graph first, so a pair that would close a loop fails the *whole* edit
    /// with [`DomainError::CyclicComposition`] and leaves the parent's current
    /// composition untouched. The delete-then-insert rewrite is what keeps
    /// composition changes out of the INSERT-only cycle trigger's blind spot
    /// (key-changing UPDATEs would slip past it — never add one, see the
    /// warning on [`Item`]).
    ///
    /// The schema's trigger is the backstop for any raw-SQL writer; this
    /// pre-check is what turns a rejection into a friendly, all-or-nothing
    /// error when the app does the writing.
    #[allow(dead_code)] // waits on the composition UI
    pub fn set_for_item(
        conn: &Connection,
        parent_id: i64,
        pairs: &[(i64, f64)],
    ) -> Result<(), DomainError> {
        for (child_id, _) in pairs {
            if contains_transitively(conn, *child_id, parent_id, &mut HashSet::new())? {
                return Err(DomainError::CyclicComposition {
                    parent: item_name(conn, parent_id),
                    child: item_name(conn, *child_id),
                });
            }
        }

        conn.execute(
            "DELETE FROM item_reference WHERE parent_id = ?1",
            [parent_id],
        )?;
        let mut stmt = conn.prepare(
            "INSERT INTO item_reference (parent_id, child_id, quantity) \
             VALUES (?1, ?2, ?3)",
        )?;
        for (child_id, quantity) in pairs {
            stmt.execute(params![parent_id, child_id, quantity])?;
        }
        Ok(())
    }

    /// Load an item's composition, in child-name order (stable, like every
    /// list in the app). `[]` when the item is a leaf.
    #[allow(dead_code)] // waits on the composition UI
    pub fn for_item(conn: &Connection, parent_id: i64) -> Result<Vec<Self>, DomainError> {
        let mut stmt = conn.prepare(
            "SELECT ri.parent_id, ri.child_id, ri.quantity \
             FROM item_reference ri \
             JOIN items i ON i.id = ri.child_id \
             WHERE ri.parent_id = ?1 \
             ORDER BY i.name",
        )?;
        let rows = stmt.query_map([parent_id], |row| {
            Ok(Self {
                parent_id: row.get(0)?,
                child_id: row.get(1)?,
                quantity: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

/// Whether `item_id` transitively contains (is composed of, directly or
/// indirectly) `target`. Walking forward along child edges is how a would-be
/// parent→child edge is checked for closing a loop: adding it would make the
/// child contain its own future parent. The visited set keeps the walk
/// terminating even if hand-edited data already holds a loop.
fn contains_transitively(
    conn: &Connection,
    item_id: i64,
    target: i64,
    visited: &mut HashSet<i64>,
) -> Result<bool, DomainError> {
    if item_id == target {
        return Ok(true);
    }
    if !visited.insert(item_id) {
        return Ok(false); // already walked this branch; a loop elsewhere can't hide the target
    }
    for reference in ItemReference::for_item(conn, item_id)? {
        if contains_transitively(conn, reference.child_id, target, visited)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The item's name for an error message, falling back to its id if the row is
/// somehow gone (composition children always exist — the schema restricts it).
fn item_name(conn: &Connection, id: i64) -> String {
    Item::find_by_id(conn, id)
        .ok()
        .flatten()
        .map(|item| item.name)
        .unwrap_or_else(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::apply_schema;
    use crate::domain::Item;

    /// An in-memory database with the full schema applied and a few leaf items
    /// to compose, in alphabetical order (A, B, C, D, E).
    fn test_db() -> (Connection, Vec<Item>) {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply_schema(&mut conn).unwrap();
        let items = [
            ("A", 1.0, 0.5, 2.0, 0.5, 3.0, 10.0),
            ("B", 4.0, 2.0, 1.0, 1.0, 5.0, 20.0),
            ("C", 0.5, 0.25, 8.0, 0.25, 1.0, 5.0),
            ("D", 0.25, 0.0, 0.5, 0.0, 0.5, 2.0),
            ("E", 2.0, 1.0, 4.0, 2.0, 7.0, 30.0),
        ]
        .map(|(name, uf, sf, cc, su, p, c)| {
            Item::create(&conn, name, uf, sf, cc, su, p, c).unwrap()
        });
        (conn, items.to_vec())
    }

    #[test]
    fn set_and_for_item_round_trip_factors() {
        let (conn, items) = test_db();
        let a = items[0].id; // A
        let b = items[1].id; // B
        let c = items[2].id; // C

        ItemReference::set_for_item(&conn, a, &[(c, 0.5), (b, 2.0)]).unwrap();

        // Read back in child-name order (B before C), whatever the write order.
        let refs = ItemReference::for_item(&conn, a).unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].child_id, b);
        assert_eq!(refs[0].quantity, 2.0);
        assert_eq!(refs[1].child_id, c);
        assert_eq!(refs[1].quantity, 0.5);
    }

    #[test]
    fn set_replaces_the_previous_composition() {
        let (conn, items) = test_db();
        let a = items[0].id;
        let b = items[1].id;
        let c = items[2].id;

        ItemReference::set_for_item(&conn, a, &[(b, 1.0)]).unwrap();
        ItemReference::set_for_item(&conn, a, &[(c, 3.0)]).unwrap();

        let refs = ItemReference::for_item(&conn, a).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].child_id, c);
        assert_eq!(refs[0].quantity, 3.0);
    }

    #[test]
    fn an_empty_set_clears_the_composition() {
        let (conn, items) = test_db();
        let a = items[0].id;
        let b = items[1].id;

        ItemReference::set_for_item(&conn, a, &[(b, 1.0)]).unwrap();
        ItemReference::set_for_item(&conn, a, &[]).unwrap();

        assert!(
            ItemReference::for_item(&conn, a).unwrap().is_empty(),
            "an empty set resets an item to a leaf"
        );
    }

    #[test]
    fn rejects_a_direct_self_reference() {
        let (conn, items) = test_db();
        let a = items[0].id;

        let err = ItemReference::set_for_item(&conn, a, &[(a, 1.0)]).unwrap_err();
        assert!(
            matches!(err, DomainError::CyclicComposition { ref parent, ref child }
                if parent == "A" && child == "A"),
            "unexpected error: {err}"
        );
        // The rejected set left A a leaf.
        assert!(ItemReference::for_item(&conn, a).unwrap().is_empty());
    }

    #[test]
    fn rejects_a_direct_two_item_cycle_and_keeps_the_previous_set() {
        let (conn, items) = test_db();
        let a = items[0].id;
        let b = items[1].id;

        ItemReference::set_for_item(&conn, b, &[(a, 1.0)]).unwrap(); // B contains A

        let err = ItemReference::set_for_item(&conn, a, &[(b, 1.0)]).unwrap_err();
        assert!(
            matches!(err, DomainError::CyclicComposition { ref parent, ref child }
                if parent == "A" && child == "B"),
            "unexpected error: {err}"
        );
        // The whole edit was rejected before any write: A stays a leaf.
        assert!(ItemReference::for_item(&conn, a).unwrap().is_empty());
    }

    #[test]
    fn rejects_an_indirect_cycle() {
        let (conn, items) = test_db();
        let a = items[0].id;
        let b = items[1].id;
        let c = items[2].id;

        // C contains A, B contains C — so A → B would close A → B → C → A.
        ItemReference::set_for_item(&conn, c, &[(a, 1.0)]).unwrap();
        ItemReference::set_for_item(&conn, b, &[(c, 1.0)]).unwrap();
        let err = ItemReference::set_for_item(&conn, a, &[(b, 1.0)]).unwrap_err();
        assert!(
            matches!(err, DomainError::CyclicComposition { .. }),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn allows_a_valid_multilevel_chain() {
        let (conn, items) = test_db();
        let a = items[0].id;
        let b = items[1].id;
        let c = items[2].id;

        ItemReference::set_for_item(&conn, b, &[(c, 1.0)]).unwrap();
        ItemReference::set_for_item(&conn, a, &[(b, 2.0)]).unwrap();

        assert_eq!(ItemReference::for_item(&conn, a).unwrap().len(), 1);
        assert_eq!(ItemReference::for_item(&conn, b).unwrap().len(), 1);
        assert!(ItemReference::for_item(&conn, c).unwrap().is_empty());
    }

    #[test]
    fn deleting_a_composite_cascades_its_references() {
        let (conn, items) = test_db();
        let a = items[0].id;
        let b = items[1].id;

        ItemReference::set_for_item(&conn, a, &[(b, 1.0)]).unwrap();
        // `Item::delete` doesn't exist yet, so drive the schema rule directly.
        conn.execute("DELETE FROM items WHERE id = ?1", [a])
            .unwrap();

        assert!(
            ItemReference::for_item(&conn, a).unwrap().is_empty(),
            "a composite's composition rows cascade away with it"
        );
        assert!(
            Item::find_by_id(&conn, b).unwrap().is_some(),
            "the child item survives the delete"
        );
    }

    #[test]
    fn deleting_a_component_item_is_refused() {
        let (conn, items) = test_db();
        let a = items[0].id;
        let b = items[1].id;

        ItemReference::set_for_item(&conn, a, &[(b, 1.0)]).unwrap();
        let err = conn
            .execute("DELETE FROM items WHERE id = ?1", [b])
            .unwrap_err();
        assert!(
            matches!(err, rusqlite::Error::SqliteFailure(_, _)),
            "a used component must be refused by the foreign key: {err}"
        );
    }

    /// The v8 trigger's guard, proven against raw SQL that sidesteps the
    /// domain entirely: a plain INSERT that would close a loop is aborted.
    #[test]
    fn the_schema_trigger_refuses_a_raw_cycle_insert() {
        let (conn, items) = test_db();
        let a = items[0].id;
        let b = items[1].id;
        let c = items[2].id;

        ItemReference::set_for_item(&conn, b, &[(a, 1.0)]).unwrap(); // B contains A

        // A → B would make A a transitive ingredient of itself.
        let err = conn
            .execute(
                "INSERT INTO item_reference (parent_id, child_id, quantity) \
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![a, b, 1.0],
            )
            .unwrap_err();
        assert!(
            matches!(
                &err,
                rusqlite::Error::SqliteFailure(ffi_err, _)
                    if ffi_err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER
            ),
            "expected the cycle trigger to abort the raw insert: {err}"
        );

        // A valid acyclic raw insert passes the same trigger.
        conn.execute(
            "INSERT INTO item_reference (parent_id, child_id, quantity) \
             VALUES (?1, ?2, ?3)",
            rusqlite::params![a, c, 2.0],
        )
        .unwrap();
    }

    /// If hand-edited data ever holds a loop, the trigger's UNION-walk must
    /// still terminate (dedupe, not loop forever), and an unrelated insert
    /// still succeeds.
    #[test]
    fn the_schema_trigger_terminates_on_pre_existing_cycles() {
        let (conn, items) = test_db();
        let a = items[0].id;
        let b = items[1].id;
        let c = items[2].id;

        // The trigger refuses cycles by design, so plant one by hand: drop the
        // trigger, insert a two-node loop (A ↔ B), then put the guard back.
        conn.execute_batch("DROP TRIGGER item_reference_no_cycles;")
            .unwrap();
        conn.execute(
            "INSERT INTO item_reference (parent_id, child_id, quantity) VALUES (?1, ?2, 1.0)",
            rusqlite::params![a, b],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO item_reference (parent_id, child_id, quantity) VALUES (?1, ?2, 1.0)",
            rusqlite::params![b, a],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER item_reference_no_cycles \
             BEFORE INSERT ON item_reference \
             BEGIN \
                 WITH RECURSIVE contains(x) AS ( \
                     SELECT NEW.child_id \
                     UNION \
                     SELECT child_id FROM item_reference \
                     JOIN contains ON item_reference.parent_id = contains.x \
                 ) \
                 SELECT RAISE(ABORT, 'item cannot contain itself, directly or indirectly') \
                 WHERE NEW.parent_id IN (SELECT x FROM contains); \
             END;",
        )
        .unwrap();

        // C → A is acyclic (A contains B contains A — never C), and the walk
        // through the planted loop must terminate rather than recurse forever.
        conn.execute(
            "INSERT INTO item_reference (parent_id, child_id, quantity) VALUES (?1, ?2, 1.0)",
            rusqlite::params![c, a],
        )
        .unwrap();
    }
}
