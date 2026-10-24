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
    // FROZEN — shipped in liftmon 1.0.0. Do not edit; append a new migration.
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
    // FROZEN — shipped in liftmon 1.0.0. Do not edit; append a new migration.
    M::up(
        r"
        CREATE TABLE sessions (
            id         INTEGER PRIMARY KEY,
            created_at TEXT NOT NULL
        );

        CREATE TABLE exercise_performances (
            id          INTEGER PRIMARY KEY,
            session_id  INTEGER NOT NULL REFERENCES sessions(id)     ON DELETE CASCADE,
            exercise_id INTEGER NOT NULL REFERENCES exercises(id)     ON DELETE RESTRICT,
            created_at  TEXT NOT NULL
        );

        CREATE TABLE sets (
            id                      INTEGER PRIMARY KEY,
            exercise_performance_id INTEGER NOT NULL REFERENCES exercise_performances(id) ON DELETE CASCADE,
            exercise_id             INTEGER NOT NULL REFERENCES exercises(id)              ON DELETE RESTRICT,
            created_at              TEXT NOT NULL,
            weight_kg               REAL NOT NULL,
            reps                    INTEGER NOT NULL,
            difficulty              TEXT NOT NULL CHECK (difficulty IN ('easy','hard','failure'))
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
        "Sessions, exercise performances and sets — the workout-record domain. Every table \
         carries `created_at`, the timezone-aware (UTC) moment it was created: a session when \
         it started, a performance when its exercise was picked, a set when it was logged. The \
         exercise_id foreign keys are ON DELETE RESTRICT (an exercise in any workout can't be \
         deleted); the session and performance foreign keys ON DELETE CASCADE, so deleting a \
         session removes its performances and their sets (and a performance its sets).",
    ),
    // FROZEN — shipped in liftmon 1.0.0. Do not edit; append a new migration.
    M::up(
        r"
        ALTER TABLE sets ADD COLUMN epley_e1rm REAL NOT NULL DEFAULT 0;
        UPDATE sets SET epley_e1rm = weight_kg * (1.0 + reps / 30.0);
        ",
    )
    .down("ALTER TABLE sets DROP COLUMN epley_e1rm;")
    .comment(
        "Each set's Epley estimated one-rep max (weight_kg * (1 + reps / 30)), stored so \
         ordering needs no recompute. It ranks sets for personal bests and makes their \
         at-the-time best status derivable (a set was a best when its value strictly exceeded \
         the max of the strictly earlier sets of the same exercise), without a boolean flag. \
         The `epley_` prefix leaves room for other estimators. Backfilled for existing sets.",
    ),
    M::up(
        r"
        CREATE TABLE muscle_group (
            id          INTEGER PRIMARY KEY,
            name        TEXT NOT NULL UNIQUE,
            description TEXT NOT NULL
        );

        -- Each (exercise, muscle_group) pair appears once; stimulus_type says
        -- how strongly the exercise stimulates that group: 'primary' = primary
        -- mover, 'secondary' = secondary/synergist, 'auxiliary' = minor/supporting.
        CREATE TABLE muscle_group_reference (
            exercise_id       INTEGER NOT NULL REFERENCES exercises(id)    ON DELETE CASCADE,
            muscle_group_id   INTEGER NOT NULL REFERENCES muscle_group(id) ON DELETE RESTRICT,
            stimulus_type     TEXT NOT NULL CHECK (stimulus_type IN ('primary', 'secondary', 'auxiliary')),
            PRIMARY KEY (exercise_id, muscle_group_id)
        );
        ",
    )
    .down(
        r"
        DROP TABLE muscle_group_reference;
        DROP TABLE muscle_group;
        ",
    )
    .comment(
        "Muscle groups an exercise activates, each tagged with a stimulus \
         type ('primary' / 'secondary' / 'auxiliary'). The muscle_group table \
         is seeded from db/muscle_groups.rs' initial/default list \
         (see `seed_muscle_groups` in the migration flow); references are added per \
         exercise. An exercise's references cascade away with it; a muscle group referenced \
         by any exercise can't be deleted.",
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
