//! Database management for OpenAlpaca
//!
//! Provides connection pooling and migration management using rusqlite.

use crate::migrations::{self, Migration};
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{debug, info};

/// Database manager wrapping a SQLite connection
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Open or create a database at the given path, running any pending migrations.
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

        db.run_migrations()?;

        info!("Database initialized: {}", path.display());
        Ok(db)
    }

    /// Get the current schema version
    pub fn schema_version(&self) -> Result<i32> {
        let conn = self.conn.lock().unwrap();

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

        let conn = self.conn.lock().unwrap();

        for migration in pending {
            info!(
                "Running migration {}: {}",
                migration.version, migration.name
            );
            conn.execute_batch(migration.sql)
                .with_context(|| format!("Failed to run migration {}", migration.name))?;
        }

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
        let conn = self.conn.lock().unwrap();
        f(&conn)
    }

    /// Execute a function with mutable access to the connection (for transactions)
    pub fn with_connection_mut<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T>,
    {
        let mut conn = self.conn.lock().unwrap();
        f(&mut conn)
    }

    /// Wipe all content from the database (Factory Reset)
    pub fn factory_reset(&self) -> Result<()> {
        self.with_connection(|conn| {
            // Disable foreign keys temporarily to allow truncating in any order,
            // though we try to do it effectively.
            // conn.execute("PRAGMA foreign_keys = OFF", [])?;

            // Note: SQLite doesn't have TRUNCATE, so we use DELETE.
            // Order matters if FKs are ON.

            // 0. LLM Usage (no FKs, safe to delete first)
            conn.execute("DELETE FROM llm_call_log", [])?;
            conn.execute("DELETE FROM llm_usage_daily", [])?;

            // 0. SubAgent System (FK to agent and task)
            conn.execute("DELETE FROM agent_task_history", [])?;
            conn.execute("DELETE FROM agent_metrics", [])?;

            // 1. Task System
            conn.execute("DELETE FROM task_agent_assignment", [])?;
            conn.execute("DELETE FROM task", [])?;

            // 1. Identity, Config & Preference System
            conn.execute("DELETE FROM preference", [])?;
            conn.execute("DELETE FROM conversation_map", [])?;
            conn.execute("DELETE FROM link_token", [])?;
            conn.execute("DELETE FROM external_identity", [])?;
            conn.execute("DELETE FROM global_user", [])?;
            conn.execute("DELETE FROM system_config", [])?;

            // 2. Connector & Agent data
            // Triggers on memory should clean up memory_fts automatically
            conn.execute("DELETE FROM memory", [])?;
            conn.execute("DELETE FROM event_log", [])?;
            conn.execute("DELETE FROM agent", [])?;

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
        assert_eq!(db.schema_version().unwrap(), 8);
    }

    #[test]
    fn test_migrations_idempotent() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        // Open twice - migrations should only run once
        let _db1 = Database::open(&db_path).unwrap();
        let db2 = Database::open(&db_path).unwrap();

        assert_eq!(db2.schema_version().unwrap(), 8);
    }

    #[test]
    fn test_foreign_keys_enforced() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();

        // Insert memory without agent should fail (foreign key constraint)
        let result = db.with_connection(|c| {
            c.execute(
                "INSERT INTO memory(agent_id, role, content) VALUES ('no-agent', 'user', 'hi')",
                [],
            )?;
            Ok(())
        });

        assert!(result.is_err(), "Expected foreign key constraint error");
    }

    #[test]
    fn test_fts_update_sync() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();

        db.with_connection(|c| {
            // Create agent
            c.execute("INSERT INTO agent(id, name) VALUES ('a1', 'Test')", [])?;
            // Insert memory with "old" keyword
            c.execute(
                "INSERT INTO memory(agent_id, role, content) VALUES ('a1', 'user', 'old keyword here')",
                [],
            )?;
            Ok(())
        }).unwrap();

        // Update content to "new" keyword
        db.with_connection(|c| {
            c.execute(
                "UPDATE memory SET content = 'new keyword here' WHERE agent_id='a1'",
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
