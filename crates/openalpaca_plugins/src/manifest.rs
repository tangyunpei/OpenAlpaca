use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::PluginError;

#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    pub plugin: PluginMeta,
    #[serde(default)]
    pub capabilities: CapabilitiesSection,
    #[serde(default)]
    pub types: TypesSection,
    #[serde(default)]
    pub config: HashMap<String, ConfigField>,
    #[serde(default)]
    pub connector: Option<ConnectorMeta>,
    #[serde(default)]
    pub provider: Option<ProviderMeta>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginMeta {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub license: String,
    pub entry: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,
    #[serde(default)]
    pub mcp_compatible: bool,
}

fn default_max_concurrent() -> usize { 10 }

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CapabilitiesSection {
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TypesSection {
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub connector: bool,
    #[serde(default)]
    pub provider: bool,
    #[serde(default)]
    pub skill: bool,
    #[serde(default)]
    pub agent: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigField {
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<toml::Value>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConnectorMeta {
    pub platform: String,
    #[serde(default)]
    pub supports_files: bool,
    #[serde(default)]
    pub supports_reactions: bool,
    #[serde(default = "default_max_message_length")]
    pub max_message_length: usize,
}

fn default_max_message_length() -> usize { 4096 }

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderMeta {
    #[serde(default)]
    pub supports_streaming: bool,
    #[serde(default)]
    pub supports_tools: bool,
    #[serde(default)]
    pub default_models: Vec<String>,
}

impl PluginManifest {
    /// Parse a plugin.toml file from a plugin directory.
    pub fn from_dir(plugin_dir: &Path) -> Result<Self, PluginError> {
        let manifest_path = plugin_dir.join("plugin.toml");
        if !manifest_path.exists() {
            return Err(PluginError::ManifestNotFound(
                manifest_path.display().to_string(),
            ));
        }
        let content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| PluginError::InvalidManifest(e.to_string()))?;
        let manifest: PluginManifest = toml::from_str(&content)
            .map_err(|e| PluginError::InvalidManifest(e.to_string()))?;
        Ok(manifest)
    }

    /// Return all required config keys that are not yet set.
    pub fn missing_config_keys(&self, provided: &HashMap<String, toml::Value>) -> Vec<String> {
        self.config
            .iter()
            .filter(|(_, field)| field.required && !provided.contains_key(field.description.as_str()))
            .filter(|(key, _)| !provided.contains_key(key.as_str()))
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Resolve the entry command as an absolute path relative to plugin_dir.
    pub fn entry_path(&self, plugin_dir: &Path) -> PathBuf {
        let entry = Path::new(&self.plugin.entry);
        if entry.is_absolute() {
            entry.to_path_buf()
        } else {
            plugin_dir.join(entry)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_manifest() {
        let toml_str = r#"
[plugin]
name = "test-plugin"
version = "0.1.0"
entry = "./test-server"

[types]
tools = true
"#;
        let manifest: PluginManifest = toml::from_str(toml_str).unwrap();
        assert_eq!(manifest.plugin.name, "test-plugin");
        assert!(manifest.types.tools);
        assert!(!manifest.types.connector);
        assert_eq!(manifest.plugin.max_concurrent, 10);
    }

    #[test]
    fn test_missing_config_keys() {
        let toml_str = r#"
[plugin]
name = "test"
version = "0.1.0"
entry = "./test"

[config.api_key]
type = "secret"
required = true
description = "API key"

[config.rate_limit]
type = "number"
required = false
description = "Rate limit"
"#;
        let manifest: PluginManifest = toml::from_str(toml_str).unwrap();
        let provided = HashMap::new();
        let missing = manifest.missing_config_keys(&provided);
        assert_eq!(missing, vec!["api_key"]);
    }
}
