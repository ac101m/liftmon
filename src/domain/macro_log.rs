//! A day's macro log: one per calendar date, backed by the `macro_logs` table
//! (migration v9).
//!
//! A [`MacroLog`] is named by its `date` — the UNIQUE column, so there is at
//! most one log per day and no separate name column; the date doubles as the
//! log's name. A log's contents are the items it recorded, held as
//! [`MacroLogInclusion`] rows: each row is one logged use of an item at a
//! quantity factor, and the day's macros ([`MacroLog::resolved_macros`])
//! resolve to the sum over the rows of quantity × each included item's resolved
//! macros — a composite item counts through its composition, like everywhere
//! else.

use chrono::NaiveDate;
use rusqlite::{Connection, params};

use crate::domain::{DomainError, Item, MacroLogInclusion, Macros};

/// A single day's macro log. `date` is the unique identity: the calendar date
/// the log records, stored as the bare YYYY-MM-DD text (never a datetime).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacroLog {
    /// Database row id; assigned by [`MacroLog::create`].
    pub id: i64,
    /// The calendar date this log records; UNIQUE, and the log's name.
    pub date: NaiveDate,
}

impl MacroLog {
    /// Insert a new log for `date`. The date column is UNIQUE — one log per
    /// day — so a duplicate surfaces as [`DomainError::NameTaken`], the date
    /// doubling as the log's name (e.g. "a macro log named '2026-10-30'
    /// already exists").
    pub fn create(conn: &Connection, date: NaiveDate) -> Result<Self, DomainError> {
        match conn.execute("INSERT INTO macro_logs (date) VALUES (?1)", params![date]) {
            Ok(_) => {}
            Err(err)
                if matches!(
                    &err,
                    rusqlite::Error::SqliteFailure(ffi_err, _)
                        if ffi_err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                ) =>
            {
                return Err(DomainError::NameTaken {
                    kind: "macro log",
                    name: date.to_string(),
                });
            }
            Err(err) => return Err(err.into()),
        }
        Ok(Self {
            id: conn.last_insert_rowid(),
            date,
        })
    }

    /// Load the log for a `date`, if one exists.
    pub fn find_by_date(conn: &Connection, date: NaiveDate) -> Result<Option<Self>, DomainError> {
        match conn.query_row(
            "SELECT id, date FROM macro_logs WHERE date = ?1",
            [date],
            row_mapper,
        ) {
            Ok(log) => Ok(Some(log)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Load the log for a `date`, creating it if there is none yet — the
    /// log-item flow's single write path: a day's first entry makes the row,
    /// every later entry reuses it.
    pub fn find_or_create(conn: &Connection, date: NaiveDate) -> Result<Self, DomainError> {
        match Self::find_by_date(conn, date)? {
            Some(log) => Ok(log),
            None => Self::create(conn, date),
        }
    }

    /// Load every log, oldest date first.
    #[allow(dead_code)] // waits on the macro-log total/history UI
    pub fn all(conn: &Connection) -> Result<Vec<Self>, DomainError> {
        let mut stmt = conn.prepare("SELECT id, date FROM macro_logs ORDER BY date")?;
        let rows = stmt.query_map([], row_mapper)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// The macros a day's log resolves to: the sum over its inclusions of each
    /// included item's resolved macros **scaled by the row's quantity factor**
    /// — a row at quantity 2 counts the item twice, so logging two servings can
    /// be one row at 2 or two rows at 1 (the same total), and a composite item
    /// resolves through its composition; an empty log sums to all zeros, like a
    /// leaf item's zero macros.
    #[allow(dead_code)] // waits on the macro-log total/history UI
    pub fn resolved_macros(conn: &Connection, log_id: i64) -> Result<Macros, DomainError> {
        let mut macros = Macros {
            unsaturated_fats: 0.0,
            saturated_fats: 0.0,
            complex_carbohydrates: 0.0,
            sugars: 0.0,
            protein: 0.0,
            calories: 0.0,
        };
        for inclusion in MacroLogInclusion::for_log(conn, log_id)? {
            let item = Item::resolved_macros(conn, inclusion.item_id)?;
            macros.unsaturated_fats += inclusion.quantity * item.unsaturated_fats;
            macros.saturated_fats += inclusion.quantity * item.saturated_fats;
            macros.complex_carbohydrates += inclusion.quantity * item.complex_carbohydrates;
            macros.sugars += inclusion.quantity * item.sugars;
            macros.protein += inclusion.quantity * item.protein;
            macros.calories += inclusion.quantity * item.calories;
        }
        Ok(macros)
    }
}

/// Read one `macro_logs` row into a [`MacroLog`].
fn row_mapper(row: &rusqlite::Row<'_>) -> rusqlite::Result<MacroLog> {
    Ok(MacroLog {
        id: row.get(0)?,
        date: row.get(1)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::apply_schema;
    use crate::domain::Item;

    /// An in-memory database with the full schema applied, plus an item to log
    /// (leaf `A` and composite `B = 2 × A`) and a created log.
    fn test_db() -> (Connection, Item, Item, MacroLog) {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply_schema(&mut conn).unwrap();
        let a = Item::create(&conn, "A", 1.0, 0.5, 2.0, 0.5, 3.0, 10.0).unwrap();
        let b = Item::create(&conn, "B", 0.0, 0.0, 0.0, 0.0, 0.0, 0.0).unwrap();
        ItemReference::set_for_item(&conn, b.id, &[(a.id, 2.0)]).unwrap();
        let log = MacroLog::create(&conn, NaiveDate::from_ymd_opt(2026, 10, 30).unwrap()).unwrap();
        (conn, a, b, log)
    }

    use crate::domain::ItemReference;
    use chrono::{DateTime, TimeZone, Utc};

    fn moment(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn create_inserts_a_row_and_round_trips_the_date() {
        let conn = test_db().0;
        // The test_db log took 2026-10-30, so use a fresh date.
        let date = NaiveDate::from_ymd_opt(2027, 1, 2).unwrap();
        let log = MacroLog::create(&conn, date).unwrap();

        assert!(log.id > 0);
        let loaded = MacroLog::find_by_date(&conn, date).unwrap().unwrap();
        assert_eq!(loaded.id, log.id);
        assert_eq!(loaded.date, date);
    }

    #[test]
    fn create_rejects_a_duplicate_date() {
        let (conn, _, _, log) = test_db();
        let err = MacroLog::create(&conn, log.date).unwrap_err();
        assert!(
            matches!(
                err,
                DomainError::NameTaken { kind: "macro log", ref name }
                    if name == "2026-10-30"
            ),
            "the date doubles as the log's name: {err}"
        );
    }

    #[test]
    fn find_by_date_returns_none_for_a_missing_day() {
        let conn = test_db().0;
        assert!(
            MacroLog::find_by_date(&conn, NaiveDate::from_ymd_opt(1999, 1, 1).unwrap())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn find_or_create_reuses_the_existing_log_or_creates_one() {
        let (conn, _, _, log) = test_db();
        let existing = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        let fresh = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();

        // An existing date comes back as the same row; a fresh date is created.
        assert_eq!(
            MacroLog::find_or_create(&conn, existing).unwrap().id,
            log.id
        );
        let created = MacroLog::find_or_create(&conn, fresh).unwrap();
        assert_eq!(created.date, fresh);
        assert!(
            MacroLog::find_by_date(&conn, fresh).unwrap().is_some(),
            "the created log is persisted"
        );
    }

    #[test]
    fn all_returns_logs_in_date_order() {
        let conn = test_db().0;
        MacroLog::create(&conn, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()).unwrap();
        MacroLog::create(&conn, NaiveDate::from_ymd_opt(2026, 12, 31).unwrap()).unwrap();
        // test_db made one for 2026-10-30 already.

        let dates: Vec<String> = MacroLog::all(&conn)
            .unwrap()
            .into_iter()
            .map(|log| log.date.to_string())
            .collect();
        assert_eq!(dates, ["2026-01-01", "2026-10-30", "2026-12-31"]);
    }

    #[test]
    fn resolved_macros_scales_by_quantity_and_resolves_composites() {
        let (conn, a, b, log) = test_db();
        // A at quantity 2 counts its macros twice.
        MacroLogInclusion::add(&conn, log.id, a.id, 2.0, moment(2026, 10, 30, 8, 0)).unwrap();
        // B at a 1.5 factor: B = 2 × A, so the row contributes 1.5 × (2 × A).
        MacroLogInclusion::add(&conn, log.id, b.id, 1.5, moment(2026, 10, 30, 12, 0)).unwrap();

        let macros = MacroLog::resolved_macros(&conn, log.id).unwrap();
        assert_eq!(
            (
                macros.unsaturated_fats,
                macros.saturated_fats,
                macros.complex_carbohydrates,
                macros.sugars,
                macros.protein,
                macros.calories,
            ),
            // 2 × A + 1.5 × (2 × A) = 5 × A.
            (5.0, 2.5, 10.0, 2.5, 15.0, 50.0)
        );
    }

    #[test]
    fn logging_the_same_item_twice_sums_its_quantities() {
        let (conn, a, _, log) = test_db();
        MacroLogInclusion::add(&conn, log.id, a.id, 1.0, moment(2026, 10, 30, 8, 0)).unwrap();
        MacroLogInclusion::add(&conn, log.id, a.id, 3.0, moment(2026, 10, 30, 18, 0)).unwrap();

        let macros = MacroLog::resolved_macros(&conn, log.id).unwrap();
        assert_eq!(
            macros.protein, 12.0,
            "two rows of A at quantities 1 and 3 count A's macros 4 times"
        );
        assert_eq!(macros.calories, 40.0);
    }

    #[test]
    fn an_empty_log_resolves_to_all_zero_macros() {
        let (conn, _, _, log) = test_db();
        let macros = MacroLog::resolved_macros(&conn, log.id).unwrap();
        assert_eq!(
            (
                macros.unsaturated_fats,
                macros.saturated_fats,
                macros.complex_carbohydrates,
                macros.sugars,
                macros.protein,
                macros.calories,
            ),
            (0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
        );
    }

    #[test]
    fn deleting_a_log_cascades_its_inclusions() {
        let (conn, a, _, log) = test_db();
        MacroLogInclusion::add(&conn, log.id, a.id, 1.0, moment(2026, 10, 30, 8, 0)).unwrap();
        // `MacroLog::delete` doesn't exist yet, so drive the schema rule
        // directly.
        conn.execute("DELETE FROM macro_logs WHERE id = ?1", [log.id])
            .unwrap();

        assert!(
            MacroLogInclusion::for_log(&conn, log.id)
                .unwrap()
                .is_empty(),
            "a log's inclusions cascade away with it"
        );
        assert!(
            Item::find_by_id(&conn, a.id).unwrap().is_some(),
            "the included item survives the delete"
        );
    }
}
