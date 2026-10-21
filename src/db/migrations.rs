//! Schema migrations for the on-disk database.
//!
//! The schema is versioned with SQLite's `PRAGMA user_version`, managed by
//! [`rusqlite_migration`]. Each entry in [`MIGRATION_DEFS`] is one schema
//! version, applied in order when the database is upgraded.
//!
//! # Freezing migrations
//!
//! A migration is **frozen** from the moment it ships — a release, or any
//! build that writes the schema onto a database it doesn't own. A frozen
//! migration is the definition of "version N" for every existing file that
//! claims to be at N, so it must never be edited: changing it would silently
//! diverge those files from the schema the version number promises. Schema
//! changes go in a *new* `M::up(...)` appended to the list.
//!
//! Mark a frozen migration by leaving a `FROZEN` comment on its entry (grep for
//! `FROZEN` to find them all), naming what shipped it:
//!
//! ```text
//! // FROZEN — shipped in liftmon 0.1.0. Do not edit; append a new migration.
//! M::up(
//!     "CREATE TABLE ...",
//! ),
//! ```
//!
//! A migration **without** a `FROZEN` comment is not frozen and may still be
//! edited. New migrations start out unfrozen; add the marker only when the
//! migration actually ships.

use rusqlite_migration::{M, Migrations};

/// One migration per schema version, oldest first. `MIGRATION_DEFS.len()` is the
/// latest schema version this build knows.
pub(crate) static MIGRATION_DEFS: &[M<'static>] = &[
    M::up(
        r"
        CREATE TABLE app_meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )
    .down("DROP TABLE app_meta;")
    .comment("Bootstrap schema. Real domain tables arrive with the domain module."),
    M::up(
        r"
        CREATE TABLE exercises (
            id          INTEGER PRIMARY KEY,
            name        TEXT NOT NULL UNIQUE,
            description TEXT NOT NULL
        );
        ",
    )
    .down("DROP TABLE exercises;")
    .comment("Exercises, the first domain entity. name is unique (enforced by SQLite)."),
    M::up(
        r"
        CREATE TABLE sessions (
            id         INTEGER PRIMARY KEY,
            start_date TEXT NOT NULL,
            end_date   TEXT
        );

        CREATE TABLE exercise_performances (
            id          INTEGER PRIMARY KEY,
            session_id  INTEGER NOT NULL REFERENCES sessions(id)     ON DELETE CASCADE,
            exercise_id INTEGER NOT NULL REFERENCES exercises(id)     ON DELETE RESTRICT,
            date        TEXT NOT NULL,
            time_of_day TEXT NOT NULL
        );

        CREATE TABLE sets (
            id                      INTEGER PRIMARY KEY,
            exercise_performance_id INTEGER NOT NULL REFERENCES exercise_performances(id) ON DELETE CASCADE,
            exercise_id             INTEGER NOT NULL REFERENCES exercises(id)              ON DELETE RESTRICT,
            weight_kg               REAL NOT NULL,
            reps                    INTEGER NOT NULL,
            to_failure              INTEGER NOT NULL
        );

        -- FK columns the session-tree query joins on.
        CREATE INDEX idx_performances_session ON exercise_performances(session_id);
        CREATE INDEX idx_sets_performance ON sets(exercise_performance_id);
        ",
    )
    .down(
        r"
        DROP TABLE sets;
        DROP TABLE exercise_performances;
        DROP TABLE sessions;
        ",
    )
    .comment(
        "Sessions, exercise performances and sets — the workout-record domain. The exercise_id \
         foreign keys are ON DELETE RESTRICT (an exercise in any workout can't be deleted); the \
         session and performance foreign keys ON DELETE CASCADE, so deleting a session removes \
         its performances and their sets (and a performance its sets).",
    ),
];

/// The migrations, ready to apply to a connection.
///
/// `from_slice` is a `const` fn, so this builds once at compile time.
pub(crate) static MIGRATIONS: Migrations<'static> = Migrations::from_slice(MIGRATION_DEFS);

/// The latest schema version this build can migrate to.
pub(crate) fn latest_version() -> usize {
    MIGRATION_DEFS.len()
}

/// The full migration list, to apply against a connection.
pub(crate) fn schema_migrations() -> &'static Migrations<'static> {
    &MIGRATIONS
}
