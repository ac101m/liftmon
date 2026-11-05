//! A body-weight measurement, backed by the `weight_measurements` table
//! (migration v11).
//!
//! A [`WeightMeasurement`] records one weigh-in: the body weight in kilograms
//! for a calendar `date` at a `created_at` moment. Several weigh-ins can share
//! a date (nothing here is UNIQUE) — `date` is the day the weigh-in records
//! (the user's chosen local calendar day, stored as bare YYYY-MM-DD, the same
//! day convention a macro log's date uses) and `created_at` is the UTC moment
//! it was actually entered, in the same timezone-aware format as every other
//! `created_at` column. A row's own `id` tells measurements apart.

use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{Connection, params};

use crate::domain::DomainError;

/// One recorded weigh-in: the body weight in kilograms for the calendar
/// `date`, entered at `created_at`. The row's own `id` tells measurements
/// apart — several per day are the natural shape, so nothing here is UNIQUE.
#[derive(Debug, Clone, PartialEq)]
pub struct WeightMeasurement {
    /// Database row id; assigned by [`WeightMeasurement::create`].
    pub id: i64,
    /// The calendar date this weigh-in records (YYYY-MM-DD).
    pub date: NaiveDate,
    /// The UTC moment the weight was entered.
    pub created_at: DateTime<Utc>,
    /// The body weight, in kilograms.
    pub weight_kg: f64,
}

impl WeightMeasurement {
    /// Record a weigh-in for `date` at `created_at`, and return the row with
    /// its id. `date` and `created_at` are passed in (the
    /// parameterized-constructor pattern of `Session::create`), so the UI can
    /// pass the chosen day and "now" while tests stay deterministic.
    pub fn create(
        conn: &Connection,
        weight_kg: f64,
        date: NaiveDate,
        created_at: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        conn.execute(
            "INSERT INTO weight_measurements (date, created_at, weight_kg) VALUES (?1, ?2, ?3)",
            params![date, created_at, weight_kg],
        )?;
        Ok(Self {
            id: conn.last_insert_rowid(),
            date,
            created_at,
            weight_kg,
        })
    }

    /// Load one measurement by its own id, if it is still there — the entry
    /// options screen reloads by id every pass, so an edited row re-renders
    /// and a deleted one surfaces as gone.
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>, DomainError> {
        match conn.query_row(
            "SELECT id, date, created_at, weight_kg FROM weight_measurements WHERE id = ?1",
            [id],
            row_mapper,
        ) {
            Ok(measurement) => Ok(Some(measurement)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Load every measurement by their recorded date, then their entry moment
    /// (`date`, then `created_at`, then `id`) — the chronological reading of a
    /// weight history, where a backdated weigh-in sorts under its own day, not
    /// under the day it was entered.
    pub fn all(conn: &Connection) -> Result<Vec<Self>, DomainError> {
        let mut stmt = conn.prepare(
            "SELECT id, date, created_at, weight_kg \
             FROM weight_measurements \
             ORDER BY date, created_at, id",
        )?;
        let rows = stmt.query_map([], row_mapper)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Load a date's measurements — the "has this day already been weighed?"
    /// check the log-weight flow makes, and the rows a history groups under a
    /// day. One query for the day, in entry order.
    pub fn on_date(conn: &Connection, date: NaiveDate) -> Result<Vec<Self>, DomainError> {
        let mut stmt = conn.prepare(
            "SELECT id, date, created_at, weight_kg \
             FROM weight_measurements \
             WHERE date = ?1 \
             ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([date], row_mapper)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Replace the entry's weight, keeping its `date` and `created_at`
    /// (editing rewrites *what* was weighed, not the day it records or when it
    /// was entered). [`DomainError::MissingRow`] when the entry is already
    /// gone.
    pub fn save(&self, conn: &Connection) -> Result<(), DomainError> {
        let changed = conn.execute(
            "UPDATE weight_measurements SET weight_kg = ?1 WHERE id = ?2",
            params![self.weight_kg, self.id],
        )?;
        if changed == 0 {
            return Err(DomainError::MissingRow {
                kind: "weight log",
                id: self.id,
            });
        }
        Ok(())
    }

    /// Drop this entry **by its own id**, so removing one never touches
    /// another weigh-in on the same day. [`DomainError::MissingRow`] when no
    /// such row exists. Wired to the entry options screen's "delete" action.
    pub fn delete(&self, conn: &Connection) -> Result<(), DomainError> {
        let changed = conn.execute("DELETE FROM weight_measurements WHERE id = ?1", [self.id])?;
        if changed == 0 {
            return Err(DomainError::MissingRow {
                kind: "weight log",
                id: self.id,
            });
        }
        Ok(())
    }
}

/// Read one `weight_measurements` row into a [`WeightMeasurement`].
fn row_mapper(row: &rusqlite::Row<'_>) -> rusqlite::Result<WeightMeasurement> {
    Ok(WeightMeasurement {
        id: row.get(0)?,
        date: row.get(1)?,
        created_at: row.get(2)?,
        weight_kg: row.get(3)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::apply_schema;

    /// An in-memory database with the full schema applied. No seed needed —
    /// measurements are app-created rows, not defaults.
    fn test_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply_schema(&mut conn).unwrap();
        conn
    }

    use chrono::{DateTime, TimeZone, Utc};

    fn moment(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn create_inserts_a_row_and_round_trips_weight_date_and_moment() {
        let conn = test_db();
        let when = moment(2026, 11, 4, 7, 30);

        let measurement = WeightMeasurement::create(&conn, 82.5, date(2026, 11, 4), when).unwrap();
        assert!(measurement.id > 0);

        let loaded = WeightMeasurement::find_by_id(&conn, measurement.id)
            .unwrap()
            .unwrap();
        assert_eq!(loaded, measurement);
        assert_eq!(
            loaded.weight_kg, 82.5,
            "the weight in kg is stored, not lost"
        );
        assert_eq!(
            loaded.date,
            date(2026, 11, 4),
            "the recorded day is stored, not lost"
        );
        assert_eq!(loaded.created_at, when, "the moment is stored, not lost");
    }

    #[test]
    fn create_stores_date_and_moment_in_the_standard_text_formats() {
        let conn = test_db();
        // The parameterized-constructor pattern passes a synthetic moment, but
        // what lands on disk must be the same texts every other date/created_at
        // column uses — the date as bare YYYY-MM-DD and the moment as the
        // timezone-aware UTC text (e.g. '2026-11-04 07:30:00+00:00') — so
        // loaders and any raw SQL read them the same way everywhere.
        WeightMeasurement::create(&conn, 80.0, date(2026, 11, 4), moment(2026, 11, 4, 7, 30))
            .unwrap();

        let (stored_date, stored_moment): (String, String) = conn
            .query_row(
                "SELECT date, created_at FROM weight_measurements",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored_date, "2026-11-04");
        assert_eq!(stored_moment, "2026-11-04 07:30:00+00:00");
    }

    #[test]
    fn find_by_id_returns_none_for_a_missing_measurement() {
        let conn = test_db();
        assert!(
            WeightMeasurement::find_by_id(&conn, 9999)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn all_lists_measurements_by_recorded_date_not_entry_moment() {
        let conn = test_db();
        // Three weigh-ins entered in a different order than the days they
        // record: an 11/03 entered second, an 11/01 entered third (a backdate
        // typed in after), and an 11/04 entered last — so entry-moment order
        // would read 11/03, 11/01, 11/04, while the recorded-day order the
        // history wants is 11/01, 11/03, 11/04.
        let third =
            WeightMeasurement::create(&conn, 83.0, date(2026, 11, 3), moment(2026, 11, 4, 8, 0))
                .unwrap();
        let first =
            WeightMeasurement::create(&conn, 84.0, date(2026, 11, 1), moment(2026, 11, 4, 9, 0))
                .unwrap();
        let fourth =
            WeightMeasurement::create(&conn, 82.5, date(2026, 11, 4), moment(2026, 11, 4, 10, 0))
                .unwrap();

        let loaded = WeightMeasurement::all(&conn).unwrap();
        let weights: Vec<f64> = loaded.iter().map(|m| m.weight_kg).collect();
        assert_eq!(
            weights,
            [84.0, 83.0, 82.5],
            "the 11/01 backdate sorts before 11/03, the 11/03 before 11/04"
        );
        assert_eq!(loaded, vec![first, third, fourth]);
    }

    #[test]
    fn all_ties_same_date_and_moment_in_id_order() {
        let conn = test_db();
        // Two same-moment weigh-ins for the same day (a UI asking "now" could
        // produce these in one pass): the id tiebreak keeps the order
        // deterministic, and the row's own `id` is what tells the two rows
        // apart.
        let when = moment(2026, 11, 4, 7, 30);
        let first = WeightMeasurement::create(&conn, 82.5, date(2026, 11, 4), when).unwrap();
        let second = WeightMeasurement::create(&conn, 82.5, date(2026, 11, 4), when).unwrap();

        let loaded = WeightMeasurement::all(&conn).unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(loaded, vec![first, second]);
    }

    #[test]
    fn all_is_empty_when_nothing_has_been_recorded() {
        let conn = test_db();
        assert!(WeightMeasurement::all(&conn).unwrap().is_empty());
    }

    #[test]
    fn on_date_returns_only_that_days_rows_in_entry_order() {
        let conn = test_db();
        let first =
            WeightMeasurement::create(&conn, 82.5, date(2026, 11, 4), moment(2026, 11, 4, 7, 0))
                .unwrap();
        let second =
            WeightMeasurement::create(&conn, 82.0, date(2026, 11, 4), moment(2026, 11, 4, 8, 0))
                .unwrap();
        // A different day's weigh-in must not leak into the day's rows.
        WeightMeasurement::create(&conn, 83.0, date(2026, 11, 3), moment(2026, 11, 3, 7, 0))
            .unwrap();

        let loaded = WeightMeasurement::on_date(&conn, date(2026, 11, 4)).unwrap();
        assert_eq!(
            loaded,
            vec![first, second],
            "both of the day's, in entry order"
        );
        assert!(
            WeightMeasurement::on_date(&conn, date(2026, 11, 5))
                .unwrap()
                .is_empty(),
            "a day with no weigh-ins reads empty"
        );
    }

    #[test]
    fn save_updates_the_weight_but_keeps_date_and_moment() {
        let conn = test_db();
        let when = moment(2026, 11, 4, 7, 30);
        let mut measurement =
            WeightMeasurement::create(&conn, 82.5, date(2026, 11, 4), when).unwrap();

        measurement.weight_kg = 81.9;
        measurement.save(&conn).unwrap();

        let loaded = WeightMeasurement::find_by_id(&conn, measurement.id)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.weight_kg, 81.9);
        assert_eq!(loaded.date, date(2026, 11, 4), "the recorded day is kept");
        assert_eq!(loaded.created_at, when, "the entry moment is kept");
    }

    #[test]
    fn save_reports_a_missing_row() {
        let conn = test_db();
        let err = WeightMeasurement {
            id: 9999,
            date: date(2026, 11, 4),
            created_at: moment(2026, 11, 4, 7, 30),
            weight_kg: 82.5,
        }
        .save(&conn)
        .unwrap_err();
        assert!(
            matches!(
                err,
                DomainError::MissingRow {
                    kind: "weight log",
                    id: 9999,
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn delete_drops_only_the_named_entry_leaving_others_on_the_day() {
        let conn = test_db();
        let first =
            WeightMeasurement::create(&conn, 82.5, date(2026, 11, 4), moment(2026, 11, 4, 7, 0))
                .unwrap();
        let second =
            WeightMeasurement::create(&conn, 82.0, date(2026, 11, 4), moment(2026, 11, 4, 8, 0))
                .unwrap();

        second.delete(&conn).unwrap();
        let loaded = WeightMeasurement::on_date(&conn, date(2026, 11, 4)).unwrap();
        assert_eq!(
            loaded,
            vec![first],
            "the other weigh-in on the day survives"
        );
    }

    #[test]
    fn delete_reports_a_missing_row() {
        let conn = test_db();
        let err = WeightMeasurement {
            id: 9999,
            date: date(2026, 11, 4),
            created_at: moment(2026, 11, 4, 7, 30),
            weight_kg: 82.5,
        }
        .delete(&conn)
        .unwrap_err();
        assert!(
            matches!(
                err,
                DomainError::MissingRow {
                    kind: "weight log",
                    id: 9999,
                }
            ),
            "{err}"
        );
    }
}
