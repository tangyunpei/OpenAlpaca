//! This crate's own home-store guard (Q05): every test here that resolves the
//! home store holds one, and points the process at a temp directory first —
//! no test reads or writes the developer's `~/.openalpaca`. A copy of
//! `openalpaca_storage`'s, never shared with it: this crate takes no
//! dependency on storage, not even a dev-dependency, and each crate's test
//! binary is its own process, so each has its own lock.

use crate::paths::HOME_STORE_ENV;
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

/// `home_root()` reads `OPENALPACA_HOME_STORE` on every call, so tests that set
/// it must not run concurrently with each other.
static ENV_LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct HomeStoreGuard {
    _lock: MutexGuard<'static, ()>,
    prev: Option<OsString>,
    /// `HOME` as it was, when [`HomeStoreGuard::for_user_home`] moved it.
    prev_home: Option<Option<OsString>>,
}

impl HomeStoreGuard {
    /// Points `OPENALPACA_HOME_STORE` at `path` until the guard drops.
    pub(crate) fn set(path: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os(HOME_STORE_ENV);
        // SAFETY: serialized by ENV_LOCK; every test that reads the variable
        // holds the same guard.
        unsafe { std::env::set_var(HOME_STORE_ENV, path) };
        Self {
            _lock: lock,
            prev,
            prev_home: None,
        }
    }

    /// Moves the user's home directory — `HOME`, which `std::env::home_dir`
    /// reads, so `home_root()`'s fallback becomes `<home>/.openalpaca` — and
    /// sets `OPENALPACA_HOME_STORE` to `store`, or removes it for `None`.
    /// Same lock, no second one. Panics, restoring both variables, if
    /// `std::env::home_dir` does not answer with `home`.
    pub(crate) fn for_user_home(home: &Path, store: Option<&Path>) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let guard = Self {
            _lock: lock,
            prev: std::env::var_os(HOME_STORE_ENV),
            prev_home: Some(std::env::var_os("HOME")),
        };
        // SAFETY: serialized by ENV_LOCK, which `guard` holds until it drops
        // and restores both variables.
        unsafe {
            std::env::set_var("HOME", home);
            match store {
                Some(store) => std::env::set_var(HOME_STORE_ENV, store),
                None => std::env::remove_var(HOME_STORE_ENV),
            }
        }
        let resolved = std::env::home_dir();
        assert_eq!(
            resolved.as_deref(),
            Some(home),
            "HOME did not move the user's home directory; refusing to run"
        );
        guard
    }
}

impl Drop for HomeStoreGuard {
    fn drop(&mut self) {
        // SAFETY: as above — still holding ENV_LOCK.
        if let Some(prev_home) = self.prev_home.take() {
            match prev_home {
                Some(v) => unsafe { std::env::set_var("HOME", v) },
                None => unsafe { std::env::remove_var("HOME") },
            }
        }
        match self.prev.take() {
            Some(v) => unsafe { std::env::set_var(HOME_STORE_ENV, v) },
            None => unsafe { std::env::remove_var(HOME_STORE_ENV) },
        }
    }
}
