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

    /// `save` (or a future loader) could not find the row for this entity.
    #[allow(dead_code)] // raised by Exercise::save, not yet called outside tests
    #[error("no {kind} with id {id} exists")]
    MissingRow {
        /// The kind of entity, e.g. "exercise".
        kind: &'static str,
        /// The id that does not exist.
        id: i64,
    },
}
