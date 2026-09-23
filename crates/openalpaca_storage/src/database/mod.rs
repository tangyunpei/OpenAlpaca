//! Database management for OpenAlpaca
//!
//! Provides connection pooling and migration management using rusqlite.

use crate::migrations::{self, Migration};
use anyhow::{Context, Result};
use rusqlite::Connection;
use sqlite_vec::sqlite3_vec_init;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};
use tracing::{debug, error, info};

static VEC_INIT: Once = Once::new();

/// Register sqlite-vec extension globally (process-wide, idempotent).
///
/// Uses `sqlite3_auto_extension` so every `Connection::open()` gets vec functions.
/// The `transmute` converts `sqlite3_vec_init` (which has the sqlite3 extension
/// entry point signature) into the `Option<unsafe extern "C" fn()>` that
/// `sqlite3_auto_extension` expects. This is the documented pattern from
/// the sqlite-vec crate.
fn ensure_vec_extension() {
    VEC_INIT.call_once(|| unsafe {
        #[allow(clippy::missing_transmute_annotations)]
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite3_vec_init as *const (),
        )));
    });
}

/// Database manager wrapping a SQLite connection
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Open or create a database at the given path, running any pending migrations.
    pub fn open(path: &Path) -> Result<Self> {
        ensure_vec_extension();

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
    /// `path` is the database file this connection was opened from; it is used
    /// only to name the file in the legacy-refusal message, which is otherwise
    /// unactionable — the store root is overridable with `OPENALPACA_HOME_STORE`,
    /// so "this database" identifies nothing.
    fn run_migrations(&self, path: &Path) -> Result<()> {
        let current_version = self.schema_version()?;
        debug!("Current schema version: {}", current_version);

        // The baseline creates the final schema directly; it cannot upgrade a
        // partially migrated development database. Refuse before executing SQL
        // so its existing schema and rows remain available to an older build.
        //
        // The message names both the verb and the file. `openalpaca config
        // reset --factory` is reachable from here because it opens no database
        // at all — it deletes exactly the three files `delete_database_files`
        // below names, and the next open sees version 0 and gets the baseline.
        // The path is still spelled out because the store root is overridable
        // with `OPENALPACA_HOME_STORE`, so "your database" identifies nothing.
        if current_version > 0 && current_version < migrations::BASELINE_VERSION {
            let file = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
            anyhow::bail!(
                "Unsupported legacy schema version {current_version}: this build starts at schema \
                 version {baseline} and cannot upgrade a database created by an older one. \
                 Run `openalpaca config reset --factory` to delete {file} (with its -wal and \
                 -shm files) and start again at schema version {baseline}; that discards every \
                 task, message and memory it holds, and leaves your artifact and upload files on \
                 disk with nothing pointing at them. To keep any of it, open {file} with a build \
                 from before the migrations were squashed into the baseline and export it first.",
                baseline = migrations::BASELINE_VERSION,
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
