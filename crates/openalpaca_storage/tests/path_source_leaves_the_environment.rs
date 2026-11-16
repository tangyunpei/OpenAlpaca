//! LOGIN-CAPTURE (*Rev 91* (2)): `OPENALPACA_PATH_SOURCE` leaves the daemon's
//! environment.
//!
//! The desktop app sets the variable on the daemon it spawns, naming where
//! the daemon's `PATH` came from. The daemon's boot preamble reads it once,
//! for its boot line, and removes it — beside `OPENALPACA_MASTER_KEY` — so no
//! child it spawns (a shell command, an MCP server, a plugin, a daemon an
//! agent starts) inherits the app's word about a `PATH` it never captured.
//! `store::take_path_source` is that step; this test drives it the way the
//! daemon does and watches a child.
//!
//! **Its own process, one test**, as `openalpaca_llm`'s
//! `master_key_leaves_the_environment.rs`: the environment is process-wide,
//! and an integration test file is its own binary, so with exactly ONE
//! `#[test]` here the test thread is the only thread touching it. A second
//! test in this file breaks that argument — put it in a file of its own.
#![cfg(unix)]

use openalpaca_storage::store::{PATH_SOURCE_ENV, take_path_source};
use std::ffi::OsStr;
use std::process::Command;

/// What the app sets: the source, the capture's milliseconds and a key the
/// owner listed, by name.
const ANNOUNCED: &str = "login-shell 212 AWS_PROFILE";

/// Whether a child spawned now is handed the variable: `env` prints one
/// `NAME=value` line per variable it inherited. The command's environment is
/// deliberately not cleared — inheritance is the point.
fn a_child_inherits_the_path_source() -> bool {
    let out = Command::new("/usr/bin/env")
        .output()
        .expect("spawn /usr/bin/env");
    assert!(out.status.success(), "`env` exited {:?}", out.status.code());
    let prefix = format!("{PATH_SOURCE_ENV}=");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|line| line.starts_with(&prefix))
}

#[test]
fn the_path_source_is_read_once_and_no_child_spawned_after_inherits_it() {
    // The premise: the app spawned this daemon with the variable set.
    // SAFETY: this file's one test runs on the only thread that touches the
    // environment (libtest's main thread only waits on it).
    unsafe { std::env::set_var(PATH_SOURCE_ENV, ANNOUNCED) };
    assert!(
        a_child_inherits_the_path_source(),
        "premise: before the take a child inherits {PATH_SOURCE_ENV}"
    );

    // The daemon's step: read it for the boot line, and remove it.
    // SAFETY: as above — one thread.
    let taken = unsafe { take_path_source() };
    assert_eq!(taken.as_deref(), Some(OsStr::new(ANNOUNCED)));
    assert!(
        std::env::var_os(PATH_SOURCE_ENV).is_none(),
        "{PATH_SOURCE_ENV} must be gone from this process's environment"
    );
    assert!(
        !a_child_inherits_the_path_source(),
        "a child spawned after the take must not see {PATH_SOURCE_ENV}"
    );
    // A daemon no app announced a PATH to takes nothing.
    // SAFETY: as above.
    assert_eq!(unsafe { take_path_source() }, None);
}
