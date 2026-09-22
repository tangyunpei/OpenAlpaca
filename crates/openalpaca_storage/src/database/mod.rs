//! Database management for OpenAlpaca
//!
//! Provides a shared connection and transactional migrations using rusqlite.

use crate::migrations::{self, Migration};
use anyhow::{Context, Result};
use rusqlite::{Connection, TransactionBehavior};
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

        // Connection-local settings may be applied before validating the database.
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            PRAGMA busy_timeout = 5000;
            PRAGMA temp_store = MEMORY;
            "#,
        )
        .context("Failed to apply PRAGMA settings")?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        db.run_migrations(path, migrations::MIGRATIONS)?;
        // WAL persists in the file: enable it only after accepting the schema.
        db.with_connection(|conn| {
            conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
            Ok(())
        })?;

        info!("Database initialized: {}", path.display());
        Ok(db)
    }

    /// Get the current schema version
    pub fn schema_version(&self) -> Result<i32> {
        let conn = self.conn.lock().unwrap_or_else(|p| {
            error!("Database mutex poisoned, recovering");
            p.into_inner()
        });

        // The status API keeps its schema_version name; the ledger is runner-owned.
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
            [],
            |row| row.get(0),
        )?;

        if !exists {
            return Ok(0);
        }

        let version: i32 = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;

        Ok(version)
    }

    /// Validate the ledger and apply its missing suffix in one transaction.
    fn run_migrations(&self, path: &Path, migrations: &[Migration]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap_or_else(|p| {
            error!("Database mutex poisoned, recovering");
            p.into_inner()
        });

        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("Failed to begin migration transaction")?;
        let has_ledger: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
            [],
            |row| row.get(0),
        )?;
        let applied = if has_ledger {
            let versions = tx
                .prepare("SELECT version FROM schema_migrations ORDER BY version")?
                .query_map([], |row| row.get::<_, i32>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            anyhow::ensure!(
                !versions.is_empty()
                    && versions.len() <= migrations.len()
                    && versions
                        .iter()
                        .zip(migrations)
                        .all(|(v, m)| *v == m.version),
                "Unsupported migration history in {}: expected a prefix of this build's \
                 migrations (latest version {}). Use a compatible build or back up and \
                 remove this database to start fresh.",
                std::path::absolute(path)
                    .unwrap_or_else(|_| path.to_path_buf())
                    .display(),
                migrations.last().map_or(0, |m| m.version),
            );
            versions.len()
        } else {
            let populated: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name NOT GLOB 'sqlite_*')",
                [],
                |row| row.get(0),
            )?;
            anyhow::ensure!(
                !populated,
                "Unsupported untracked database at {}: a fresh database is required. \
                 Back up and remove this database together with its -wal and -shm files \
                 to start fresh; this discards its stored data. No automatic import is supported.",
                std::path::absolute(path)
                    .unwrap_or_else(|_| path.to_path_buf())
                    .display(),
            );
            tx.execute_batch(
                "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY CHECK(version > 0));",
            )?;
            0
        };

        debug!("Already applied {} migrations", applied);
        for migration in &migrations[applied..] {
            info!(
                "Running migration {}: {}",
                migration.version, migration.name
            );
            tx.execute_batch(migration.sql)
                .with_context(|| format!("Failed to run migration {}", migration.name))?;
            tx.execute(
                "INSERT INTO schema_migrations (version) VALUES (?1)",
                [migration.version],
            )?;
        }

        tx.commit()
            .context("Failed to commit migration transaction")?;

        info!(
            "Migrations complete, schema version: {}",
            migrations.last().map(|m| m.version).unwrap_or(0)
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

            // 0. File assets (FK: message_attachments -> file_assets, conversation_messages)
            tx.execute("DELETE FROM conversation_message_attachments", [])?;
            // 036: versions reference file_assets (ON DELETE CASCADE) — named
            // for the same reason `subagent_span` is below: the reset does not
            // depend on foreign_keys being enabled.
            tx.execute("DELETE FROM artifact_versions", [])?;
            tx.execute("DELETE FROM file_assets", [])?;

            // 0. Conversation history (children first: feedback -> messages -> conversations)
            tx.execute("DELETE FROM message_feedback", [])?;
            tx.execute("DELETE FROM conversation_messages", [])?;
            // Migration 039 rebuilt `conversations` as `session`.
            tx.execute("DELETE FROM session", [])?;
            // 033: queued follow-ups reference no table, so nothing cascades
            // them away. Left behind, `GatewayFollowupRunner` would fire them
            // as turns against the database the user just wiped.
            tx.execute("DELETE FROM lane_followups", [])?;

            // 0. LLM Usage (no FKs, safe to delete first)
            tx.execute("DELETE FROM llm_call_log", [])?;
            tx.execute("DELETE FROM llm_usage_daily", [])?;

            // 0. Telemetry / analytics logs (standalone, no FKs)
            tx.execute("DELETE FROM discovered_models", [])?;
            tx.execute("DELETE FROM orchestrator_latency", [])?;
            tx.execute("DELETE FROM dispatch_decisions", [])?;
            tx.execute("DELETE FROM skill_execution_log", [])?;
            tx.execute("DELETE FROM tool_execution_log", [])?;

            // 0. SubAgent System (FK to agent and task)
            tx.execute("DELETE FROM agent_task_history", [])?;
            tx.execute("DELETE FROM agent_metrics", [])?;

            // 1. Task System
            tx.execute("DELETE FROM task_agent_assignment", [])?;
            // 037: spans reference task (ON DELETE CASCADE) — explicit so the
            // reset does not depend on foreign_keys being enabled.
            tx.execute("DELETE FROM subagent_span", [])?;
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
mod tests;
