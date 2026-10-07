//! F3: the master key leaves the daemon's environment.
//!
//! The daemon's boot preamble takes the key from `state/.master_key`, installs
//! it as this crate's process-local master key and removes
//! `OPENALPACA_MASTER_KEY` from its environment, so no child it spawns — a
//! shell command, a skill script, an MCP server, a plugin, a CLI model backend
//! — receives the key. `KeyEncryptor::install_process_key_and_clear_env` is
//! that step; this test drives it the way the daemon does and watches a child.
//!
//! **Its own process, one test.** The installed key is a process-wide
//! `OnceLock`, and the environment is process-wide too: inside a crate's unit
//! test binary (parallel threads; the daemon's `test_util.rs` sets the very
//! variable) either would change what every other test reads. An integration
//! test file is its own binary, and this one holds exactly ONE `#[test]`, so
//! the test thread is the only thread touching the environment. A second test
//! in this file breaks that argument — put it in a file of its own.
#![cfg(unix)]

use openalpaca_llm::keys::key_encryption::{KeyEncryptor, MASTER_KEY_ENV};
use std::process::Command;

/// What a launching shell might have exported — the value the daemon used to
/// overwrite, and now removes.
const INHERITED_KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
/// What `state/.master_key` holds — the key the daemon installs.
const FILE_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

/// Whether a child spawned now is handed the variable: `env` prints one
/// `NAME=value` line per variable it inherited. The command's environment is
/// deliberately not cleared — inheritance is the point.
fn a_child_inherits_the_master_key() -> bool {
    let out = Command::new("env").output().expect("spawn `env`");
    assert!(out.status.success(), "`env` exited {:?}", out.status.code());
    let prefix = format!("{MASTER_KEY_ENV}=");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|line| line.starts_with(&prefix))
}

#[test]
fn a_child_spawned_after_the_install_sees_no_master_key_and_the_installed_key_still_decrypts() {
    // The premise: the daemon was started by a shell that exported the key.
    // SAFETY: this file's one test runs on the only thread that touches the
    // environment (libtest's main thread only waits on it).
    unsafe { std::env::set_var(MASTER_KEY_ENV, INHERITED_KEY) };
    assert!(
        a_child_inherits_the_master_key(),
        "premise: before the install a child inherits {MASTER_KEY_ENV}"
    );

    // Before any install the environment is what `from_env` reads — the
    // CLI's and the test harnesses' path, which must keep working.
    let inherited = KeyEncryptor::from_env().expect("the environment is the fallback");
    let sealed_by_inherited = inherited.encrypt("sk-from-the-shell").unwrap();

    // Review focus — a malformed key is refused before anything is installed,
    // and the environment fallback still answers afterwards.
    assert!(
        KeyEncryptor::install_process_key("not-hex").is_err(),
        "a key that is not 64 hex characters is refused"
    );
    KeyEncryptor::from_env().expect("a refused install leaves the fallback in place");

    // The daemon's step: the file's key in, the variable out.
    // SAFETY: as above — one thread.
    unsafe { KeyEncryptor::install_process_key_and_clear_env(FILE_KEY) }
        .expect("installing the file's key");

    assert!(
        std::env::var_os(MASTER_KEY_ENV).is_none(),
        "{MASTER_KEY_ENV} must be gone from this process's environment"
    );
    assert!(
        !a_child_inherits_the_master_key(),
        "a child spawned after the install must not see {MASTER_KEY_ENV}"
    );

    // Every reader built on `from_env` now gets the installed key, with no
    // variable anywhere...
    let installed = KeyEncryptor::from_env().expect("the installed key answers with no variable");
    let sealed = installed.encrypt("sk-secret").unwrap();
    assert_eq!(
        KeyEncryptor::from_env().unwrap().decrypt(&sealed).unwrap(),
        "sk-secret",
        "a second from_env reads the same installed key"
    );
    // ...and it is the FILE's key, not the inherited one: what the shell's
    // key sealed does not open. `ensure_at` never reads the environment, so
    // the daemon's file wins over an inherited variable, as it always has.
    assert!(
        installed.decrypt(&sealed_by_inherited).is_err(),
        "the inherited key must not be the one installed"
    );

    // The same key again is a no-op; a different one is refused.
    KeyEncryptor::install_process_key(FILE_KEY).expect("re-installing the same key is fine");
    assert!(
        KeyEncryptor::install_process_key(INHERITED_KEY).is_err(),
        "a second, different master key in one process is refused"
    );
}
