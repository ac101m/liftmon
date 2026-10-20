//! Database connection and schema versioning.
//!
//! Opening a database never migrates it implicitly. Callers (the UI) inspect
//! [`Database::status`] to learn what kind of database they are pointing at,
//! then either call [`Database::upgrade`] (after the user agrees) or report a
//! compatibility problem. Upgrading an existing schema first creates a backup
//! of the old file, per the requirement in `src/db/README.md`.

mod migrations;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, MAIN_DB};
use rusqlite_migration::Migrations;
use thiserror::Error;

use self::migrations::{latest_version, schema_migrations};

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
        "database at {path} is at schema version {version}, newer than this build supports ({latest})"
    )]
    TooNew {
        path: PathBuf,
        version: usize,
        latest: usize,
    },
}

impl Database {
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

    /// True when the file has an older schema that [`Self::upgrade`] can bring
    /// up to [`Self::latest_version`].
    #[allow(dead_code)] // convenience for the UI, which is not built yet
    pub fn needs_upgrade(&self) -> Result<bool, DbError> {
        Ok(matches!(self.status()?, DbStatus::Upgradeable { .. }))
    }

    /// True when the file was written by a newer build than this one.
    #[allow(dead_code)] // convenience for the UI, which is not built yet
    pub fn is_from_future(&self) -> Result<bool, DbError> {
        Ok(matches!(self.status()?, DbStatus::FromFuture { .. }))
    }

    /// Apply all pending migrations, backing up the old file first when an
    /// existing schema is being changed (a fresh database has nothing to back
    /// up). Returns what was done.
    pub fn upgrade(&mut self) -> Result<UpgradeOutcome, DbError> {
        self.apply_migrations(schema_migrations())
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
    fn fresh_file_reports_version_zero() {
        let scratch = Scratch::new("fresh.sqlite");
        assert!(!scratch.path.exists());
        let db = Database::open(&scratch.path).unwrap();
        assert!(scratch.path.exists());
        assert_eq!(db.version().unwrap(), 0);
        assert_eq!(db.status().unwrap(), DbStatus::Fresh);
        assert!(!db.needs_upgrade().unwrap());
        assert!(!db.is_from_future().unwrap());
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

        // This build knows one more migration (exercises): opening the old file
        // reports an upgradable schema — the case main/UI prompts on.
        let mut db = Database::open(&scratch.path).unwrap();
        assert_eq!(
            db.status().unwrap(),
            DbStatus::Upgradeable { from: 1, to: 2 }
        );

        let outcome = db.upgrade().unwrap();
        assert_eq!(outcome.applied, 1);
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
    fn newer_database_is_rejected_as_too_new() {
        let scratch = Scratch::new("future.sqlite");

        // A future build knew three migrations, one more than this build, so
        // the file is left at a schema version we don't understand (v3).
        let future_defs = [
            M::up("CREATE TABLE a (x INTEGER);"),
            M::up("CREATE TABLE b (x INTEGER);"),
            M::up("CREATE TABLE c (x INTEGER);"),
        ];
        let future = Migrations::from_slice(&future_defs);
        let mut conn = Connection::open(&scratch.path).unwrap();
        future.to_latest(&mut conn).unwrap();
        drop(conn);

        let mut db = Database::open(&scratch.path).unwrap();
        assert_eq!(db.status().unwrap(), DbStatus::FromFuture { version: 3 });
        assert!(db.is_from_future().unwrap());
        assert!(
            matches!(db.upgrade(), Err(DbError::TooNew { .. })),
            "must refuse to downgrade a newer database"
        );
    }
}
