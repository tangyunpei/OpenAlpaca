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
    pub provides: Vec<String>,
    // NEW P3e:
    #[serde(default, rename = "virtual")]
    pub virtual_: VirtualCapabilitiesSection,
}

/// Plugin-declared virtual capabilities (MCP P3e).
///
/// Every tool registered by this plugin gets these cap names added to its
/// virtual-capability set via a synthesized `PluginCapabilityProvider`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct VirtualCapabilitiesSection {
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
    /// A value that must never land in `plugins/.config/<name>.toml`
    /// (extension design §8, X-29). The TOML stores only a reference —
    /// `secret_ref` (OS keychain) or `secret_encrypted` (AES-256-GCM under
    /// `state/.master_key`) — and a read of the config redacts it.
    ///
    /// Defaults to `false`, so every existing manifest is unchanged.
    #[serde(default)]
    pub sensitive: bool,
}

/// How a `plugin.toml` that turns out to be a symlink is treated. R66: the
/// regular-file requirement belongs to the **untrusted-source** inspection
/// (install/update) — [`PluginManifest::from_dir`] — where a symlink is
/// refused outright, same as any other non-regular file. The **load** path —
/// [`PluginManifest::from_dir_for_load`] — is more permissive: a plugin
/// directory the owner assembled by hand may hold a `plugin.toml` symlinked
/// from elsewhere in the same directory (a dev setup), so only a symlink that
/// escapes the directory, or one that resolves to something other than a
/// regular file, is refused there.
#[derive(Clone, Copy)]
enum SymlinkPolicy {
    Refuse,
    AllowInTree,
}

impl PluginManifest {
    /// Parse a `plugin.toml` from an **untrusted source** — an install or
    /// update payload nobody has vetted yet ([`crate::install`]'s
    /// `inspect_source`/`inspect_update_source`).
    ///
    /// **The type is checked before the open.** A source directory is whatever
    /// the caller pointed at, and `read_to_string` on a FIFO with no writer
    /// never returns — a `mkfifo plugin.toml` inside an unpacked third-party
    /// plugin used to park this read forever, with the file-type check that
    /// would have caught it living downstream in `install::copy_tree`, which
    /// the manifest read runs *before*. `symlink_metadata` is the check that
    /// does not follow a link, so a symlinked `plugin.toml` is refused too:
    /// what an install copies in is a regular file, and that is what an
    /// untrusted source must hold. (The **load** path is less strict about
    /// symlinks — see [`Self::from_dir_for_load`].)
    pub fn from_dir(plugin_dir: &Path) -> Result<Self, PluginError> {
        Self::read(plugin_dir, SymlinkPolicy::Refuse)
    }

    /// Parse a `plugin.toml` from an **installed** plugin directory — the
    /// load path (a boot/reconcile scan, or a verb's fresh re-read of its
    /// declaration).
    ///
    /// R66: a `plugin.toml` symlink is accepted here as long as it resolves
    /// to a regular file **inside the plugin's own directory** — a dev setup
    /// that manages its manifest as a symlink. Anything else non-regular
    /// (FIFO, socket, device) is refused, same as [`Self::from_dir`], and so
    /// is a symlink that escapes the directory.
    pub fn from_dir_for_load(plugin_dir: &Path) -> Result<Self, PluginError> {
        Self::read(plugin_dir, SymlinkPolicy::AllowInTree)
    }

    fn read(plugin_dir: &Path, symlink_policy: SymlinkPolicy) -> Result<Self, PluginError> {
        let manifest_path = plugin_dir.join("plugin.toml");
        let Ok(meta) = std::fs::symlink_metadata(&manifest_path) else {
            return Err(PluginError::ManifestNotFound(
                manifest_path.display().to_string(),
            ));
        };
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            match symlink_policy {
                SymlinkPolicy::Refuse => {
                    return Err(PluginError::InvalidManifest(format!(
                        "'{}' is not a regular file (a symlink), and a manifest must be one",
                        manifest_path.display()
                    )));
                }
                SymlinkPolicy::AllowInTree => {
                    Self::check_in_tree_symlink(plugin_dir, &manifest_path)?;
                }
            }
        } else if !file_type.is_file() {
            return Err(PluginError::InvalidManifest(format!(
                "'{}' is not a regular file ({}), and a manifest must be one",
                manifest_path.display(),
                crate::install::describe(file_type)
            )));
        }
        let content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| PluginError::InvalidManifest(e.to_string()))?;
        let manifest: PluginManifest = toml::from_str(&content)
            .map_err(|e| PluginError::InvalidManifest(e.to_string()))?;
        Ok(manifest)
    }

    /// R66's load-path symlink check: resolve the whole chain and require the
    /// real path to stay inside `plugin_dir`, then require what it names to
    /// be a regular file — a FIFO/socket/device reached *through* a symlink
    /// is exactly as much a manifest as one sitting there directly.
    fn check_in_tree_symlink(plugin_dir: &Path, manifest_path: &Path) -> Result<(), PluginError> {
        let resolved = std::fs::canonicalize(manifest_path).map_err(|e| {
            PluginError::InvalidManifest(format!(
                "'{}' is a symlink that could not be resolved: {e}",
                manifest_path.display()
            ))
        })?;
        let plugin_dir = std::fs::canonicalize(plugin_dir).map_err(|e| {
            PluginError::InvalidManifest(format!(
                "'{}' could not be resolved: {e}",
                plugin_dir.display()
            ))
        })?;
        if !resolved.starts_with(&plugin_dir) {
            return Err(PluginError::InvalidManifest(format!(
                "'{}' is a symlink that escapes the plugin directory",
                manifest_path.display()
            )));
        }
        let target_type = std::fs::metadata(&resolved)
            .map_err(|e| PluginError::InvalidManifest(e.to_string()))?
            .file_type();
        if !target_type.is_file() {
            return Err(PluginError::InvalidManifest(format!(
                "'{}' resolves to '{}', which is not a regular file ({}), and a manifest must be one",
                manifest_path.display(),
                resolved.display(),
                crate::install::describe(target_type)
            )));
        }
        Ok(())
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

#[cfg(test)]
mod p3e_tests {
    use super::*;

    fn parse_manifest(toml_text: &str) -> PluginManifest {
        toml::from_str(toml_text).expect("parse manifest")
    }

    #[test]
    fn manifest_without_virtual_section_defaults_empty() {
        let toml = r#"
            [plugin]
            name = "test"
            version = "1.0.0"
            entry = "./bin"
        "#;
        let m = parse_manifest(toml);
        assert!(m.capabilities.virtual_.provides.is_empty());
    }

    #[test]
    fn manifest_with_virtual_provides_parses_correctly() {
        let toml = r#"
            [plugin]
            name = "test"
            version = "1.0.0"
            entry = "./bin"

            [capabilities.virtual]
            provides = ["annotation:x", "plugin:y"]
        "#;
        let m = parse_manifest(toml);
        assert_eq!(m.capabilities.virtual_.provides.len(), 2);
        assert!(m.capabilities.virtual_.provides.contains(&"annotation:x".to_string()));
        assert!(m.capabilities.virtual_.provides.contains(&"plugin:y".to_string()));
    }

    #[test]
    fn manifest_virtual_empty_provides_list() {
        let toml = r#"
            [plugin]
            name = "test"
            version = "1.0.0"
            entry = "./bin"

            [capabilities.virtual]
            provides = []
        "#;
        let m = parse_manifest(toml);
        assert!(m.capabilities.virtual_.provides.is_empty());
    }
}
