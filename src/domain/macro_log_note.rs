//! A free-text note on a day's macro log, backed by the `macro_log_notes`
//! table (migration v10).
//!
//! A [`MacroLogNote`] pairs a [`MacroLog`] with a plain line of the user's
//! own words (how the day went, why a total is what it is). The table exists
//! because `macro_logs` held only items before: a day now carries any number
//! of notes. Nothing here is unique — the row's own `id` tells several notes
//! on one log apart — and each note records its own `created_at` moment, in
//! the same timezone-aware UTC format as every other `created_at` column.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::domain::DomainError;

/// One note recorded on a day: `note` was written against `macro_log_id` at
/// `created_at`. The row's own `id` tells notes that share a log apart —
/// several are allowed on a day.
#[derive(Debug, Clone, PartialEq)]
pub struct MacroLogNote {
    /// Database row id; assigned by [`MacroLogNote::create`].
    pub id: i64,
    /// The macro log this note belongs to.
    pub macro_log_id: i64,
    /// The note's text.
    pub note: String,
    /// The UTC moment the note was written.
    pub created_at: DateTime<Utc>,
}

impl MacroLogNote {
    /// Record `note` on `macro_log_id` at `created_at`, and return the row
    /// with its id. Many notes per log are exactly the intended shape. Like
    /// [`MacroLogInclusion::add`], `created_at` is passed in (the
    /// parameterized-constructor pattern of `Session::create`), so the UI can
    /// pass "now" while tests stay deterministic.
    pub fn create(
        conn: &Connection,
        macro_log_id: i64,
        note: String,
        created_at: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        conn.execute(
            "INSERT INTO macro_log_notes (macro_log_id, note, created_at) \
             VALUES (?1, ?2, ?3)",
            params![macro_log_id, note, created_at],
        )?;
        Ok(Self {
            id: conn.last_insert_rowid(),
            macro_log_id,
            note,
            created_at,
        })
    }

    /// Load one note by its own id, if it is still there — the note options
    /// screen reloads by id every pass, so an edited row re-renders and a
    /// deleted one surfaces as gone.
    pub fn find_by_id(conn: &Connection, note_id: i64) -> Result<Option<Self>, DomainError> {
        match conn.query_row(
            "SELECT id, macro_log_id, note, created_at FROM macro_log_notes WHERE id = ?1",
            [note_id],
            row_mapper,
        ) {
            Ok(note) => Ok(Some(note)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Load a log's notes in the order they were written (`created_at`, then
    /// `id`), the natural reading of a day's notes — and the order the log
    /// options screen's info lists them in.
    pub fn for_log(conn: &Connection, macro_log_id: i64) -> Result<Vec<Self>, DomainError> {
        let mut stmt = conn.prepare(
            "SELECT id, macro_log_id, note, created_at \
             FROM macro_log_notes \
             WHERE macro_log_id = ?1 \
             ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([macro_log_id], row_mapper)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Replace the note's text, keeping its `created_at` (editing rewrites
    /// *what* was said, not *when* it was said). [`DomainError::MissingRow`]
    /// when the note is already gone.
    pub fn save(&self, conn: &Connection) -> Result<(), DomainError> {
        let changed = conn.execute(
            "UPDATE macro_log_notes SET note = ?1 WHERE id = ?2",
            params![self.note, self.id],
        )?;
        if changed == 0 {
            return Err(DomainError::MissingRow {
                kind: "macro log note",
                id: self.id,
            });
        }
        Ok(())
    }

    /// Drop this note **by its own id**, so removing one never touches
    /// another note on the same day. [`DomainError::MissingRow`] when no such
    /// row exists. Wired to the note options screen's "delete" action.
    pub fn delete(&self, conn: &Connection) -> Result<(), DomainError> {
        let changed = conn.execute("DELETE FROM macro_log_notes WHERE id = ?1", [self.id])?;
        if changed == 0 {
            return Err(DomainError::MissingRow {
                kind: "macro log note",
                id: self.id,
            });
        }
        Ok(())
    }
}

/// Read one `macro_log_notes` row into a [`MacroLogNote`].
fn row_mapper(row: &rusqlite::Row<'_>) -> rusqlite::Result<MacroLogNote> {
    Ok(MacroLogNote {
        id: row.get(0)?,
        macro_log_id: row.get(1)?,
        note: row.get(2)?,
        created_at: row.get(3)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::apply_schema;
    use crate::domain::macro_log::MacroLog;

    /// An in-memory database with the full schema applied and a log to note.
    fn test_db() -> (Connection, MacroLog) {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply_schema(&mut conn).unwrap();
        let log = MacroLog::create(
            &conn,
            chrono::NaiveDate::from_ymd_opt(2026, 10, 30).unwrap(),
        )
        .unwrap();
        (conn, log)
    }

    use chrono::{DateTime, TimeZone, Utc};

    fn moment(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn create_round_trips_the_row_its_text_and_its_moment() {
        let (conn, log) = test_db();
        let when = moment(2026, 10, 30, 9, 15);

        let note = MacroLogNote::create(&conn, log.id, "Slept well".to_string(), when).unwrap();
        let loaded = MacroLogNote::find_by_id(&conn, note.id).unwrap().unwrap();
        assert_eq!(loaded, note);
        assert_eq!(loaded.note, "Slept well");
        assert_eq!(loaded.created_at, when, "the moment is stored, not lost");
    }

    #[test]
    fn find_by_id_returns_none_for_a_missing_note() {
        let conn = test_db().0;
        assert!(MacroLogNote::find_by_id(&conn, 9999).unwrap().is_none());
    }

    #[test]
    fn for_log_lists_notes_in_written_order() {
        let (conn, log) = test_db();
        let first = MacroLogNote::create(
            &conn,
            log.id,
            "morning".to_string(),
            moment(2026, 10, 30, 8, 0),
        )
        .unwrap();
        let second = MacroLogNote::create(
            &conn,
            log.id,
            "evening".to_string(),
            moment(2026, 10, 30, 21, 0),
        )
        .unwrap();

        let loaded = MacroLogNote::for_log(&conn, log.id).unwrap();
        assert_eq!(loaded, vec![first, second], "earlier moment first");
    }

    #[test]
    fn one_log_can_hold_many_notes() {
        let (conn, log) = test_db();
        let first =
            MacroLogNote::create(&conn, log.id, "one".to_string(), moment(2026, 10, 30, 8, 0))
                .unwrap();
        let second =
            MacroLogNote::create(&conn, log.id, "two".to_string(), moment(2026, 10, 30, 9, 0))
                .unwrap();
        let third = MacroLogNote::create(
            &conn,
            log.id,
            "three".to_string(),
            moment(2026, 10, 30, 10, 0),
        )
        .unwrap();

        // Three rows with distinct ids recording the same log — the separate
        // `id` primary key's whole point; nothing here is UNIQUE.
        assert_ne!(first.id, second.id);
        assert_ne!(second.id, third.id);
        let loaded = MacroLogNote::for_log(&conn, log.id).unwrap();
        assert_eq!(loaded.len(), 3);
    }

    #[test]
    fn save_replaces_the_text_but_keeps_the_moment() {
        let (conn, log) = test_db();
        let when = moment(2026, 10, 30, 9, 0);
        let mut note = MacroLogNote::create(&conn, log.id, "original".to_string(), when).unwrap();

        note.note = "rewritten".to_string();
        note.save(&conn).unwrap();

        let loaded = MacroLogNote::find_by_id(&conn, note.id).unwrap().unwrap();
        assert_eq!(loaded.note, "rewritten");
        assert_eq!(loaded.created_at, when, "editing rewrites what, not when");
    }

    #[test]
    fn save_reports_a_missing_row() {
        let conn = test_db().0;
        let err = MacroLogNote {
            id: 9999,
            macro_log_id: 1,
            note: "ghost".to_string(),
            created_at: moment(2026, 10, 30, 9, 0),
        }
        .save(&conn)
        .unwrap_err();
        assert!(
            matches!(
                err,
                DomainError::MissingRow {
                    kind: "macro log note",
                    id: 9999,
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn delete_drops_only_the_named_note_leaving_others() {
        let (conn, log) = test_db();
        let first = MacroLogNote::create(
            &conn,
            log.id,
            "keep".to_string(),
            moment(2026, 10, 30, 8, 0),
        )
        .unwrap();
        let second = MacroLogNote::create(
            &conn,
            log.id,
            "drop".to_string(),
            moment(2026, 10, 30, 9, 0),
        )
        .unwrap();

        second.delete(&conn).unwrap();
        let loaded = MacroLogNote::for_log(&conn, log.id).unwrap();
        assert_eq!(loaded, vec![first], "the other note on the day survives");
    }

    #[test]
    fn delete_reports_a_missing_row() {
        let conn = test_db().0;
        let err = MacroLogNote {
            id: 9999,
            macro_log_id: 1,
            note: "ghost".to_string(),
            created_at: moment(2026, 10, 30, 9, 0),
        }
        .delete(&conn)
        .unwrap_err();
        assert!(
            matches!(
                err,
                DomainError::MissingRow {
                    kind: "macro log note",
                    id: 9999,
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn deleting_a_log_cascades_its_notes() {
        let (conn, log) = test_db();
        MacroLogNote::create(
            &conn,
            log.id,
            "note".to_string(),
            moment(2026, 10, 30, 9, 0),
        )
        .unwrap();

        log.delete(&conn).unwrap();
        assert!(
            MacroLogNote::for_log(&conn, log.id).unwrap().is_empty(),
            "a log's notes cascade away with it"
        );
    }
}
