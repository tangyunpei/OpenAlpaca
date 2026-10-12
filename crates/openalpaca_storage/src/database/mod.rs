//! Database management for OpenAlpaca
//!
//! Provides connection pooling and migration management using rusqlite.

use crate::migrations::{self, Migration};
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info};

/// Database manager wrapping a SQLite connection
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Open or create a database at the given path, running any pending migrations.
    ///
    /// No SQLite extension is registered on the way: the baseline declares no
    /// virtual table beyond SQLite itself, and the refusal of a database this
    /// build does not know reads only `schema_version`, which needs no module
    /// either.
    pub fn open(path: &Path) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create database directory: {}", parent.display())
            })?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open database: {}", path.display()))?;

        // PRAGMA settings for optimal performance
        // - journal_mode=WAL: Better concurrency (DB-level, persists)
        // - synchronous=NORMAL: Safe with WAL, faster writes
        // - foreign_keys=ON: Enforce referential integrity
        // - busy_timeout=5000: Wait up to 5s before "database is locked"
        // - temp_store=MEMORY: Faster temp tables for FTS queries
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;
            PRAGMA busy_timeout = 5000;
            PRAGMA temp_store = MEMORY;
            "#,
        )
        .context("Failed to apply PRAGMA settings")?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        db.run_migrations(path)?;

        info!("Database initialized: {}", path.display());
        Ok(db)
    }

    /// Get the current schema version
    pub fn schema_version(&self) -> Result<i32> {
        let conn = self.conn.lock().unwrap_or_else(|p| {
            error!("Database mutex poisoned, recovering");
            p.into_inner()
        });

        // Check if schema_version table exists
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_version')",
            [],
            |row| row.get(0),
        )?;

        if !exists {
            return Ok(0);
        }

        let version: i32 = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )?;

        Ok(version)
    }

    /// Run all pending migrations
    ///
    /// Four cases, decided by the stored `schema_version` before any SQL runs:
    /// a fresh database (version 0: no table, or no row) runs every registered
    /// migration in one transaction and opens at the newest; one below the
    /// newest runs the migrations it is missing; one at the newest returns;
    /// and one **above the newest registered migration** is refused with
    /// nothing changed. That last case is also what a database from before the
    /// schema was re-baselined at version 1 reads as (the old line had reached
    /// 42 before the memory redesign and 45 after it), so the refusal names
    /// both possibilities: a newer build wrote it, or a pre-squash build did
    /// and the factory reset is the way through.
    ///
    /// `path` is the database file this connection was opened from; it is used
    /// only to name the file in the refusal, which is otherwise unactionable —
    /// the store root is overridable with `OPENALPACA_HOME_STORE`, so "this
    /// database" identifies nothing.
    fn run_migrations(&self, path: &Path) -> Result<()> {
        let current_version = self.schema_version()?;
        debug!("Current schema version: {}", current_version);
        let named_file = || std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());

        // A version past the newest registered migration is one this build
        // does not know. Nothing is pending, so without this check the open
        // would succeed and fail later on a table this build has never heard
        // of — or worse, write rows the other schema does not expect. Refuse
        // before executing SQL, so the file's schema and rows stay exactly as
        // they are.
        //
        // Two builds can have written such a file, and the message cannot tell
        // them apart, so it states both: a newer build (use it), or a build from
        // before the schema was re-baselined at version 1 — such a database
        // (42 before the memory redesign, 43 to 45 after it) reads as a version
        // above this build's and can be neither read nor upgraded here. For
        // that one it names `openalpaca config reset --factory`, which is
        // reachable from here because it opens no database at all: it deletes
        // exactly the three files `delete_database_files` below names, and the
        // next open sees version 0 and gets the baseline. The file is spelled
        // out because the store root is overridable with
        // `OPENALPACA_HOME_STORE`, so "your database" identifies nothing.
        // Start-up refusals never name each other's remedies; this one names
        // its own.
        let newest = migrations::MIGRATIONS
            .last()
            .map(|m| m.version)
            .unwrap_or(migrations::BASELINE_VERSION);
        if current_version > newest {
            let file = named_file();
            anyhow::bail!(
                "Database `{file}` is at schema version {current_version}, which this build does \
                 not know (it creates schema version 1 and knows versions up to {newest}). \
                 Nothing was changed. If a newer OpenAlpaca build created this database, use \
                 that build. If it was created by a build from before the schema was \
                 re-baselined at version 1, it cannot be read or upgraded: run \
                 `openalpaca config reset --factory` to start again (it deletes the database \
                 and `llm.toml`'s keys; see the Installation Manual, \"Switching to the \
                 Markdown Memory Build\").",
                file = file.display(),
            );
        }

        let pending: Vec<&Migration> = migrations::MIGRATIONS
            .iter()
            .filter(|m| m.version > current_version)
            .collect();

        if pending.is_empty() {
            debug!("No pending migrations");
            return Ok(());
        }

        let mut conn = self.conn.lock().unwrap_or_else(|p| {
            error!("Database mutex poisoned, recovering");
            p.into_inner()
        });

        let tx = conn
            .transaction()
            .context("Failed to begin migration transaction")?;

        for migration in pending {
            info!(
                "Running migration {}: {}",
                migration.version, migration.name
            );
            tx.execute_batch(migration.sql)
                .with_context(|| format!("Failed to run migration {}", migration.name))?;
        }

        tx.commit()
            .context("Failed to commit migration transaction")?;

        info!(
            "Migrations complete, schema version: {}",
            migrations::MIGRATIONS
                .last()
                .map(|m| m.version)
                .unwrap_or(0)
        );

        Ok(())
    }

    /// Execute a function with exclusive access to the connection
    pub fn with_connection<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self.conn.lock().unwrap_or_else(|p| {
            error!("Database mutex poisoned, recovering");
            p.into_inner()
        });
        f(&conn)
    }

    /// Execute a function with mutable access to the connection (for transactions)
    pub fn with_connection_mut<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T>,
    {
        let mut conn = self.conn.lock().unwrap_or_else(|p| {
            error!("Database mutex poisoned, recovering");
            p.into_inner()
        });
        f(&mut conn)
    }
}

/// Deletes a SQLite database and the two files WAL mode keeps beside it.
///
/// This is what `openalpaca config reset --factory` does, and the reason the
/// verb needs no open `Database`: on a schema this build refuses ([`Database::open`]
/// bails before any SQL runs), deleting the files is the only remedy that works,
/// and the next open sees version 0 and gets the baseline.
///
/// A file that is not there is not an error — a factory reset on a fresh store
/// must succeed, not complain that there was nothing to destroy — and nothing is
/// created on the way: the function only ever removes.
///
/// The sidecars go **first**. A run interrupted between the two leaves the
/// database at the version it was already at, which is where it started; the
/// other order could leave a stale `-wal` sitting beside a database SQLite is
/// about to recreate.
///
/// The two suffixes are spelled out here on purpose rather than shared with
/// any other module: this is the one place in the crate that owns the
/// knowledge "a WAL-mode database is three files".
pub fn delete_database_files(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm"] {
        remove_if_present(&sidecar_path(path, suffix))?;
    }
    remove_if_present(path)
}

/// `openalpaca.db` + `-wal` → `openalpaca.db-wal`. Not an extension change:
/// `Path::set_extension` would produce `openalpaca.-wal`.
fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("Failed to delete {}", path.display())),
    }
}

#[cfg(test)]
mod tests;
