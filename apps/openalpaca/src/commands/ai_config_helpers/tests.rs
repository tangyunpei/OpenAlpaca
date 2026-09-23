//! The CLI's crypto edge.
//!
//! The master key is one file, `~/.openalpaca/state/.master_key`. The daemon
//! resolves it through `store::master_key_dir()`; so must every CLI command, or
//! `openalpaca ai config set-key` writes secrets the daemon cannot read.

use super::llm_config_path;
use crate::commands::ai_config::{get_ai_value, set_ai_value};
use crate::test_util::EnvSandbox;
use openalpaca_llm::keys::key_encryption::KeyEncryptor;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

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
