//! Test support shared by every `#[cfg(test)]` module of this binary.
//!
//! A bin crate compiles all its test modules into **one** test binary and runs
//! them on parallel threads, so anything that mutates the process environment
//! must be serialized by a single lock. This module holds that lock; nothing
//! else in the crate may declare a second one.

use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

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

/// The one environment sandbox of this test binary: every test in it that
/// touches `SANDBOXED` does so through this, under `ENV_LOCK` — the binary runs
/// all its `#[cfg(test)]` modules on parallel threads, so a second lock would
/// not serialize anything against this one.
pub(crate) struct EnvSandbox {
    _lock: MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvSandbox {
    /// Points every sandboxed variable into `root`: `HOME` is `root` itself,
    /// the home store is `root/home` and the config directory is
    /// `root/config` (created here). Held until the guard is dropped.
    pub(crate) fn enter(root: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let saved = SANDBOXED
            .iter()
            .map(|var| (*var, std::env::var_os(var)))
            .collect();
        let config = root.join("config");
        std::fs::create_dir_all(&config).unwrap();
        // SAFETY: serialized by ENV_LOCK; every test in this binary that
        // touches these variables holds it through this sandbox.
        unsafe {
            std::env::set_var("HOME", root);
            std::env::set_var("OPENALPACA_HOME_STORE", root.join("home"));
            std::env::set_var("OPENALPACA_CONFIG_DIR", &config);
            std::env::remove_var("OPENALPACA_MASTER_KEY");
        }
        let sandbox = Self { _lock: lock, saved };

        // Fail before writing anything if the sandbox is not airtight: a test
        // must never be able to reach the real legacy application data dir.
        let legacy = openalpaca_storage::store::legacy_root::legacy_app_dir()
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
