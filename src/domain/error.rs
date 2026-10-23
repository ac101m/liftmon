//! Errors produced by the domain layer.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DomainError {
    /// Error from the underlying SQLite call.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// `create` was asked for a name that already exists (a UNIQUE column).
    #[error("a {kind} named '{name}' already exists")]
    NameTaken {
        /// The kind of entity, e.g. "exercise".
        kind: &'static str,
        /// The name that is already taken.
        name: String,
    },

    /// `delete` was asked to remove a row that other rows still reference. The
    /// schema refuses it (ON DELETE RESTRICT, migration v3); this maps that
    /// constraint to a friendly error the UI can report.
    #[error("cannot delete {kind} '{name}' — it is still used")]
    InUse {
        /// The kind of entity, e.g. "exercise".
        kind: &'static str,
        /// The name of the entity that is still referenced.
        name: String,
    },

    /// `save`/`delete` could not find the row for this entity.
    #[error("no {kind} with id {id} exists")]
    MissingRow {
        /// The kind of entity, e.g. "exercise".
        kind: &'static str,
        /// The id that does not exist.
        id: i64,
    },
}
