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
    /// The dotted paths of the keys this `plugin.toml` sets that OpenAlpaca
    /// does not read (`tool`, `plugin.entrypoint`, `config.api_key.requird`),
    /// filled by [`Self::parse`]. A manifest built any other way reads none.
    ///
    /// A record, never a refusal: the struct has no `deny_unknown_fields`, so
    /// an unknown key is ignored — and [`Self::warnings`] says so (N-PLUG-2).
    #[serde(skip)]
    pub unknown_keys: Vec<String>,
}

/// The keys `[plugin]` holds. `env` is a map whose keys are the author's.
const PLUGIN_KEYS: &[&str] = &[
    "name",
    "version",
    "description",
    "author",
    "license",
    "entry",
    "args",
    "env",
    "max_concurrent",
    "mcp_compatible",
];

/// The switches `[types]` holds.
const TYPES_KEYS: &[&str] = &["tools", "connector", "provider", "skill", "agent"];

/// The keys one `[config.<name>]` table holds; `<name>` is the author's.
const CONFIG_FIELD_KEYS: &[&str] = &["type", "required", "default", "description", "sensitive"];

/// Every key path a manifest may set outside the two author-named maps, in
/// the order a "did you mean" offers them.
const KNOWN_PATHS: &[&str] = &[
    "types.tools",
    "types.connector",
    "types.provider",
    "types.skill",
    "types.agent",
    "capabilities.provides",
    "capabilities.virtual.provides",
    "plugin.name",
    "plugin.version",
    "plugin.description",
    "plugin.author",
    "plugin.license",
    "plugin.entry",
    "plugin.args",
    "plugin.env",
    "plugin.max_concurrent",
    "plugin.mcp_compatible",
];

/// The warning for `types.connector` while no plugin connector is wired.
pub const CONNECTOR_NOT_WIRED: &str = "plugin.toml declares types.connector, but connectors are \
     not supported: the plugin connector bridge is not wired, so nothing this plugin offers as a \
     connector is used";

/// The warning for `types.provider` while no plugin provider is wired.
pub const PROVIDER_NOT_WIRED: &str = "plugin.toml declares types.provider, but the plugin \
     provider bridge is not wired: OpenAlpaca does not use a plugin as a model provider, so \
     nothing this plugin offers as one is used";

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
        Self::parse(&content)
    }

    /// Parse a manifest's text, recording the keys it sets that nothing reads
    /// in [`Self::unknown_keys`].
    ///
    /// The struct is deserialized exactly as before, so every refusal and its
    /// message are unchanged; only a manifest that parses is then walked again
    /// as a plain table against the keys the struct reads. No
    /// `deny_unknown_fields`: an unknown key is a warning, never a refusal
    /// (N-PLUG-2).
    pub fn parse(text: &str) -> Result<Self, PluginError> {
        let mut manifest: PluginManifest =
            toml::from_str(text).map_err(|e| PluginError::InvalidManifest(e.to_string()))?;
        let table: toml::Table =
            toml::from_str(text).map_err(|e| PluginError::InvalidManifest(e.to_string()))?;
        manifest.unknown_keys = unknown_keys(&table);
        Ok(manifest)
    }

    /// What this manifest declares that OpenAlpaca will not act on, one
    /// sentence each: every key it does not read ([`Self::unknown_keys`]), and
    /// `types.connector` / `types.provider` while their bridges are unwired.
    ///
    /// Warnings, never refusals (N-PLUG-2): the validate dry run and the
    /// install preview carry them on the summary, install, update and approve
    /// on the row's `warnings`, and a load logs them. A key is the author's
    /// text, so it is shown with every hidden character as `<U+XXXX>` and the
    /// approval surfaces' count under it.
    pub fn warnings(&self) -> Vec<String> {
        use openalpaca_core::utils::text::{escape_hidden_line, hidden_notice};

        let mut warnings: Vec<String> = self
            .unknown_keys
            .iter()
            .map(|path| {
                let shown = escape_hidden_line(path);
                let mut warning = match suggestion(path) {
                    Some(known) => format!(
                        "plugin.toml sets '{}', which OpenAlpaca does not read, so it is \
                         ignored — did you mean '{known}'?",
                        shown.text
                    ),
                    None => format!(
                        "plugin.toml sets '{}', which OpenAlpaca does not read, so it is ignored",
                        shown.text
                    ),
                };
                if let Some(notice) = hidden_notice(shown.hidden) {
                    // Its own sentence, after the warning's: `… is ignored.
                    // Note: …`, or `… mean '…'? Note: …`.
                    if !warning.ends_with('?') {
                        warning.push('.');
                    }
                    warning.push(' ');
                    warning.push_str(&notice);
                }
                warning
            })
            .collect();
        if self.types.connector {
            warnings.push(CONNECTOR_NOT_WIRED.to_string());
        }
        if self.types.provider {
            warnings.push(PROVIDER_NOT_WIRED.to_string());
        }
        warnings
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

/// The dotted path of every key in `table` that [`PluginManifest`] does not
/// read, in the manifest's own key order within each table.
///
/// Only called on text that already deserialized, so every known table here
/// is a table; `plugin.env` and the names under `[config]` are the author's
/// and never reported.
fn unknown_keys(table: &toml::Table) -> Vec<String> {
    let mut unknown = Vec::new();
    for (key, value) in table {
        match key.as_str() {
            "plugin" => unknown_in(value, "plugin", PLUGIN_KEYS, &mut unknown),
            "types" => unknown_in(value, "types", TYPES_KEYS, &mut unknown),
            "capabilities" => {
                for (cap_key, cap_value) in value.as_table().into_iter().flatten() {
                    match cap_key.as_str() {
                        "provides" => {}
                        "virtual" => unknown_in(
                            cap_value,
                            "capabilities.virtual",
                            &["provides"],
                            &mut unknown,
                        ),
                        other => unknown.push(format!("capabilities.{other}")),
                    }
                }
            }
            "config" => {
                for (name, field) in value.as_table().into_iter().flatten() {
                    unknown_in(
                        field,
                        &format!("config.{name}"),
                        CONFIG_FIELD_KEYS,
                        &mut unknown,
                    );
                }
            }
            other => unknown.push(other.to_string()),
        }
    }
    unknown
}

/// The keys of the table `value`, under `prefix`, that `known` does not list.
fn unknown_in(value: &toml::Value, prefix: &str, known: &[&str], unknown: &mut Vec<String>) {
    for key in value.as_table().into_iter().flat_map(|t| t.keys()) {
        if !known.contains(&key.as_str()) {
            unknown.push(format!("{prefix}.{key}"));
        }
    }
}

/// The key an unknown path most likely meant: the first known path whose last
/// segment matches the unknown one's, ignoring case, `-` for `_` and a plural
/// `s` — `tool` and `types.tool` → `types.tools`, `skills` → `types.skill`,
/// `config.api_key.requird` → nothing. Inside `[config.<name>]` the match is
/// against that table's own fields.
fn suggestion(path: &str) -> Option<String> {
    fn stem(segment: &str) -> String {
        let lower = segment.to_lowercase().replace('-', "_");
        lower.strip_suffix('s').map(str::to_string).unwrap_or(lower)
    }
    let (parent, leaf) = match path.rsplit_once('.') {
        Some((parent, leaf)) => (Some(parent), leaf),
        None => (None, path),
    };
    let wanted = stem(leaf);
    if let Some(parent) = parent
        && parent.starts_with("config.")
    {
        return CONFIG_FIELD_KEYS
            .iter()
            .find(|field| stem(field) == wanted)
            .map(|field| format!("{parent}.{field}"));
    }
    KNOWN_PATHS
        .iter()
        .find(|known| {
            let known_leaf = known.rsplit('.').next().unwrap_or(known);
            stem(known_leaf) == wanted && **known != path
        })
        .map(|known| known.to_string())
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

    // ── N-PLUG-2: what a manifest says that nothing reads ────────────

    const MINIMAL: &str = "[plugin]\nname = \"p\"\nversion = \"1\"\nentry = \"./p\"\n";

    /// Every unknown key is reported by its dotted path, wherever it sits;
    /// `plugin.env` and the names under `[config]` are the author's.
    #[test]
    fn parse_records_every_key_nothing_reads_by_its_path() {
        let text = format!(
            "tool = true\n{MINIMAL}entrypoint = \"./x\"\n[plugin.env]\nANY = \"1\"\n\
             [capabilities]\nprovides = []\nprovide = []\n[capabilities.virtual]\nprovides = []\n\
             extra = 1\n[types]\ntools = true\ntool = true\n\
             [config.api_key]\ntype = \"secret\"\nrequird = true\n"
        );
        let manifest = PluginManifest::parse(&text).expect("an unknown key is not a refusal");
        let mut unknown = manifest.unknown_keys.clone();
        unknown.sort();
        assert_eq!(
            unknown,
            vec![
                "capabilities.provide",
                "capabilities.virtual.extra",
                "config.api_key.requird",
                "plugin.entrypoint",
                "tool",
                "types.tool",
            ]
        );
        assert!(manifest.types.tools, "the known keys still read as before");
    }

    /// A clean manifest has no unknown keys and, with no unwired type, no
    /// warning.
    #[test]
    fn a_clean_manifest_has_no_warning() {
        let manifest = PluginManifest::parse(MINIMAL).unwrap();
        assert!(manifest.unknown_keys.is_empty());
        assert!(manifest.warnings().is_empty());
    }

    /// The warning names the key and, when one plainly matches, the key it
    /// most likely meant.
    #[test]
    fn a_warning_names_the_key_and_what_it_likely_meant() {
        let manifest = PluginManifest::parse(&format!("tool = true\n{MINIMAL}")).unwrap();
        assert_eq!(
            manifest.warnings(),
            vec![
                "plugin.toml sets 'tool', which OpenAlpaca does not read, so it is ignored — \
                 did you mean 'types.tools'?"
                    .to_string()
            ]
        );
        assert_eq!(suggestion("skills").as_deref(), Some("types.skill"));
        assert_eq!(suggestion("types.tool").as_deref(), Some("types.tools"));
        assert_eq!(
            suggestion("config.api_key.types").as_deref(),
            Some("config.api_key.type")
        );
        assert_eq!(suggestion("plugin.entrypoint"), None);
    }

    /// The key is the author's text: a hidden character in it is shown as
    /// `<U+XXXX>` and counted in the approval surfaces' words.
    #[test]
    fn a_hidden_character_in_a_key_is_shown_and_counted() {
        // A bidi override and a zero-width space, the two every new approval
        // surface's test plants.
        let manifest =
            PluginManifest::parse(&format!("\"to\u{202e}ol\u{200b}\" = true\n{MINIMAL}")).unwrap();
        let warnings = manifest.warnings();
        assert_eq!(
            warnings,
            vec![
                "plugin.toml sets 'to<U+202E>ol<U+200B>', which OpenAlpaca does not read, so \
                 it is ignored. Note: this request contains 2 hidden characters, shown as \
                 <U+…>."
                    .to_string()
            ]
        );
        assert!(!warnings[0].contains('\u{202e}') && !warnings[0].contains('\u{200b}'));
    }

    /// `types.connector` and `types.provider` warn while their bridges are
    /// unwired — and only those two types.
    #[test]
    fn the_unwired_types_warn_and_the_wired_ones_do_not() {
        let manifest = PluginManifest::parse(&format!(
            "{MINIMAL}[types]\ntools = true\nskill = true\nagent = true\nconnector = true\n\
             provider = true\n"
        ))
        .unwrap();
        assert_eq!(
            manifest.warnings(),
            vec![
                CONNECTOR_NOT_WIRED.to_string(),
                PROVIDER_NOT_WIRED.to_string()
            ]
        );
    }

    /// A refusal reads exactly as before: the struct is deserialized first.
    #[test]
    fn a_manifest_that_does_not_parse_is_still_refused() {
        let error = PluginManifest::parse("[plugin]\nname = \"p\"\n").unwrap_err();
        assert!(error.to_string().contains("missing field"), "{error}");
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
