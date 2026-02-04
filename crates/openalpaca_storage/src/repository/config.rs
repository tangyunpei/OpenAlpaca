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
}
