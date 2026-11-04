//! A body-weight measurement, backed by the `weight_measurements` table
//! (migration v11).
//!
//! A [`WeightMeasurement`] records one weigh-in: the body weight in kilograms
//! at a `created_at` moment, stored in the same timezone-aware UTC format as
//! every other `created_at` column. Nothing here is UNIQUE — you can weigh in
//! more than once a day — so the row's own `id` tells measurements apart, and
//! a measurement stands alone (no foreign keys, unlike the workout tables).

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::domain::DomainError;

/// One recorded weigh-in: the body weight in kilograms at the moment it was
/// taken. The row's own `id` tells measurements apart — several per day are
/// the natural shape, so nothing here is UNIQUE.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // waits on the weight-tracking UI
pub struct WeightMeasurement {
    /// Database row id; assigned by [`WeightMeasurement::create`].
    pub id: i64,
    /// The UTC moment the weight was recorded.
    pub created_at: DateTime<Utc>,
    /// The body weight, in kilograms.
    pub weight_kg: f64,
}

#[allow(dead_code)] // waits on the weight-tracking UI
impl WeightMeasurement {
    /// Record a weigh-in at `created_at`, and return the row with its id.
    /// Like [`crate::domain::MacroLogNote::create`], `created_at` is passed
    /// in (the parameterized-constructor pattern of `Session::create`), so
    /// the UI can pass "now" while tests stay deterministic.
    pub fn create(
        conn: &Connection,
        weight_kg: f64,
        created_at: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        conn.execute(
            "INSERT INTO weight_measurements (created_at, weight_kg) VALUES (?1, ?2)",
            params![created_at, weight_kg],
        )?;
        Ok(Self {
            id: conn.last_insert_rowid(),
            created_at,
            weight_kg,
        })
    }

    /// Load one measurement by its own id, if it is still there.
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>, DomainError> {
        match conn.query_row(
            "SELECT id, created_at, weight_kg FROM weight_measurements WHERE id = ?1",
            [id],
            row_mapper,
        ) {
            Ok(measurement) => Ok(Some(measurement)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Load every measurement in the order they were taken (`created_at`,
    /// then `id`) — the natural reading of a weight history, from first to
    /// most recent.
    pub fn all(conn: &Connection) -> Result<Vec<Self>, DomainError> {
        let mut stmt = conn.prepare(
            "SELECT id, created_at, weight_kg \
             FROM weight_measurements \
             ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([], row_mapper)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

/// Read one `weight_measurements` row into a [`WeightMeasurement`].
fn row_mapper(row: &rusqlite::Row<'_>) -> rusqlite::Result<WeightMeasurement> {
    Ok(WeightMeasurement {
        id: row.get(0)?,
        created_at: row.get(1)?,
        weight_kg: row.get(2)?,
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

    #[test]
    fn create_inserts_a_row_and_round_trips_the_weight_and_moment() {
        let conn = test_db();
        let when = moment(2026, 11, 4, 7, 30);

        let measurement = WeightMeasurement::create(&conn, 82.5, when).unwrap();
        assert!(measurement.id > 0);

        let loaded = WeightMeasurement::find_by_id(&conn, measurement.id)
            .unwrap()
            .unwrap();
        assert_eq!(loaded, measurement);
        assert_eq!(
            loaded.weight_kg, 82.5,
            "the weight in kg is stored, not lost"
        );
        assert_eq!(loaded.created_at, when, "the moment is stored, not lost");
    }

    #[test]
    fn create_stores_created_at_in_the_standard_utc_text_format() {
        let conn = test_db();
        // The parameterized-constructor pattern passes a synthetic moment, but
        // what lands on disk must be the same timezone-aware UTC text every
        // other created_at column uses (e.g. '2026-11-04 07:30:00+00:00'), so
        // loaders and any raw SQL read it the same way everywhere.
        WeightMeasurement::create(&conn, 80.0, moment(2026, 11, 4, 7, 30)).unwrap();

        let stored: String = conn
            .query_row("SELECT created_at FROM weight_measurements", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(stored, "2026-11-04 07:30:00+00:00");
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
    fn all_lists_measurements_in_taken_order() {
        let conn = test_db();
        let first = WeightMeasurement::create(&conn, 83.0, moment(2026, 11, 1, 7, 0)).unwrap();
        let second = WeightMeasurement::create(&conn, 82.7, moment(2026, 11, 3, 7, 0)).unwrap();
        let third = WeightMeasurement::create(&conn, 82.4, moment(2026, 11, 4, 7, 0)).unwrap();

        let loaded = WeightMeasurement::all(&conn).unwrap();
        assert_eq!(
            loaded,
            vec![first, second, third],
            "earliest weigh-in first"
        );
    }

    #[test]
    fn all_ties_same_moment_in_id_order() {
        let conn = test_db();
        // Two same-moment weigh-ins (a UI asking "now" could produce these in
        // one pass): the id tiebreak keeps the order deterministic, and the
        // row's own `id` is what tells the two rows apart.
        let when = moment(2026, 11, 4, 7, 30);
        let first = WeightMeasurement::create(&conn, 82.5, when).unwrap();
        let second = WeightMeasurement::create(&conn, 82.5, when).unwrap();

        let loaded = WeightMeasurement::all(&conn).unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(loaded, vec![first, second]);
    }

    #[test]
    fn all_is_empty_when_nothing_has_been_recorded() {
        let conn = test_db();
        assert!(WeightMeasurement::all(&conn).unwrap().is_empty());
    }
}
