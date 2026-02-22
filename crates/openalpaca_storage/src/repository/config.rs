use crate::Database;
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;

/// Repository for system configuration (system_config table).
pub struct ConfigRepository<'a> {
    db: &'a Database,
}

impl<'a> ConfigRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Get a configuration value as a boolean.
    pub fn get_bool(&self, key: &str) -> Result<bool> {
        match self.get(key)? {
            Some(v) => Ok(v == "true"),
            None => Ok(false),
        }
    }

    /// Get a configuration value by key.
    pub fn get(&self, key: &str) -> Result<Option<String>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare("SELECT value FROM system_config WHERE key = ?")?;
            let value: Option<String> = stmt
                .query_row([key], |row| row.get(0))
                .optional()
                .context("Failed to get config value")?;
            Ok(value)
        })
    }

    /// Set a configuration value (Upsert).
    pub fn set(&self, key: &str, value: &str, kind: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO system_config (key, value, kind, updated_at) 
                 VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
                 ON CONFLICT(key) DO UPDATE SET 
                 value = excluded.value, 
                 kind = excluded.kind,
                 updated_at = excluded.updated_at",
                (key, value, kind),
            )
            .context("Failed to set config value")?;
            Ok(())
        })
    }

    /// Delete a configuration key.
    pub fn delete(&self, key: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute("DELETE FROM system_config WHERE key = ?", [key])
                .context("Failed to delete config value")?;
            Ok(())
        })
    }

    /// List all configuration items.
    /// Returns Vec<(key, value, kind)>
    pub fn list(&self) -> Result<Vec<(String, String, String)>> {
        self.db.with_connection(|conn| {
            let mut stmt =
                conn.prepare("SELECT key, value, kind FROM system_config ORDER BY key ASC")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;

            let mut entries = Vec::new();
            for row in rows {
                entries.push(row?);
            }
            Ok(entries)
        })
    }

    /// Clear all configuration items.
    pub fn clear_all(&self) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute("DELETE FROM system_config", [])
                .context("Failed to clear config")?;
            Ok(())
        })
    }

    /// Set a config value after validating against the schema.
    ///
    /// Only allows writes for `SystemConfig`-backed keys (SQLite).
    /// File-backed keys (`LlmToml`, `DaemonToml`) are rejected with guidance
    /// to use the CLI or edit the file directly.
    ///
    /// Use this for programmatic writes that should be validated. For daemon
    /// internal writes that skip validation, use `set()` directly.
    pub fn set_checked(&self, key: &str, value: &str) -> Result<()> {
        use crate::config_schema;
        use crate::config_schema::ConfigBackend;

        let def = config_schema::lookup(key).ok_or_else(|| {
            anyhow::anyhow!(
                "Unknown config key '{}'. Run 'openalpaca config list --all' to see valid keys.",
                key
            )
        })?;

        // Guard against writes for file-backed keys
        match def.backend {
            ConfigBackend::LlmToml => {
                anyhow::bail!(
                    "Key '{}' is managed by config/llm.toml. \
                     Use `openalpaca config set {} <value>` (CLI) or edit the file directly.",
                    key,
                    key
                );
            }
            ConfigBackend::DaemonToml => {
                anyhow::bail!(
                    "Key '{}' is managed by config/daemon.toml. \
                     Use `openalpaca config set {} <value>` (CLI) or edit the file directly.",
                    key,
                    key
                );
            }
            ConfigBackend::SystemConfig => {} // proceed
        }

        config_schema::validate(key, value)
            .map_err(|e| anyhow::anyhow!("Invalid value for '{}': {}", key, e))?;
        let normalized = config_schema::normalize(key, value);
        let kind = def.kind.as_db_kind();
        self.set(key, &normalized, kind)
    }

    /// Get a config value, falling back to schema default if not in DB.
    pub fn get_or_default(&self, key: &str) -> Result<Option<String>> {
        if let Some(val) = self.get(key)? {
            return Ok(Some(val));
        }
        use crate::config_schema;
        Ok(config_schema::lookup(key).and_then(|def| def.default.map(|d| d.to_string())))
    }
}
