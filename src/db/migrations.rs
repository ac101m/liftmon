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
    // FROZEN — shipped in liftmon 1.0.0. Do not edit; append a new migration.
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
         ordering needs no recompute. It ranks sets for personal records and makes their \
         at-the-time best status derivable (a set was a best when its value strictly exceeded \
         the max of the strictly earlier sets of the same exercise), without a boolean flag. \
         The `epley_` prefix leaves room for other estimators. Backfilled for existing sets.",
    ),
    // FROZEN — shipped in liftmon 1.1.0. Do not edit; append a new migration.
    M::up(
        r"
        CREATE TABLE muscle_group (
            id          INTEGER PRIMARY KEY,
            name        TEXT NOT NULL UNIQUE,
            description TEXT NOT NULL
        );

        -- Each (exercise, muscle_group) pair appears once; stimulus_type says
        -- how strongly the exercise stimulates that group: 'high' = primary
        -- mover, 'medium' = secondary/synergist, 'low' = minor/supporting.
        CREATE TABLE muscle_group_reference (
            exercise_id       INTEGER NOT NULL REFERENCES exercises(id)    ON DELETE CASCADE,
            muscle_group_id   INTEGER NOT NULL REFERENCES muscle_group(id) ON DELETE RESTRICT,
            stimulus_type     TEXT NOT NULL CHECK (stimulus_type IN ('low', 'medium', 'high')),
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
         type ('low' / 'medium' / 'high'). The muscle_group table \
         is seeded from db/muscle_groups.rs' initial/default list \
         (see `seed_muscle_groups` in the migration flow); references are added per \
         exercise. An exercise's references cascade away with it; a muscle group referenced \
         by any exercise can't be deleted.",
    ),
    // FROZEN — shipped in liftmon 1.2.0. Do not edit; append a new migration.
    M::up(
        r"
        INSERT INTO muscle_group (name, description)
        VALUES ('Hip adductors', 'Muscles of the inside leg and groin. Trained using movements that squeeze the legs together.')
        ON CONFLICT(name) DO NOTHING;
        ",
    )
    .down("DELETE FROM muscle_group WHERE name = 'Hip adductors';")
    .comment(
        "Adds the 'Hip adductors' default muscle group to databases that \
         shipped before it, so an existing file gains the new group even \
         though the muscle_group table itself is unchanged. Guarded by the \
         name's UNIQUE index (ON CONFLICT DO NOTHING): a database that \
         already holds the name — a fresh build's seed, or one added by hand \
         — is left alone. The idempotent seed that runs after every migration \
         reconciles all the default rows, this one included.",
    ),
    M::up(
        r"
        CREATE TABLE items (
            id                    INTEGER PRIMARY KEY,
            name                  TEXT NOT NULL UNIQUE,
            unsaturated_fats      REAL NOT NULL DEFAULT 0.0,
            saturated_fats        REAL NOT NULL DEFAULT 0.0,
            complex_carbohydrates REAL NOT NULL DEFAULT 0.0,
            sugars                REAL NOT NULL DEFAULT 0.0,
            protein               REAL NOT NULL DEFAULT 0.0,
            calories              REAL NOT NULL DEFAULT 0.0
        );
        ",
    )
    .down("DROP TABLE items;")
    .comment(
        "Items for the macronutrient tracker: a named thing with its macros in \
         the user's chosen unit (e.g. per 100 g, per serving, per whole item) — \
         unsaturated fats, saturated fats, complex carbohydrates and sugars \
         (grams), protein (grams) and calories (kcal). An item can also be a \
         composite of other items (the item_reference table, migration v8), in \
         which case these stored macros are unused and the item's macros \
         resolve from that graph instead. Not necessarily a single food, hence \
         'item' rather than 'ingredient'. All quantities are \
         REAL NOT NULL DEFAULT 0.0, so a row whose macros haven't been filled \
         in yet reads as a zero-macro item. name is UNIQUE, like exercises and \
         muscle groups, so create surfaces a friendly NameTaken on a duplicate.",
    ),
    // v8 — not FROZEN: the macros feature (items through composition) hasn't
    // shipped yet; freeze this and v7 together when it does.
    M::up(
        r"
        CREATE TABLE item_reference (
            parent_id            INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
            child_id             INTEGER NOT NULL REFERENCES items(id) ON DELETE RESTRICT,
            quantity             REAL NOT NULL,
            PRIMARY KEY (parent_id, child_id)
        );

        -- The DAG-keeping guard: every writer (the app, or raw SQL) is refused
        -- an edge that would close a loop. Adding (parent, child) is a cycle
        -- iff the child transitively contains the parent, so walk child edges
        -- forward from the NEW child looking for the NEW parent. UNION (not
        -- UNION ALL) dedupes the walk, so it terminates even if hand-edited
        -- data already holds a loop.
        CREATE TRIGGER item_reference_no_cycles
        BEFORE INSERT ON item_reference
        BEGIN
            WITH RECURSIVE contains(x) AS (
                SELECT NEW.child_id
                UNION
                SELECT child_id FROM item_reference
                JOIN contains ON item_reference.parent_id = contains.x
            )
            SELECT RAISE(ABORT, 'item cannot contain itself, directly or indirectly')
            WHERE NEW.parent_id IN (SELECT x FROM contains);
        END;
        ",
    )
    .down(
        r"
        DROP TRIGGER item_reference_no_cycles;
        DROP TABLE item_reference;
        ",
    )
    .comment(
        "An item composed of other items — the macronutrient tracker's DAG: \
         each row says the parent item is made with `quantity` copies of the \
         child item. `quantity` is a straight factor, not a gram amount, since \
         item macros are stored in whatever unit the user chose. A leaf item \
         has no rows here and supplies its own stored macros; a composite's \
         macros resolve to the sum over its edges of quantity x the child's \
         macros (item_reference is multi-level, so bread can itself be a \
         composite). The (parent_id, child_id) pair is the compound primary \
         key, so the same child can't be listed twice under one parent. \
         Deleting a composite cascades its composition away (CASCADE); an item \
         used as a component anywhere can't be deleted (RESTRICT). \
         Acyclicity is enforced in the schema by the item_reference_no_cycles \
         trigger — a BEFORE INSERT walk that aborts any edge closing a loop, \
         the crate's first trigger (SQLite cannot express acyclicity in a \
         CHECK) — with ItemReference::set_for_item pre-checking the whole set \
         first for a friendly error. A key-changing UPDATE of parent_id/ \
         child_id would dodge the INSERT-only trigger, so composition is \
         always rewritten as delete + insert; see the warning in \
         src/domain/item.rs.",
    ),
    // v9 — not FROZEN: part of the unfrozen macros feature (items through
    // macro logging); freeze this with v7 and v8 when the feature ships. The
    // inclusions' `quantity` column was added in place before it shipped — no
    // database has ever run this migration, so editing it diverges no file.
    M::up(
        r"
        CREATE TABLE macro_logs (
            id   INTEGER PRIMARY KEY,
            date TEXT NOT NULL UNIQUE
        );

        CREATE TABLE macro_log_inclusions (
            id           INTEGER PRIMARY KEY,
            macro_log_id INTEGER NOT NULL REFERENCES macro_logs(id) ON DELETE CASCADE,
            item_id      INTEGER NOT NULL REFERENCES items(id)      ON DELETE RESTRICT,
            quantity     REAL NOT NULL,
            created_at   TEXT NOT NULL
        );

        -- FK column the per-log inclusion loaders join on.
        CREATE INDEX idx_inclusions_log ON macro_log_inclusions(macro_log_id);

        -- The 'Hip abductors' default muscle group, appended here while this
        -- migration is still unfrozen (v9 has never run against a file, so
        -- editing it diverges no database) the way the 'Hip adductors' default
        -- shipped in v6: an existing file gains the new group even though the
        -- muscle_group table itself is unchanged. Guarded by the name's
        -- UNIQUE index (ON CONFLICT DO NOTHING): a database that already
        -- holds the name — a fresh build's seed, or one added by hand — is
        -- left alone. The idempotent seed that runs after every migration
        -- reconciles all the default rows, this one included.
        INSERT INTO muscle_group (name, description)
        VALUES ('Hip abductors', 'Muscles on the outside of the pelvis. Trained using movements that push the legs apart.')
        ON CONFLICT(name) DO NOTHING;
        ",
    )
    .down(
        r"
        DROP TABLE macro_log_inclusions;
        DROP TABLE macro_logs;
        DELETE FROM muscle_group WHERE name = 'Hip abductors';
        ",
    )
    .comment(
        "Day-by-day macro logging for the macronutrient tracker. `macro_logs` \
         is one row per day: its `date` column holds a plain calendar date \
         (YYYY-MM-DD, not a datetime) and is UNIQUE, so there can be only one \
         log for a date and the date doubles as the log's name — the table \
         carries no separate name column. `macro_log_inclusions` records which \
         items were logged on that day: the same item can appear several times \
         — its own `id` primary key (unlike the compound-PK inclusions tables, \
         muscle_group_reference and item_reference) permits duplicate \
         (macro_log_id, item_id) rows — each row being one logged use of the \
         item **at a `quantity` factor**, the same straight scale as \
         item_reference's quantity (not a gram amount; an item's macros are in \
         the user's chosen unit). Logging two servings can be one row with \
         quantity 2 or two rows with quantity 1 — the day's total is the same \
         either way — and the day's macros resolve to the sum over its rows of \
         quantity x the item's resolved macros. Every inclusion \
         also carries a `created_at` UTC moment, recording when the item was \
         logged in the same timezone-aware way as the workout tables, and the \
         idx_inclusions_log index (like the session-tree indexes) serves the \
         per-log lookups. A log's inclusions cascade away with it (CASCADE); an \
         item included in any log can't be deleted (RESTRICT). There is no \
         cycle trigger: a log contains items, never other logs, so the graph \
         cannot loop. See src/domain/macro_log.rs. This migration also seeds \
         the 'Hip abductors' default muscle group (appended in place — see the \
         guarding INSERT at the end of its `up`), the v6 'Hip adductors' \
         precedent.",
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
