//! `<plugins root>/.permissions.toml` — the plugin disposition **and** consent
//! store (extension design §2.2, §5).
//!
//! Two independent facts live in one entry, and keeping them independent is the
//! whole point of this file:
//!
//! * **`enabled`** is the owner's toggle. `#[serde(default = "default_true")]`,
//!   so every existing file reads as enabled.
//! * **`approved`** is the consent decision, and it is **tri-state**:
//!   `Some(true)` approved, `Some(false)` denied, `None` *pending*. A
//!   never-approved plugin whose toggle the owner pre-set to off must keep that
//!   bit across a restart, which means writing an entry that carries **no**
//!   consent decision — impossible while `approved` was a bare `bool` (§4).
//!
//! Existing files parse unchanged: a bare `approved = true` deserialises into
//! `Some(true)`.
//!
//! Two behaviours changed with the shape:
//!
//! 1. **The store fails closed** (§5.1). A parse error used to warn and return
//!    an empty table, losing every approval; with `enabled` in the same file it
//!    would additionally re-enable every integration the owner turned off. It
//!    now returns `Err`, the file is never overwritten, and a copy is kept for
//!    repair. A *missing* file still reads as an empty table — fail-closed on
//!    corruption, open on absence.
//! 2. **Writes are atomic and entry-preserving** (§2.1, §5). Every mutation is a
//!    surgical `toml_edit` assignment through `atomic_write_toml`: comments
//!    survive, the result is re-parsed with this module's own parser before the
//!    rename, and the previous version is rotated into `state/backups/`. The old
//!    `approve`/`deny` inserted a *fresh* entry, which would have reset
//!    `enabled` to its default on every consent decision.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use openalpaca_core::config_io::{atomic_write_toml, copy_unparseable_once};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::PluginError;

/// What a redacting read shows in place of a sensitive value (design §8, X-29).
pub const REDACTED: &str = "<redacted>";

/// The two reference forms a sensitive plugin config value may take. The value
/// itself never lands in `.config/<plugin>.toml`; only the reference does.
const SECRET_REF_KEY: &str = "secret_ref";
const SECRET_ENCRYPTED_KEY: &str = "secret_encrypted";

fn default_true() -> bool {
    true
}

/// Persisted disposition + consent for a single plugin, keyed by its
/// **directory** name (design §2.2, X-3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionEntry {
    /// The owner's toggle. Defaulted, so every pre-existing entry reads as on.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The consent decision. `None` is *pending* — for a missing entry and for
    /// a decision-less one alike.
    ///
    /// `skip_serializing_if` is declared rather than inherited from the TOML
    /// serializer's None-skipping behaviour, because the `{enabled = false}`
    /// entry §5.1 depends on has to serialise by construction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<String>,
    /// The capability list recorded **at approval time**. Written since day one
    /// and never read back until the E1 drift check (design §3.3 E1).
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl Default for PermissionEntry {
    fn default() -> Self {
        Self {
            enabled: true,
            approved: None,
            approved_at: None,
            capabilities: Vec::new(),
        }
    }
}

/// A parsed `.permissions.toml`.
///
/// The reads a supervisor needs are on this type rather than on
/// [`PermissionGate`] so a verb parses the store **once** and answers consent,
/// the bit and the recorded capability list from the same snapshot — the three
/// facts the §6.2 #7 gate reads together.
#[derive(Debug, Clone, Default)]
pub struct PermissionTable {
    entries: HashMap<String, PermissionEntry>,
}

impl PermissionTable {
    pub fn entry(&self, plugin: &str) -> Option<&PermissionEntry> {
        self.entries.get(plugin)
    }

    /// The consent decision: `None` for a missing entry **and** for a
    /// decision-less one, so the §6.2 #7 gate reads the same `NeverSeen`
    /// either way.
    pub fn approved(&self, plugin: &str) -> Option<bool> {
        self.entries.get(plugin).and_then(|e| e.approved)
    }

    /// The owner's toggle. An absent entry reads as `true` — the serde default
    /// an entry would have had (design §5.1, row 1).
    pub fn enabled(&self, plugin: &str) -> bool {
        self.entries.get(plugin).map(|e| e.enabled).unwrap_or(true)
    }

    /// Every plugin the store carries an entry for.
    ///
    /// The scan's vanished set is computed from this **union** the in-memory
    /// map: at a cold start the map is empty, so the entry is the only thing
    /// that remembers a plugin whose directory is gone, and without it
    /// `Orphaned` would have no boot trigger at all (design §5.1 row 2).
    pub fn names(&self) -> Vec<&str> {
        self.entries.keys().map(|k| k.as_str()).collect()
    }

    /// The capability list recorded at approval, for the E1 drift check.
    pub fn recorded_capabilities(&self, plugin: &str) -> Vec<String> {
        self.entries
            .get(plugin)
            .map(|e| e.capabilities.clone())
            .unwrap_or_default()
    }
}

/// Manages plugin disposition, first-load approval and user-provided plugin
/// configuration persistence.
///
/// - Permissions file: `<plugin_dir>/.permissions.toml`
/// - Config directory: `<plugin_dir>/.config/`
pub struct PermissionGate {
    permissions_path: PathBuf,
    config_dir: PathBuf,
}

impl PermissionGate {
    /// Create a new `PermissionGate` rooted at `plugin_dir`.
    pub fn new(plugin_dir: &Path) -> Self {
        Self {
            permissions_path: plugin_dir.join(".permissions.toml"),
            config_dir: plugin_dir.join(".config"),
        }
    }

    pub fn permissions_path(&self) -> &Path {
        &self.permissions_path
    }

    pub fn config_path_for(&self, plugin_name: &str) -> PathBuf {
        self.config_dir.join(format!("{plugin_name}.toml"))
    }

    // ── queries ──────────────────────────────────────────────────────

    /// **The store read, fail-closed** (design §5.1).
    ///
    /// A parse error returns `Err`: every plugin then parks at
    /// `Failed{ConfigInvalid, "permissions store unreadable"}`, the file is
    /// never overwritten so the owner can repair it, and it is copied once to
    /// `state/backups/.permissions.toml.unparseable-<ts>`. A **missing** file
    /// still yields an empty table exactly as before — without that
    /// distinction every fresh install would park at `Failed{ConfigInvalid}`.
    pub fn load_table(&self) -> Result<PermissionTable, PluginError> {
        match std::fs::read_to_string(&self.permissions_path) {
            Ok(content) => match parse_permissions(&content) {
                Ok(entries) => Ok(PermissionTable { entries }),
                Err(e) => {
                    warn!(
                        path = %self.permissions_path.display(),
                        error = %e,
                        "permissions store is unreadable; refusing to load any plugin"
                    );
                    let copy = copy_unparseable_once(&self.permissions_path);
                    Err(PluginError::StoreUnreadable(match copy {
                        Some(path) => format!("{e} (copy kept at {})", path.display()),
                        None => e,
                    }))
                }
            },
            // Absent is not corrupt: a fresh install has no file yet, and §5.1
            // row 1 depends on that reading. **Only** absent, though — any
            // other read error (a directory in the file's place, a permissions
            // problem) is a store nobody can read, and fail-closed means the
            // same refusal a parse error gets.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PermissionTable::default()),
            Err(e) => {
                warn!(
                    path = %self.permissions_path.display(),
                    error = %e,
                    "permissions store cannot be read; refusing to load any plugin"
                );
                Err(PluginError::StoreUnreadable(e.to_string()))
            }
        }
    }

    /// Check whether a plugin has been approved, denied, or never seen.
    ///
    /// `None` covers a missing entry, a decision-less entry **and** an
    /// unreadable store. A caller that must tell the last of those apart —
    /// every supervisor verb does, because it is `409 store_unreadable` rather
    /// than a transition — uses [`load_table`](Self::load_table).
    pub fn is_approved(&self, plugin_name: &str) -> Option<bool> {
        self.load_table().ok()?.approved(plugin_name)
    }

    // ── mutations ────────────────────────────────────────────────────

    /// Record an approval for `plugin_name` against the capability list it
    /// declares **now**.
    ///
    /// A read-modify-write: `enabled` is not in the edit, so the owner's toggle
    /// position survives a consent decision (design §5).
    pub fn approve(
        &self,
        plugin_name: &str,
        capabilities: &[String],
    ) -> Result<(), PluginError> {
        self.write_consent(plugin_name, true, capabilities)?;
        debug!(plugin = plugin_name, "plugin approved");
        Ok(())
    }

    /// Record a denial for `plugin_name`. `capabilities` is cleared and
    /// `enabled` is deliberately left untouched, so a later approve restores
    /// the owner's last toggle position (design §3.2 W-deny).
    pub fn deny(&self, plugin_name: &str) -> Result<(), PluginError> {
        self.write_consent(plugin_name, false, &[])?;
        debug!(plugin = plugin_name, "plugin denied");
        Ok(())
    }

    /// Write the owner's toggle, **creating a decision-less entry** when the
    /// plugin has none.
    ///
    /// That entry — `{enabled = false}` with no `approved` key — is what makes a
    /// pre-set bit on a never-approved plugin survive a restart while the row
    /// still reads `never_seen` (design §5.1, §4).
    pub fn set_enabled(&self, plugin_name: &str, enabled: bool) -> Result<(), PluginError> {
        self.edit(plugin_name, |entry| {
            entry["enabled"] = toml_edit::value(enabled);
        })?;
        debug!(plugin = plugin_name, enabled, "plugin disposition written");
        Ok(())
    }

    /// Remove a plugin's entry entirely — the **only** path that ever deletes
    /// one (design §5.1, §8's `DELETE /v1/extensions/plugin/{id}`).
    ///
    /// Reconciles never delete: a vanished directory parks as `Orphaned` with
    /// its disposition and consent preserved, because deleting the entry would
    /// silently flip the plugin back on the next time the directory reappears.
    /// Only the owner's explicit Remove gets here.
    pub fn remove_entry(&self, plugin_name: &str) -> Result<(), PluginError> {
        self.load_table()?;
        let owned = plugin_name.to_string();
        atomic_write_toml(
            &self.permissions_path,
            move |doc| {
                doc.remove(owned.as_str());
                Ok(())
            },
            |rendered| parse_permissions(rendered).map(|_| ()),
        )
        .map_err(|e| {
            PluginError::StoreWriteFailed(format!("{}: {e}", self.permissions_path.display()))
        })?;
        debug!(plugin = plugin_name, "plugin permissions entry removed");
        Ok(())
    }

    fn write_consent(
        &self,
        plugin_name: &str,
        approved: bool,
        capabilities: &[String],
    ) -> Result<(), PluginError> {
        let now = Utc::now().to_rfc3339();
        let mut caps = toml_edit::Array::new();
        for cap in capabilities {
            caps.push(cap.as_str());
        }
        self.edit(plugin_name, move |entry| {
            entry["approved"] = toml_edit::value(approved);
            entry["approved_at"] = toml_edit::value(now);
            entry["capabilities"] = toml_edit::value(caps);
        })
    }

    /// One surgical, atomic edit of one entry.
    ///
    /// Fail-closed first: a store that does not parse refuses the write before
    /// anything is touched, which is what makes the supervisor's step W a
    /// `409 store_unreadable` rather than a `500` against a file nobody can
    /// read (design §4, §5.1).
    fn edit<F>(&self, plugin_name: &str, edit: F) -> Result<(), PluginError>
    where
        F: FnOnce(&mut toml_edit::Item),
    {
        self.load_table()?;
        let owned = plugin_name.to_string();
        atomic_write_toml(
            &self.permissions_path,
            move |doc| {
                // Explicit, not auto-vivified: a decision-less entry must
                // render as its own `[plugin]` header so the file still reads
                // as a table of entries after the round trip.
                let item = doc
                    .entry(owned.as_str())
                    .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                if let Some(table) = item.as_table_mut() {
                    table.set_implicit(false);
                }
                edit(item);
                Ok(())
            },
            |rendered| parse_permissions(rendered).map(|_| ()),
        )
        .map_err(|e| {
            PluginError::StoreWriteFailed(format!(
                "{}: {e}",
                self.permissions_path.display()
            ))
        })
    }

    // ── plugin config ────────────────────────────────────────────────

    /// The plugin's configuration **as stored** — sensitive values appear as
    /// their reference table, never as plaintext, because that is all that was
    /// ever written (design §8, X-29).
    ///
    /// Returns an empty map if the config file does not exist.
    pub fn load_plugin_config(&self, plugin_name: &str) -> HashMap<String, toml::Value> {
        let path = self.config_path_for(plugin_name);
        match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str::<HashMap<String, toml::Value>>(&content)
                .unwrap_or_else(|e| {
                    warn!(
                        plugin = plugin_name,
                        error = %e,
                        "failed to parse plugin config, returning empty"
                    );
                    HashMap::new()
                }),
            Err(_) => HashMap::new(),
        }
    }

    /// The plugin's configuration with every secret reference replaced by
    /// [`REDACTED`] — what a `GET` on the config route serves (design §8).
    pub fn redacted_plugin_config(&self, plugin_name: &str) -> HashMap<String, toml::Value> {
        self.load_plugin_config(plugin_name)
            .into_iter()
            .map(|(key, value)| {
                if secret_reference(&value).is_some() {
                    (key, toml::Value::String(REDACTED.to_string()))
                } else {
                    (key, value)
                }
            })
            .collect()
    }

    /// Set (or overwrite) a single **non-sensitive** key.
    ///
    /// The write goes through `atomic_write_toml`, replacing the bare
    /// `fs::write` this file used to do: a crash mid-write can no longer
    /// truncate a plugin's configuration.
    ///
    /// Sensitivity is the *manifest's* fact, so the refusal for a sensitive key
    /// lives on `PluginManager::set_plugin_config`, which has the manifest in
    /// hand; this function is the storage half.
    pub fn set_plugin_config(
        &self,
        plugin_name: &str,
        key: &str,
        value: toml::Value,
    ) -> Result<(), PluginError> {
        let item = toml_value_to_item(&value);
        self.edit_config(plugin_name, key, item)?;
        debug!(plugin = plugin_name, key, "plugin config updated");
        Ok(())
    }

    /// Store a **reference** to a sensitive value. The value itself never
    /// reaches the TOML (design §8, X-29).
    pub fn set_plugin_secret_reference(
        &self,
        plugin_name: &str,
        key: &str,
        reference: SecretReference,
    ) -> Result<(), PluginError> {
        let mut table = toml_edit::InlineTable::new();
        table.insert(reference.field(), reference.value().into());
        self.edit_config(plugin_name, key, toml_edit::value(table))?;
        debug!(
            plugin = plugin_name,
            key,
            store = reference.field(),
            "plugin secret reference stored"
        );
        Ok(())
    }

    fn edit_config(
        &self,
        plugin_name: &str,
        key: &str,
        item: toml_edit::Item,
    ) -> Result<(), PluginError> {
        let path = self.config_path_for(plugin_name);
        let owned = key.to_string();
        atomic_write_toml(
            &path,
            move |doc| {
                doc[owned.as_str()] = item;
                Ok(())
            },
            |rendered| {
                toml::from_str::<HashMap<String, toml::Value>>(rendered)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            },
        )
        .map_err(|e| PluginError::StoreWriteFailed(format!("{}: {e}", path.display())))
    }
}

/// Where a sensitive plugin config value is actually kept. **Which of the two
/// is the default is design §13 Q12 and is not decided here** — the caller
/// says, so nothing picks one by omission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretReference {
    /// OS keychain, by reference key — resolved through a
    /// `openalpaca_llm::SecretStore`.
    Keychain(String),
    /// AES-256-GCM under `state/.master_key`, the ciphertext stored inline.
    Encrypted(String),
}

impl SecretReference {
    fn field(&self) -> &'static str {
        match self {
            Self::Keychain(_) => SECRET_REF_KEY,
            Self::Encrypted(_) => SECRET_ENCRYPTED_KEY,
        }
    }

    fn value(&self) -> &str {
        match self {
            Self::Keychain(v) | Self::Encrypted(v) => v,
        }
    }
}

/// Read a stored value as a secret reference, or `None` when it is an ordinary
/// value.
pub fn secret_reference(value: &toml::Value) -> Option<SecretReference> {
    let table = value.as_table()?;
    if let Some(r) = table.get(SECRET_REF_KEY).and_then(|v| v.as_str()) {
        return Some(SecretReference::Keychain(r.to_string()));
    }
    table
        .get(SECRET_ENCRYPTED_KEY)
        .and_then(|v| v.as_str())
        .map(|c| SecretReference::Encrypted(c.to_string()))
}

/// The one parser. The writer re-parses its rendered document with exactly this
/// function before the rename, so a write can never leave a store the reader
/// would refuse.
fn parse_permissions(content: &str) -> Result<HashMap<String, PermissionEntry>, String> {
    toml::from_str::<HashMap<String, PermissionEntry>>(content).map_err(|e| e.to_string())
}

fn toml_value_to_item(value: &toml::Value) -> toml_edit::Item {
    // `toml_edit` and `toml` are separate value trees; a one-key round trip
    // through text is the supported bridge, and this runs once per config write.
    let mut wrapper = toml::map::Map::new();
    wrapper.insert("v".to_string(), value.clone());
    toml::to_string(&toml::Value::Table(wrapper))
        .ok()
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .map(|doc| doc["v"].clone())
        .unwrap_or_else(|| toml_edit::value(value.to_string()))
}

/// `pub(crate)` because `manager::tests` shares this module's
/// `OPENALPACA_HOME_STORE` guard — one lock for the whole crate.
#[cfg(test)]
pub(crate) mod tests;
