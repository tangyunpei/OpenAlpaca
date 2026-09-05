//! Tests for the tri-state permissions store (design §2.2, §5, §5.1).

use super::*;
use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard};
use tempfile::TempDir;

/// `atomic_write_toml` rotates the version it replaces into `state/backups/`,
/// and the encrypted-secret path reads `state/.master_key`; both resolve through
/// `OPENALPACA_HOME_STORE` on **every** call. No test in this crate ever touches
/// the real `~/.openalpaca`.
///
/// The lock is crate-wide on purpose: `manager::tests` sandboxes the same
/// variable, and two modules with a lock each would serialize only against
/// themselves — one module's guard could re-point the store out from under the
/// other's write. It is a plain `std` mutex held across `.await` points, which
/// `#[tokio::test]`'s current-thread runtime allows.
pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct HomeStoreGuard {
    _lock: MutexGuard<'static, ()>,
    prev: Option<OsString>,
}

impl HomeStoreGuard {
    pub(crate) fn set(path: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::fs::create_dir_all(path).unwrap();
        let prev = std::env::var_os(openalpaca_storage::store::HOME_STORE_ENV);
        // SAFETY: serialized by ENV_LOCK; these are the only tests in this
        // module that touch the variable.
        unsafe { std::env::set_var(openalpaca_storage::store::HOME_STORE_ENV, path) };
        Self { _lock: lock, prev }
    }
}

impl Drop for HomeStoreGuard {
    fn drop(&mut self) {
        // SAFETY: as above — still holding ENV_LOCK.
        match self.prev.take() {
            Some(v) => unsafe { std::env::set_var(openalpaca_storage::store::HOME_STORE_ENV, v) },
            None => unsafe { std::env::remove_var(openalpaca_storage::store::HOME_STORE_ENV) },
        }
    }
}

fn gate(tmp: &TempDir) -> (PermissionGate, HomeStoreGuard) {
    let home = HomeStoreGuard::set(&tmp.path().join(".home"));
    (PermissionGate::new(tmp.path()), home)
}

#[test]
fn approve_and_deny_round_trip() {
    let tmp = TempDir::new().unwrap();
    let (gate, _home) = gate(&tmp);

    assert_eq!(gate.is_approved("my-plugin"), None);

    gate.approve("my-plugin", &["network".into(), "filesystem.read".into()])
        .unwrap();
    assert_eq!(gate.is_approved("my-plugin"), Some(true));
    assert_eq!(
        gate.load_table().unwrap().recorded_capabilities("my-plugin"),
        vec!["network".to_string(), "filesystem.read".to_string()]
    );

    gate.deny("my-plugin").unwrap();
    let table = gate.load_table().unwrap();
    assert_eq!(table.approved("my-plugin"), Some(false));
    assert!(
        table.recorded_capabilities("my-plugin").is_empty(),
        "deny left the approved capability list in place"
    );
}

/// **The decision-less entry** (design §5.1, §4): a plugin the owner switched
/// off before it was ever approved must keep that bit across a restart, which
/// means an entry with **no** `approved` key at all.
#[test]
fn set_enabled_writes_an_entry_with_no_consent_decision() {
    let tmp = TempDir::new().unwrap();
    let (gate, _home) = gate(&tmp);

    gate.set_enabled("never-seen", false).unwrap();

    let raw = std::fs::read_to_string(tmp.path().join(".permissions.toml")).unwrap();
    assert!(raw.contains("enabled = false"), "{raw}");
    assert!(
        !raw.contains("approved"),
        "a toggle wrote a consent decision: {raw}"
    );

    let table = gate.load_table().unwrap();
    assert_eq!(table.approved("never-seen"), None, "pending, not denied");
    assert!(!table.enabled("never-seen"));
}

/// Consent and the toggle are independent, and each mutation preserves the
/// other — the whole point of splitting the field (design §2.2).
#[test]
fn consent_and_the_toggle_never_overwrite_each_other() {
    let tmp = TempDir::new().unwrap();
    let (gate, _home) = gate(&tmp);

    gate.set_enabled("p", false).unwrap();
    gate.approve("p", &["net".into()]).unwrap();
    let table = gate.load_table().unwrap();
    assert_eq!(table.approved("p"), Some(true));
    assert!(!table.enabled("p"), "approve reset the owner's toggle");

    gate.set_enabled("p", true).unwrap();
    let table = gate.load_table().unwrap();
    assert!(table.enabled("p"));
    assert_eq!(
        table.approved("p"),
        Some(true),
        "the toggle revoked the consent decision"
    );

    gate.deny("p").unwrap();
    let table = gate.load_table().unwrap();
    assert_eq!(table.approved("p"), Some(false));
    assert!(table.enabled("p"), "deny cleared the owner's toggle");
}

/// Existing files parse unchanged: a bare `approved = true` with no `enabled`
/// key reads as approved and on (design §2.2).
#[test]
fn a_pre_existing_entry_reads_as_approved_and_enabled() {
    let tmp = TempDir::new().unwrap();
    let (gate, _home) = gate(&tmp);
    std::fs::write(
        tmp.path().join(".permissions.toml"),
        "[legacy]\napproved = true\napproved_at = \"2026-01-01T00:00:00Z\"\ncapabilities = [\"net\"]\n",
    )
    .unwrap();

    let table = gate.load_table().unwrap();
    assert_eq!(table.approved("legacy"), Some(true));
    assert!(table.enabled("legacy"), "the serde default is not `true`");
    assert_eq!(table.recorded_capabilities("legacy"), vec!["net".to_string()]);
}

/// **Fail-closed on corruption, open on absence** (design §5.1).
#[test]
fn a_corrupt_store_is_an_error_and_a_missing_one_is_empty() {
    let tmp = TempDir::new().unwrap();
    let (gate, _home) = gate(&tmp);

    // Absent: an empty table, exactly as before. Row 1 of §5.1 depends on it —
    // otherwise every fresh install would park at `Failed{ConfigInvalid}`.
    assert!(gate.load_table().unwrap().entry("anything").is_none());

    let garbage = "[oops\napproved = yes";
    std::fs::write(tmp.path().join(".permissions.toml"), garbage).unwrap();
    assert!(
        matches!(gate.load_table(), Err(PluginError::StoreUnreadable(_))),
        "one malformed line still lost every approval"
    );

    // And no write may touch it: the refusal happens before the edit.
    assert!(gate.set_enabled("oops", false).is_err());
    assert!(gate.approve("oops", &[]).is_err());
    assert!(gate.deny("oops").is_err());
    assert_eq!(
        std::fs::read_to_string(tmp.path().join(".permissions.toml")).unwrap(),
        garbage,
        "the unreadable store was overwritten"
    );
}

/// The writer preserves a hand-authored file's comments and its other entries
/// (design §2.1).
#[test]
fn a_write_preserves_comments_and_every_other_entry() {
    let tmp = TempDir::new().unwrap();
    let (gate, _home) = gate(&tmp);
    let authored = "# The owner's own note.\n\
                    [alpha]\n\
                    approved = true\n\
                    approved_at = \"2026-01-01T00:00:00Z\"\n\
                    \n\
                    # beta was refused on purpose.\n\
                    [beta]\n\
                    approved = false\n\
                    approved_at = \"2026-01-02T00:00:00Z\"\n";
    std::fs::write(tmp.path().join(".permissions.toml"), authored).unwrap();

    gate.set_enabled("alpha", false).unwrap();

    let raw = std::fs::read_to_string(tmp.path().join(".permissions.toml")).unwrap();
    assert!(raw.contains("# The owner's own note."), "{raw}");
    assert!(raw.contains("# beta was refused on purpose."), "{raw}");
    let table = gate.load_table().unwrap();
    assert_eq!(table.approved("beta"), Some(false));
    assert!(!table.enabled("alpha"));
    assert_eq!(table.approved("alpha"), Some(true));
}

/// Ordinary config values are stored and read back as themselves; only a
/// reference is redacted (design §8, X-29).
#[test]
fn plugin_config_round_trips_and_redacts_only_references() {
    let tmp = TempDir::new().unwrap();
    let (gate, _home) = gate(&tmp);

    assert!(gate.load_plugin_config("p").is_empty());

    gate.set_plugin_config("p", "rate_limit", toml::Value::Integer(100))
        .unwrap();
    gate.set_plugin_secret_reference("p", "api_key", SecretReference::Encrypted("aes256:xx".into()))
        .unwrap();

    let stored = gate.load_plugin_config("p");
    assert_eq!(stored.get("rate_limit"), Some(&toml::Value::Integer(100)));
    assert_eq!(
        secret_reference(stored.get("api_key").unwrap()),
        Some(SecretReference::Encrypted("aes256:xx".into()))
    );

    let shown = gate.redacted_plugin_config("p");
    assert_eq!(shown.get("rate_limit"), Some(&toml::Value::Integer(100)));
    assert_eq!(
        shown.get("api_key"),
        Some(&toml::Value::String(REDACTED.to_string())),
        "a secret reference was served verbatim"
    );
}
