use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::PluginError;

/// Persisted permission entry for a single plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PermissionEntry {
    approved: bool,
    approved_at: String,
    #[serde(default)]
    capabilities: Vec<String>,
}

/// Manages first-load plugin approval, denial, and user-provided plugin
/// configuration persistence.
///
/// Approvals are stored in `.permissions.toml` inside the plugin directory.
/// Per-plugin configuration lives under `.config/<plugin_name>.toml`.
pub struct PermissionGate {
    permissions_path: PathBuf,
    config_dir: PathBuf,
}

impl PermissionGate {
    /// Create a new `PermissionGate` rooted at `plugin_dir`.
    ///
    /// - Permissions file: `<plugin_dir>/.permissions.toml`
    /// - Config directory: `<plugin_dir>/.config/`
    pub fn new(plugin_dir: &Path) -> Self {
        Self {
            permissions_path: plugin_dir.join(".permissions.toml"),
            config_dir: plugin_dir.join(".config"),
        }
    }

    // ── queries ──────────────────────────────────────────────────────

    /// Check whether a plugin has been approved, denied, or never seen.
    ///
    /// Returns `None` if the plugin has no recorded decision, `Some(true)` if
    /// approved, `Some(false)` if denied.
    pub fn is_approved(&self, plugin_name: &str) -> Option<bool> {
        let table = self.load_permissions_table();
        table.get(plugin_name).map(|entry| entry.approved)
    }

    // ── mutations ────────────────────────────────────────────────────

    /// Record an approval for `plugin_name` with the given capability list.
    pub fn approve(
        &self,
        plugin_name: &str,
        capabilities: &[String],
    ) -> Result<(), PluginError> {
        let mut table = self.load_permissions_table();
        table.insert(
            plugin_name.to_string(),
            PermissionEntry {
                approved: true,
                approved_at: Utc::now().to_rfc3339(),
                capabilities: capabilities.to_vec(),
            },
        );
        self.save_permissions_table(&table)?;
        debug!(plugin = plugin_name, "plugin approved");
        Ok(())
    }

    /// Record a denial for `plugin_name`.
    pub fn deny(&self, plugin_name: &str) -> Result<(), PluginError> {
        let mut table = self.load_permissions_table();
        table.insert(
            plugin_name.to_string(),
            PermissionEntry {
                approved: false,
                approved_at: Utc::now().to_rfc3339(),
                capabilities: Vec::new(),
            },
        );
        self.save_permissions_table(&table)?;
        debug!(plugin = plugin_name, "plugin denied");
        Ok(())
    }

    // ── plugin config ────────────────────────────────────────────────

    /// Load the user-provided configuration for a plugin.
    ///
    /// Returns an empty map if the config file does not exist.
    pub fn load_plugin_config(
        &self,
        plugin_name: &str,
    ) -> HashMap<String, toml::Value> {
        let path = self.config_dir.join(format!("{plugin_name}.toml"));
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                toml::from_str::<HashMap<String, toml::Value>>(&content)
                    .unwrap_or_else(|e| {
                        warn!(
                            plugin = plugin_name,
                            error = %e,
                            "failed to parse plugin config, returning empty"
                        );
                        HashMap::new()
                    })
            }
            Err(_) => HashMap::new(),
        }
    }

    /// Set (or overwrite) a single key in a plugin's configuration file.
    ///
    /// Creates the `.config/` directory and the file if they do not exist yet.
    pub fn set_plugin_config(
        &self,
        plugin_name: &str,
        key: &str,
        value: toml::Value,
    ) -> Result<(), PluginError> {
        std::fs::create_dir_all(&self.config_dir)?;

        let path = self.config_dir.join(format!("{plugin_name}.toml"));
        let mut map = self.load_plugin_config(plugin_name);
        map.insert(key.to_string(), value);

        let content = toml::to_string_pretty(&map)
            .map_err(|e| PluginError::InvalidManifest(format!("toml serialize: {e}")))?;
        std::fs::write(&path, content)?;
        debug!(plugin = plugin_name, key, "plugin config updated");
        Ok(())
    }

    // ── internal helpers ─────────────────────────────────────────────

    fn load_permissions_table(&self) -> HashMap<String, PermissionEntry> {
        match std::fs::read_to_string(&self.permissions_path) {
            Ok(content) => {
                toml::from_str::<HashMap<String, PermissionEntry>>(&content)
                    .unwrap_or_else(|e| {
                        warn!(
                            error = %e,
                            "failed to parse .permissions.toml, starting fresh"
                        );
                        HashMap::new()
                    })
            }
            Err(_) => HashMap::new(),
        }
    }

    fn save_permissions_table(
        &self,
        table: &HashMap<String, PermissionEntry>,
    ) -> Result<(), PluginError> {
        if let Some(parent) = self.permissions_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(table)
            .map_err(|e| PluginError::InvalidManifest(format!("toml serialize: {e}")))?;
        std::fs::write(&self.permissions_path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_approve_and_check() {
        let tmp = TempDir::new().unwrap();
        let gate = PermissionGate::new(tmp.path());

        // Never-seen plugin returns None
        assert_eq!(gate.is_approved("my-plugin"), None);

        // Approve it
        gate.approve("my-plugin", &["network".into(), "filesystem.read".into()])
            .unwrap();

        // Now returns Some(true)
        assert_eq!(gate.is_approved("my-plugin"), Some(true));

        // Verify the persisted file is valid TOML with the expected fields
        let raw = std::fs::read_to_string(tmp.path().join(".permissions.toml")).unwrap();
        let table: HashMap<String, PermissionEntry> = toml::from_str(&raw).unwrap();
        let entry = table.get("my-plugin").unwrap();
        assert!(entry.approved);
        assert_eq!(entry.capabilities, vec!["network", "filesystem.read"]);
    }

    #[test]
    fn test_deny() {
        let tmp = TempDir::new().unwrap();
        let gate = PermissionGate::new(tmp.path());

        gate.deny("bad-plugin").unwrap();
        assert_eq!(gate.is_approved("bad-plugin"), Some(false));
    }

    #[test]
    fn test_plugin_config() {
        let tmp = TempDir::new().unwrap();
        let gate = PermissionGate::new(tmp.path());

        // Empty config for unknown plugin
        let cfg = gate.load_plugin_config("my-plugin");
        assert!(cfg.is_empty());

        // Set a value
        gate.set_plugin_config("my-plugin", "api_key", toml::Value::String("sk-123".into()))
            .unwrap();

        // Read it back
        let cfg = gate.load_plugin_config("my-plugin");
        assert_eq!(
            cfg.get("api_key"),
            Some(&toml::Value::String("sk-123".into()))
        );

        // Overwrite and add another key
        gate.set_plugin_config("my-plugin", "rate_limit", toml::Value::Integer(100))
            .unwrap();
        let cfg = gate.load_plugin_config("my-plugin");
        assert_eq!(cfg.get("api_key"), Some(&toml::Value::String("sk-123".into())));
        assert_eq!(cfg.get("rate_limit"), Some(&toml::Value::Integer(100)));
    }
}
