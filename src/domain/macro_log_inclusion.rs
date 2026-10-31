//! One recorded use of an item within a day's macro log, backed by the
//! `macro_log_inclusions` table (migration v9).
//!
//! Each row pairs a [`MacroLog`] with an [`Item`] it recorded, and carries its
//! **own `id`** — unlike the compound-PK inclusion tables ([`ItemReference`],
//! `muscle_group_reference`) — so the same item can appear on several rows of
//! one log: each row is one use at a `quantity` factor, recorded at its own
//! `created_at` moment. That is the table's whole reason to exist, and why
//! [`MacroLogInclusion::remove`] deletes by the row's own id rather than by
//! (log, item).

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::domain::DomainError;

/// One item use logged on a day: `item_id` was recorded on `macro_log_id` at a
/// `quantity` factor, at `created_at`. The row's own `id` tells rows that share
/// a (log, item) pair apart — logging the same item twice makes two inclusions,
/// each contributing its own quantity.
#[derive(Debug, Clone, PartialEq)]
pub struct MacroLogInclusion {
    /// Database row id; assigned by [`MacroLogInclusion::add`].
    pub id: i64,
    /// The macro log this inclusion belongs to.
    pub macro_log_id: i64,
    /// The item that was logged.
    pub item_id: i64,
    /// How many of the item this row records — a straight factor on the item's
    /// resolved macros, like `item_reference`'s quantity.
    pub quantity: f64,
    /// The UTC moment the item was recorded.
    pub created_at: DateTime<Utc>,
}

impl MacroLogInclusion {
    /// Record that `item_id` was used on `macro_log_id` at a `quantity` factor
    /// at `created_at`, and return the row with its id. Duplicate (log, item)
    /// additions are exactly the intended shape — two uses of the same item in
    /// one log are two rows, each with its own id, quantity and moment.
    /// `created_at` is passed in (the parameterized-constructor pattern of
    /// `Session::create`), so the UI can pass "now" while tests stay
    /// deterministic.
    pub fn add(
        conn: &Connection,
        macro_log_id: i64,
        item_id: i64,
        quantity: f64,
        created_at: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        conn.execute(
            "INSERT INTO macro_log_inclusions (macro_log_id, item_id, quantity, created_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![macro_log_id, item_id, quantity, created_at],
        )?;
        Ok(Self {
            id: conn.last_insert_rowid(),
            macro_log_id,
            item_id,
            quantity,
            created_at,
        })
    }

    /// Load a log's inclusions in the order they were recorded (`created_at`,
    /// then `id`), the natural reading of a day's log now that each use has its
    /// moment.
    #[allow(dead_code)] // waits on the macro-log total/history UI
    pub fn for_log(conn: &Connection, macro_log_id: i64) -> Result<Vec<Self>, DomainError> {
        let mut stmt = conn.prepare(
            "SELECT id, macro_log_id, item_id, quantity, created_at \
             FROM macro_log_inclusions \
             WHERE macro_log_id = ?1 \
             ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([macro_log_id], |row| {
            Ok(Self {
                id: row.get(0)?,
                macro_log_id: row.get(1)?,
                item_id: row.get(2)?,
                quantity: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Drop one inclusion **by its own id**, so removing one use never touches
    /// a later use of the same item (the duplicates the table exists for).
    /// [`DomainError::MissingRow`] when no such row exists.
    #[allow(dead_code)] // waits on the macro-log total/history UI
    pub fn remove(conn: &Connection, inclusion_id: i64) -> Result<(), DomainError> {
        let changed = conn.execute(
            "DELETE FROM macro_log_inclusions WHERE id = ?1",
            [inclusion_id],
        )?;
        if changed == 0 {
            return Err(DomainError::MissingRow {
                kind: "macro log inclusion",
                id: inclusion_id,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::apply_schema;
    use crate::domain::Item;
    use crate::domain::macro_log::MacroLog;

    /// An in-memory database with the full schema applied, an item to log, and
    /// a log to include it in.
    fn test_db() -> (Connection, Item, MacroLog) {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply_schema(&mut conn).unwrap();
        let item = Item::create(&conn, "A", 1.0, 0.5, 2.0, 0.5, 3.0, 10.0).unwrap();
        let log = MacroLog::create(
            &conn,
            chrono::NaiveDate::from_ymd_opt(2026, 10, 30).unwrap(),
        )
        .unwrap();
        (conn, item, log)
    }

    use chrono::{DateTime, TimeZone, Utc};

    fn moment(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn add_round_trips_the_row_its_quantity_and_its_moment() {
        let (conn, item, log) = test_db();
        let when = moment(2026, 10, 30, 9, 15);

        let added = MacroLogInclusion::add(&conn, log.id, item.id, 2.5, when).unwrap();
        let loaded = MacroLogInclusion::for_log(&conn, log.id).unwrap();
        assert_eq!(loaded, vec![added]);
        assert_eq!(loaded[0].quantity, 2.5, "the quantity factor is stored");
        assert_eq!(loaded[0].created_at, when, "the moment is stored, not lost");
    }

    #[test]
    fn for_log_lists_rows_in_recorded_order() {
        let (conn, item, log) = test_db();
        let first = MacroLogInclusion::add(&conn, log.id, item.id, 1.0, moment(2026, 10, 30, 8, 0))
            .unwrap();
        let second =
            MacroLogInclusion::add(&conn, log.id, item.id, 2.0, moment(2026, 10, 30, 11, 30))
                .unwrap();

        let loaded = MacroLogInclusion::for_log(&conn, log.id).unwrap();
        assert_eq!(loaded, vec![first, second], "earlier moment first");
    }

    #[test]
    fn the_same_item_can_be_included_many_times_in_one_log() {
        let (conn, item, log) = test_db();
        let first = MacroLogInclusion::add(&conn, log.id, item.id, 1.0, moment(2026, 10, 30, 8, 0))
            .unwrap();
        let second =
            MacroLogInclusion::add(&conn, log.id, item.id, 3.0, moment(2026, 10, 30, 18, 0))
                .unwrap();

        // Two rows with distinct ids recording the same (log, item) pair —
        // the separate `id` primary key's whole point.
        assert_ne!(first.id, second.id);
        let loaded = MacroLogInclusion::for_log(&conn, log.id).unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn remove_drops_only_the_named_row_leaving_duplicates() {
        let (conn, item, log) = test_db();
        let first = MacroLogInclusion::add(&conn, log.id, item.id, 1.0, moment(2026, 10, 30, 8, 0))
            .unwrap();
        let second =
            MacroLogInclusion::add(&conn, log.id, item.id, 3.0, moment(2026, 10, 30, 18, 0))
                .unwrap();

        MacroLogInclusion::remove(&conn, first.id).unwrap();
        let loaded = MacroLogInclusion::for_log(&conn, log.id).unwrap();
        assert_eq!(
            loaded,
            vec![second],
            "the other use of the same item survives"
        );
    }

    #[test]
    fn remove_reports_a_missing_row() {
        let conn = test_db().0;
        let err = MacroLogInclusion::remove(&conn, 9999).unwrap_err();
        assert!(
            matches!(
                err,
                DomainError::MissingRow {
                    kind: "macro log inclusion",
                    id: 9999,
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn deleting_an_included_item_is_refused() {
        let (conn, item, log) = test_db();
        MacroLogInclusion::add(&conn, log.id, item.id, 1.0, moment(2026, 10, 30, 8, 0)).unwrap();

        // `Item::delete` doesn't exist yet (deleting items is a future slice),
        // so drive the schema rule directly: the item_id foreign key is ON
        // DELETE RESTRICT, so the raw delete of an included item is refused.
        // (A RESTRICT/NO ACTION violation is raised through the FK-trigger
        // mechanism, so it reports SQLITE_CONSTRAINT_TRIGGER; 787 is the plain
        // foreign-key code — the exercise.rs delete matches both.)
        let err = conn
            .execute("DELETE FROM items WHERE id = ?1", [item.id])
            .unwrap_err();
        assert!(
            matches!(
                err,
                rusqlite::Error::SqliteFailure(ffi_err, _)
                    if matches!(
                        ffi_err.extended_code,
                        rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY
                            | rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER
                    )
            ),
            "an item included in any log can't be deleted: {err}"
        );
    }

    #[test]
    fn deleting_a_log_cascades_its_inclusions() {
        let (conn, item, log) = test_db();
        MacroLogInclusion::add(&conn, log.id, item.id, 1.0, moment(2026, 10, 30, 8, 0)).unwrap();

        conn.execute("DELETE FROM macro_logs WHERE id = ?1", [log.id])
            .unwrap();
        assert!(
            MacroLogInclusion::for_log(&conn, log.id)
                .unwrap()
                .is_empty(),
            "a log's inclusions cascade away with it"
        );
        assert!(
            Item::find_by_id(&conn, item.id).unwrap().is_some(),
            "the included item survives the delete"
        );
    }
}
