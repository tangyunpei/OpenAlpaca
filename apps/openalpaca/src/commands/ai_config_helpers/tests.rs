//! The CLI's crypto edge.
//!
//! The master key is one file, `~/.openalpaca/state/.master_key`. The daemon
//! resolves it through `store::master_key_dir()`; so must every CLI command, or
//! `openalpaca ai config set-key` writes secrets the daemon cannot read.

use super::llm_config_path;
use crate::commands::ai_config::{get_ai_value, set_ai_value};
use openalpaca_llm::keys::key_encryption::KeyEncryptor;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use tempfile::tempdir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

const SANDBOXED: [&str; 4] = [
    // `directories`' data dir — and therefore the pre-D1 app dir — is derived
    // from HOME, so overriding it confines any stray legacy-root write to the
    // temp dir instead of the developer's real one.
    "HOME",
    "OPENALPACA_HOME_STORE",
    "OPENALPACA_CONFIG_DIR",
    // The CLI never sets this; an inherited value would mask the bug under test.
    "OPENALPACA_MASTER_KEY",
];

struct EnvSandbox {
    _lock: MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvSandbox {
    fn enter(root: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let saved = SANDBOXED
            .iter()
            .map(|var| (*var, std::env::var_os(var)))
            .collect();
        let config = root.join("config");
        std::fs::create_dir_all(&config).unwrap();
        // SAFETY: serialized by ENV_LOCK; these are the only tests in this
        // binary that touch these variables.
        unsafe {
            std::env::set_var("HOME", root);
            std::env::set_var("OPENALPACA_HOME_STORE", root.join("home"));
            std::env::set_var("OPENALPACA_CONFIG_DIR", &config);
            std::env::remove_var("OPENALPACA_MASTER_KEY");
        }
        let sandbox = Self { _lock: lock, saved };

        // Fail before writing anything if the sandbox is not airtight: a test
        // must never be able to reach the real legacy application data dir.
        let legacy = openalpaca_storage::store::migrate::legacy_app_dir()
            .expect("the legacy app dir must resolve");
        assert!(
            legacy.starts_with(root),
            "HOME override did not sandbox the legacy root ({}); refusing to run",
            legacy.display()
        );
        sandbox
    }
}

impl Drop for EnvSandbox {
    fn drop(&mut self) {
        for (var, prev) in self.saved.drain(..) {
            // SAFETY: as above — still holding ENV_LOCK.
            match prev {
                Some(v) => unsafe { std::env::set_var(var, v) },
                None => unsafe { std::env::remove_var(var) },
            }
        }
    }
}

/// Every `.master_key` anywhere under `root`.
fn master_key_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.file_name().is_some_and(|n| n == ".master_key") {
                out.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(root, &mut found);
    found.sort();
    found
}

#[test]
fn the_cli_keeps_one_master_key_and_it_is_the_state_dir_one() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());

    set_ai_value("ai.anthropic.api_key", "sk-secret-value").unwrap();

    let state = openalpaca_storage::store::master_key_dir().unwrap();
    assert_eq!(
        master_key_files(tmp.path()),
        vec![state.join(".master_key")],
        "the CLI must encrypt with the state dir's master key and generate no other"
    );
    assert_eq!(
        get_ai_value("ai.anthropic.api_key").unwrap().as_deref(),
        Some("sk-secret-value")
    );
}

#[test]
fn the_daemons_master_key_decrypts_what_the_cli_wrote() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());

    set_ai_value("ai.anthropic.api_key", "sk-secret-value").unwrap();

    // Exactly what the daemon does at boot (`apps/openalpacad/src/main.rs`):
    // resolve the state dir, load the key that lives there.
    let daemon_dir = openalpaca_storage::store::master_key_dir().unwrap();
    let daemon = KeyEncryptor::load_or_generate_at(&daemon_dir).unwrap();

    let config = openalpaca_llm::config::read_config(&llm_config_path().unwrap()).unwrap();
    let encrypted = config
        .providers
        .as_ref()
        .and_then(|p| p.get("anthropic"))
        .and_then(|p| p.keys.as_ref())
        .and_then(|keys| keys.first())
        .and_then(|k| k.secret_encrypted.clone())
        .expect("the CLI stored an encrypted secret in llm.toml");

    assert!(KeyEncryptor::is_encrypted(&encrypted));
    assert_eq!(daemon.decrypt(&encrypted).unwrap(), "sk-secret-value");
}
