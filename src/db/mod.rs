//! Database connection and schema versioning.
//!
//! Opening a database never migrates it implicitly. Callers (the UI) inspect
//! [`Database::status`] to learn what kind of database they are pointing at,
//! then either call [`Database::upgrade`] (after the user agrees) or report a
//! compatibility problem. Upgrading an existing schema first creates a backup
//! of the old file.

mod migrations;
mod muscle_groups;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, MAIN_DB};
use rusqlite_migration::Migrations;
use thiserror::Error;

use self::migrations::{latest_version, schema_migrations};
// Re-exported so the seed is callable crate-wide (e.g. by domain tests that
// build a full current-schema database), not just inside `db`.
pub(crate) use self::muscle_groups::seed_muscle_groups;

/// An open connection to a liftmon database file.
///
/// A `Database` owns its SQLite connection and knows which schema version the
/// file is at. It never migrates on its own — that is the caller's decision,
/// via [`Database::upgrade`].
#[derive(Debug)]
pub struct Database {
    conn: Connection,
    path: PathBuf,
}

/// How compatible an opened database is with this build of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbStatus {
    /// The file did not exist (or was empty): no schema yet.
    Fresh,
    /// Schema is at the newest version this build knows.
    Current,
    /// Schema is older than this build supports and can be upgraded.
    Upgradeable { from: usize, to: usize },
    /// Schema was written by a newer build than this one; cannot be opened.
    FromFuture { version: usize },
}

/// What [`Database::upgrade`] did.
#[derive(Debug)]
pub struct UpgradeOutcome {
    /// Number of migrations applied.
    pub applied: usize,
    /// The backup of the pre-upgrade file, if one was made. `None` for a
    /// freshly-created database, which has nothing worth backing up.
    pub backup: Option<PathBuf>,
}

/// Errors that can occur while working with the database.
#[derive(Debug, Error)]
pub enum DbError {
    #[error("could not open database at {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },

    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("schema versioning error: {0}")]
    Migration(#[from] rusqlite_migration::Error),

    #[error("failed to create backup at {path}: {source}")]
    Backup {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },

    #[error(
        "the LIFTMON_DB_PATH environment variable is not set or is empty; set it to \
         the database file this run should use, e.g. LIFTMON_DB_PATH=/path/to/workouts.sqlite"
    )]
    MissingPath,

    #[error(
        "database at {path} is at schema version {version}, newer than this build supports ({latest})"
    )]
    TooNew {
        path: PathBuf,
        version: usize,
        latest: usize,
    },
}

/// Resolve the database path from a `LIFTMON_DB_PATH` lookup. `None` when
/// unset or blank — the app treats that as a refusal to start, never guessing
/// a default file. Parameterized by the lookup so unit tests can inject an
/// in-memory environment instead of mutating the real one (which is shared
/// across threads and would need `unsafe` under edition 2024).
fn database_path(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    lookup("LIFTMON_DB_PATH")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
}

impl Database {
    /// Open the database file named by the `LIFTMON_DB_PATH` environment
    /// variable, refusing to run when the variable is not set rather than
    /// silently opening a default file — the app should only ever touch a
    /// database the user explicitly pointed it at.
    pub fn open_from_env() -> Result<Self, DbError> {
        let path = database_path(|k| std::env::var_os(k)).ok_or(DbError::MissingPath)?;
        Self::open(path)
    }

    /// Open (creating if needed) the database at `path`.
    ///
    /// Does not migrate. Use [`Self::status`] to check compatibility and
    /// [`Self::upgrade`] to bring the file up to date.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path).map_err(|source| DbError::Open {
            path: path.clone(),
            source,
        })?;
        // SQLite defers foreign keys by default; the domain tables rely on
        // them, so enforce on every connection.
        conn.pragma_update(None, "foreign_keys", true)?;
        Ok(Self { conn, path })
    }

    /// The schema version currently stored on disk (0 for a fresh file).
    pub fn version(&self) -> Result<usize, DbError> {
        let version = schema_migrations().current_version(&self.conn)?;
        Ok(version.into())
    }

    /// Highest schema version this build knows how to migrate to.
    pub fn latest_version() -> usize {
        latest_version()
    }

    /// How this database relates to this build's schema.
    pub fn status(&self) -> Result<DbStatus, DbError> {
        let version = self.version()?;
        let latest = Self::latest_version();
        match version {
            0 => Ok(DbStatus::Fresh),
            v if v < latest => Ok(DbStatus::Upgradeable {
                from: v,
                to: latest,
            }),
            v if v == latest => Ok(DbStatus::Current),
            v => Ok(DbStatus::FromFuture { version: v }),
        }
    }

    /// Apply all pending migrations, backing up the old file first when an
    /// existing schema is being changed (a fresh database has nothing to back
    /// up), then load the seed data (currently the default muscle groups).
    /// Returns what the migrations did. Seeding lives here — not inside
    /// [`Self::apply_migrations`], which tests drive with synthetic migration
    /// lists over tables that lack the seed target.
    pub fn upgrade(&mut self) -> Result<UpgradeOutcome, DbError> {
        let outcome = self.apply_migrations(schema_migrations())?;
        seed_muscle_groups(&mut self.conn)?;
        Ok(outcome)
    }

    /// Shared upgrade machinery, parameterized by the migration set so tests can
    /// exercise the backup/upgrade path against synthetic migration lists.
    fn apply_migrations(&mut self, ms: &Migrations<'_>) -> Result<UpgradeOutcome, DbError> {
        let from = self.version()?;
        let pending = ms.pending_migrations(&self.conn)?;
        if pending < 0 {
            // The file is further along than this migration set knows.
            return Err(DbError::TooNew {
                path: self.path.clone(),
                version: from,
                latest: Self::latest_version(),
            });
        }
        let backup = if pending > 0 && from > 0 {
            Some(self.make_backup(from)?)
        } else {
            None
        };
        ms.to_latest(&mut self.conn)?;
        Ok(UpgradeOutcome {
            applied: pending as usize,
            backup,
        })
    }

    /// Snapshot the current database to a sibling file via SQLite's online
    /// backup, so the pre-upgrade state is preserved even in WAL mode.
    fn make_backup(&self, from: usize) -> Result<PathBuf, DbError> {
        let path = self.available_backup_path(from);
        self.conn
            .backup(MAIN_DB, &path, None)
            .map_err(|source| DbError::Backup {
                path: path.clone(),
                source,
            })?;
        Ok(path)
    }

    /// Pick a backup filename: `{db}.bak-{from}-{unix secs}`, with a numeric
    /// suffix when a previous backup already took that name.
    fn available_backup_path(&self, from: usize) -> PathBuf {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let mut candidate = PathBuf::from(format!("{}.bak-{}-{}", self.path.display(), from, secs));
        let mut n = 1;
        while candidate.exists() {
            candidate = PathBuf::from(format!(
                "{}.bak-{}-{}-{}",
                self.path.display(),
                from,
                secs,
                n
            ));
            n += 1;
        }
        candidate
    }

    /// Access to the underlying connection, used by the domain layer and the
    /// UI to run persistence queries against the migrated schema.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

/// Apply all schema migrations to an arbitrary connection. Test-only: lets
/// other modules (e.g. domain) build a scratch database in their unit tests.
#[cfg(test)]
pub(crate) fn apply_schema(conn: &mut Connection) -> Result<(), DbError> {
    schema_migrations().to_latest(conn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite_migration::M;
    use tempfile::TempDir;

    /// Owns a scratch directory so the database and its backups are removed at
    /// the end of the test.
    struct Scratch {
        _dir: TempDir,
        path: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = TempDir::new().expect("create temp dir");
            let path = dir.path().join(name);
            Self { _dir: dir, path }
        }
    }

    #[test]
    fn database_path_reads_only_the_liftmon_db_path_variable() {
        // Unset: nothing to open — the caller must refuse to start.
        assert_eq!(database_path(|_| None), None);

        // Set but blank: just as good as unset, so no path is ever guessed.
        assert_eq!(
            database_path(|key| {
                assert_eq!(key, "LIFTMON_DB_PATH");
                Some("".into())
            }),
            None
        );

        // Set to a real path, and the lookup is fed the exact variable name.
        assert_eq!(
            database_path(|key| {
                assert_eq!(key, "LIFTMON_DB_PATH");
                Some("/tmp/liftmon.sqlite".into())
            }),
            Some(PathBuf::from("/tmp/liftmon.sqlite"))
        );
    }

    #[test]
    fn fresh_file_reports_version_zero() {
        let scratch = Scratch::new("fresh.sqlite");
        assert!(!scratch.path.exists());
        let db = Database::open(&scratch.path).unwrap();
        assert!(scratch.path.exists());
        assert_eq!(db.version().unwrap(), 0);
        assert_eq!(db.status().unwrap(), DbStatus::Fresh);
    }

    #[test]
    fn upgrading_fresh_database_applies_all_migrations_without_backup() {
        let scratch = Scratch::new("init.sqlite");
        let mut db = Database::open(&scratch.path).unwrap();

        let outcome = db.upgrade().unwrap();
        assert_eq!(outcome.applied, Database::latest_version());
        assert!(outcome.backup.is_none());
        assert_eq!(db.status().unwrap(), DbStatus::Current);

        // The bootstrap table is present and usable.
        let tables: Vec<String> = db
            .connection()
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(tables.iter().any(|t| t == "app_meta"));

        // Re-running is a no-op.
        let again = db.upgrade().unwrap();
        assert_eq!(again.applied, 0);
        assert!(again.backup.is_none());
    }

    #[test]
    fn upgrading_old_database_backs_it_up_first() {
        let scratch = Scratch::new("old.sqlite");

        // An old build that only knew the first migration leaves a v1 file.
        let old_defs = [M::up(
            "CREATE TABLE app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )];
        let old = Migrations::from_slice(&old_defs);
        let mut conn = Connection::open(&scratch.path).unwrap();
        old.to_latest(&mut conn).unwrap();

        // A newer set this build *could* migrate to (one more table).
        let newer_defs = [
            M::up("CREATE TABLE app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);"),
            M::up("CREATE TABLE exercises (id INTEGER PRIMARY KEY, name TEXT NOT NULL);"),
        ];
        let newer = Migrations::from_slice(&newer_defs);
        drop(conn);

        let mut db = Database::open(&scratch.path).unwrap();
        assert_eq!(db.version().unwrap(), 1);
        // The shipped migration list only knows v1, so `status` is Current;
        // exercise the backup machinery directly via `apply_migrations`.
        let outcome = db.apply_migrations(&newer).unwrap();
        assert_eq!(outcome.applied, 1);
        let backup = outcome.backup.expect("an old database must be backed up");
        assert!(
            backup.exists(),
            "backup file should exist at {}",
            backup.display()
        );
        assert_eq!(db.version().unwrap(), 2);

        // The backup holds the pre-upgrade schema: v1, no `exercises` table.
        let backup_conn = Connection::open(&backup).unwrap();
        let user_version: i64 = backup_conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, 1);
        let exercise_tables: i64 = backup_conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='exercises'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exercise_tables, 0);
    }

    #[test]
    fn real_migration_chain_upgrades_v1_file_with_backup() {
        let scratch = Scratch::new("real-chain.sqlite");

        // Simulate a deploy that had shipped only migration v1: its file sits
        // at schema version 1 with just the bootstrap table.
        let v1_defs = [M::up(
            "CREATE TABLE app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )];
        let v1 = Migrations::from_slice(&v1_defs);
        let mut conn = Connection::open(&scratch.path).unwrap();
        v1.to_latest(&mut conn).unwrap();
        drop(conn);

        // This build knows eight more migrations (exercises, the workout
        // tables, the epley_e1rm column, the muscle groups, then the Hip
        // adductors default, items, the item composition table, then the
        // macro-log tables): opening the old file reports an upgradable schema
        // — the case main/UI prompts on.
        let mut db = Database::open(&scratch.path).unwrap();
        assert_eq!(
            db.status().unwrap(),
            DbStatus::Upgradeable { from: 1, to: 9 }
        );

        let outcome = db.upgrade().unwrap();
        assert_eq!(outcome.applied, 8);
        let backup = outcome
            .backup
            .expect("upgrading an old file must back it up");
        assert!(
            backup.exists(),
            "backup should exist at {}",
            backup.display()
        );

        // The live database is current and gained the exercises table from the
        // real v2 migration; the backup still holds the bare v1 schema.
        assert_eq!(db.status().unwrap(), DbStatus::Current);
        let exercise_tables: i64 = db
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='exercises'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exercise_tables, 1);

        let backup_conn = Connection::open(&backup).unwrap();
        let user_version: i64 = backup_conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, 1);
    }

    #[test]
    fn upgrading_v3_file_backfills_the_new_epley_e1rm_column() {
        let scratch = Scratch::new("v3-before-v4.sqlite");

        // Build a v3 file the way a pre-e1RM build would have left it: the real
        // first three migrations, then a workout with two sets. The v4
        // migration must backfill their `epley_e1rm` from weight and reps.
        let v3 = Migrations::from_slice(&super::migrations::MIGRATION_DEFS[..3]);
        let mut conn = Connection::open(&scratch.path).unwrap();
        v3.to_latest(&mut conn).unwrap();
        conn.execute_batch(
            "INSERT INTO exercises (name, description) VALUES ('bench press', '');
             INSERT INTO sessions (created_at) VALUES ('2026-10-20 09:00:00+00:00');
             INSERT INTO exercise_performances (session_id, exercise_id, created_at)
                 VALUES (1, 1, '2026-10-20 09:00:00+00:00');
             INSERT INTO sets
                 (exercise_performance_id, exercise_id, created_at, weight_kg, reps, difficulty)
                 VALUES (1, 1, '2026-10-20 09:00:00+00:00', 100.0, 5, 'easy');
             INSERT INTO sets
                 (exercise_performance_id, exercise_id, created_at, weight_kg, reps, difficulty)
                 VALUES (1, 1, '2026-10-20 09:05:00+00:00', 102.5, 3, 'failure');",
        )
        .unwrap();
        drop(conn);

        let mut db = Database::open(&scratch.path).unwrap();
        assert_eq!(
            db.status().unwrap(),
            DbStatus::Upgradeable { from: 3, to: 9 }
        );
        // v4 backfills the e1RM column; v5 adds the muscle groups; v6 adds
        // the Hip adductors default; v7 adds items; v8 adds the item
        // composition table; v9 adds the macro-log tables (and the upgrade
        // seeds the muscle groups).
        let outcome = db.upgrade().unwrap();
        assert_eq!(outcome.applied, 6);

        // Every pre-existing set now carries its e1RM, computed with the same
        // Epley formula the app writes on new inserts.
        let rows: Vec<(f64, i64, f64)> = db
            .connection()
            .prepare("SELECT weight_kg, reps, epley_e1rm FROM sets ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                (100.0, 5, 100.0 * (1.0 + 5.0 / 30.0)),
                (102.5, 3, 102.5 * (1.0 + 3.0 / 30.0)),
            ]
        );
    }

    #[test]
    fn newer_database_is_rejected_as_too_new() {
        let scratch = Scratch::new("future.sqlite");

        // A future build knew ten migrations, one more than this build, so
        // the file is left at a schema version we don't understand (v10).
        let future_defs = [
            M::up("CREATE TABLE a (x INTEGER);"),
            M::up("CREATE TABLE b (x INTEGER);"),
            M::up("CREATE TABLE c (x INTEGER);"),
            M::up("CREATE TABLE d (x INTEGER);"),
            M::up("CREATE TABLE e (x INTEGER);"),
            M::up("CREATE TABLE f (x INTEGER);"),
            M::up("CREATE TABLE g (x INTEGER);"),
            M::up("CREATE TABLE h (x INTEGER);"),
            M::up("CREATE TABLE i (x INTEGER);"),
            M::up("CREATE TABLE j (x INTEGER);"),
        ];
        let future = Migrations::from_slice(&future_defs);
        let mut conn = Connection::open(&scratch.path).unwrap();
        future.to_latest(&mut conn).unwrap();
        drop(conn);

        let mut db = Database::open(&scratch.path).unwrap();
        assert_eq!(db.status().unwrap(), DbStatus::FromFuture { version: 10 });
        assert!(
            matches!(db.upgrade(), Err(DbError::TooNew { .. })),
            "must refuse to downgrade a newer database"
        );
    }

    #[test]
    fn seed_populates_all_default_muscle_groups() {
        let scratch = Scratch::new("seeded.sqlite");
        let mut db = Database::open(&scratch.path).unwrap();
        db.upgrade().unwrap();

        let rows: Vec<(String, String)> = db
            .connection()
            .prepare("SELECT name, description FROM muscle_group ORDER BY name")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        // Every default group, with its written description — the order the
        // list declares does not matter; the DB sorts by name.
        let mut expected: Vec<(String, String)> = super::muscle_groups::INITIAL_MUSCLE_GROUPS
            .iter()
            .map(|(name, description)| (name.to_string(), description.to_string()))
            .collect();
        expected.sort();
        assert_eq!(rows, expected);
    }

    #[test]
    fn seed_is_idempotent() {
        let scratch = Scratch::new("reseed.sqlite");
        let mut db = Database::open(&scratch.path).unwrap();
        db.upgrade().unwrap();
        // A second upgrade (nothing pending) must not duplicate any group.
        db.upgrade().unwrap();

        // Nor may a direct reseed (e.g. after an app update changes the
        // defaults) — the upsert reconciles rather than duplicates.
        let mut conn = Connection::open(&scratch.path).unwrap();
        super::muscle_groups::seed_muscle_groups(&mut conn).unwrap();

        let count: i64 = db
            .connection()
            .query_row("SELECT COUNT(*) FROM muscle_group", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count,
            super::muscle_groups::INITIAL_MUSCLE_GROUPS.len() as i64
        );
    }

    #[test]
    fn v6_adds_hip_adductors_only_when_no_same_named_row_exists() {
        let scratch = Scratch::new("v6-hip-adductors.sqlite");

        // A real v5 file (the first five migrations) that already holds a
        // hand-added "Hip adductors" row. Upgrading through the v6 migration
        // must leave it as the only such row — ON CONFLICT DO NOTHING on the
        // UNIQUE name — and the seed that follows the upgrade reconciles its
        // description to the const (the usual post-upgrade end state).
        let v5 = Migrations::from_slice(&super::migrations::MIGRATION_DEFS[..5]);
        let mut conn = Connection::open(&scratch.path).unwrap();
        v5.to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO muscle_group (name, description) VALUES ('Hip adductors', 'added by hand')",
            [],
        )
        .unwrap();
        drop(conn);

        let mut db = Database::open(&scratch.path).unwrap();
        assert_eq!(
            db.status().unwrap(),
            DbStatus::Upgradeable { from: 5, to: 9 }
        );
        db.upgrade().unwrap();

        let hips: Vec<(i64, String)> = db
            .connection()
            .prepare("SELECT id, description FROM muscle_group WHERE name = 'Hip adductors'")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(hips.len(), 1, "the pre-existing row must not be duplicated");
        let const_description = super::muscle_groups::INITIAL_MUSCLE_GROUPS
            .iter()
            .find(|(name, _)| *name == "Hip adductors")
            .map(|(_, description)| *description)
            .unwrap();
        assert_eq!(hips[0].1, const_description);
    }

    #[test]
    fn stimulus_type_is_restricted_to_the_three_kinds() {
        let scratch = Scratch::new("stimulus-type.sqlite");
        let mut conn = Connection::open(&scratch.path).unwrap();
        super::apply_schema(&mut conn).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        conn.execute(
            "INSERT INTO exercises (name, description) VALUES ('bench press', '')",
            [],
        )
        .unwrap();
        super::muscle_groups::seed_muscle_groups(&mut conn).unwrap();

        let insert = |stimulus_type: &str, group_id: i64| {
            conn.execute(
                "INSERT INTO muscle_group_reference
                     (exercise_id, muscle_group_id, stimulus_type)
                 VALUES (1, ?1, ?2)",
                rusqlite::params![group_id, stimulus_type],
            )
        };

        // A 'medium' reference is fine; anything outside the three kinds
        // (fresh pairs each time, so only the CHECK can reject them) is not.
        assert!(insert("medium", 1).is_ok());
        assert!(insert("everything", 2).is_err());
        assert!(insert("", 3).is_err());
    }

    #[test]
    fn muscle_group_reference_rejects_a_duplicate_pair() {
        let scratch = Scratch::new("dup-pair.sqlite");
        let mut conn = Connection::open(&scratch.path).unwrap();
        super::apply_schema(&mut conn).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        conn.execute(
            "INSERT INTO exercises (name, description) VALUES ('bench press', '')",
            [],
        )
        .unwrap();
        super::muscle_groups::seed_muscle_groups(&mut conn).unwrap();

        // First reference for the pair is fine.
        conn.execute(
            "INSERT INTO muscle_group_reference
                 (exercise_id, muscle_group_id, stimulus_type)
             VALUES (1, 1, 'high')",
            [],
        )
        .unwrap();

        // The same (exercise, muscle group) pair again is a PK violation.
        let dup = conn.execute(
            "INSERT INTO muscle_group_reference
                 (exercise_id, muscle_group_id, stimulus_type)
             VALUES (1, 1, 'low')",
            [],
        );
        assert!(dup.is_err());
    }
}
