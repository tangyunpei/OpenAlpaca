//! Database management for OpenAlpaca
//!
//! Provides connection pooling and migration management using rusqlite.

use crate::migrations::{self, Migration};
use anyhow::{Context, Result};
use rusqlite::Connection;
use sqlite_vec::sqlite3_vec_init;
use std::path::Path;
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
    VEC_INIT.call_once(|| {
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite3_vec_init as *const (),
            )));
        }
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

        db.run_migrations()?;

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
    fn run_migrations(&self) -> Result<()> {
        let current_version = self.schema_version()?;
        debug!("Current schema version: {}", current_version);

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

        tx.commit().context("Failed to commit migration transaction")?;

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

    /// Wipe all content from the database (Factory Reset)
    pub fn factory_reset(&self) -> Result<()> {
        self.with_connection_mut(|conn| {
            let tx = conn
                .transaction()
                .context("Failed to begin factory_reset transaction")?;

            // Note: SQLite doesn't have TRUNCATE, so we use DELETE.
            // Order matters because FKs are ON.

            // 0. LLM Usage (no FKs, safe to delete first)
            tx.execute("DELETE FROM llm_call_log", [])?;
            tx.execute("DELETE FROM llm_usage_daily", [])?;

            // 0. SubAgent System (FK to agent and task)
            tx.execute("DELETE FROM agent_task_history", [])?;
            tx.execute("DELETE FROM agent_metrics", [])?;

            // 1. Task System
            tx.execute("DELETE FROM task_agent_assignment", [])?;
            tx.execute("DELETE FROM task", [])?;

            // 1. Identity, Config & Preference System
            tx.execute("DELETE FROM preference", [])?;
            tx.execute("DELETE FROM conversation_map", [])?;
            tx.execute("DELETE FROM link_token", [])?;
            tx.execute("DELETE FROM external_identity", [])?;
            tx.execute("DELETE FROM global_user", [])?;
            tx.execute("DELETE FROM system_config", [])?;

            // 2. Connector & Agent data
            // Triggers on memory should clean up memory_fts automatically
            tx.execute("DELETE FROM memory", [])?;
            tx.execute("DELETE FROM memory_vec", [])?;
            tx.execute("DELETE FROM event_log", [])?;
            tx.execute("DELETE FROM agent", [])?;

            tx.commit()
                .context("Failed to commit factory_reset transaction")?;

            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_database_creation() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let db = Database::open(&db_path).unwrap();
        assert!(db_path.exists());
        assert_eq!(db.schema_version().unwrap(), 21);
    }

    #[test]
    fn test_migrations_idempotent() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        // Open twice - migrations should only run once
        let _db1 = Database::open(&db_path).unwrap();
        let db2 = Database::open(&db_path).unwrap();

        assert_eq!(db2.schema_version().unwrap(), 21);
    }

    #[test]
    fn test_foreign_keys_enforced() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();

        // Insert task_agent_assignment with nonexistent task_id should fail (FK)
        let result = db.with_connection(|c| {
            c.execute(
                "INSERT INTO task_agent_assignment(id, task_id, agent_id, role, status)
                 VALUES ('a1', 'nonexistent-task', 'agent-1', 'test', 'pending')",
                [],
            )?;
            Ok(())
        });

        assert!(result.is_err(), "Expected foreign key constraint error");
    }

    #[test]
    fn test_sqlite_vec_available() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        db.with_connection(|conn| {
            // 1. Verify extension loaded
            let version: String = conn.query_row(
                "SELECT vec_version()", [], |row| row.get(0)
            )?;
            assert!(!version.is_empty(), "vec_version() should return a version string");

            // 2. Verify migration created the table
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='memory_vec')",
                [], |row| row.get(0),
            )?;
            assert!(exists, "memory_vec table should exist after migration");

            // 3. Insert a zero vector (768 floats x 4 bytes = 3072 bytes of zeroblob)
            conn.execute(
                "INSERT INTO memory_vec(memory_id, embedding) VALUES (1, vec_f32(zeroblob(3072)))",
                [],
            )?;

            // 4. Verify round-trip
            let count: i64 = conn.query_row(
                "SELECT count(*) FROM memory_vec", [], |row| row.get(0)
            )?;
            assert_eq!(count, 1);

            Ok(())
        }).unwrap();
    }

    #[test]
    fn test_fts_update_sync() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();

        db.with_connection(|c| {
            // Insert v2 memory with "old" keyword
            c.execute(
                "INSERT INTO memory(owner_id, kind, scope, scope_id, source, content, content_hash)
                 VALUES ('owner-1', 'fact', 'global', '', 'conversation', 'old keyword here', 'hash1')",
                [],
            )?;
            Ok(())
        }).unwrap();

        // Update content to "new" keyword
        db.with_connection(|c| {
            c.execute(
                "UPDATE memory SET content = 'new keyword here', content_hash = 'hash2' WHERE owner_id='owner-1'",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        // Verify FTS sync: old should not match, new should match
        db.with_connection(|c| {
            let old_hits: i64 = c.query_row(
                "SELECT count(*) FROM memory_fts WHERE content MATCH 'old'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(old_hits, 0, "Old keyword should NOT be found after update");

            let new_hits: i64 = c.query_row(
                "SELECT count(*) FROM memory_fts WHERE content MATCH 'new'",
                [],
                |r| r.get(0),
            )?;
            assert!(new_hits >= 1, "New keyword should be found after update");
            Ok(())
        })
        .unwrap();
    }
}
