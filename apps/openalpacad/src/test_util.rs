//! Test-only helpers shared across this binary's test modules.

use std::ffi::OsString;
use std::path::Path;
use std::sync::MutexGuard;

/// Serializes every test in this binary that re-points `OPENALPACA_HOME_STORE`.
///
/// The variable is process-global and every store accessor reads it on each
/// call, so two modules holding *separate* locks would still race. This is the
/// binary's one lock — mirroring `openalpaca_core::test_util` — and every test
/// that re-points the home store takes it through [`HomeStoreGuard`].
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Points `OPENALPACA_HOME_STORE` at a temp root for the guard's lifetime.
/// No test ever touches the real `~/.openalpaca`.
pub(crate) struct HomeStoreGuard {
    _lock: MutexGuard<'static, ()>,
    prev: Option<OsString>,
    /// The master key this guard set, if any — restored the same way.
    prev_master_key: Option<Option<OsString>>,
}

/// A fixed 32-byte key. `KeyEncryptor::from_env` is what a
/// `LlmSettingsService` is built through, and a test must never reach for the
/// owner's real one.
const TEST_MASTER_KEY: &str =
    "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

impl HomeStoreGuard {
    pub(crate) fn set(path: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os(openalpaca_storage::store::HOME_STORE_ENV);
        // SAFETY: serialized by ENV_LOCK — the binary's only writer.
        unsafe { std::env::set_var(openalpaca_storage::store::HOME_STORE_ENV, path) };
        Self {
            _lock: lock,
            prev,
            prev_master_key: None,
        }
    }

    /// As [`Self::set`], plus a throwaway `OPENALPACA_MASTER_KEY` for the tests
    /// that build an `LlmSettingsService`.
    pub(crate) fn set_with_master_key(path: &Path) -> Self {
        let mut guard = Self::set(path);
        guard.prev_master_key = Some(std::env::var_os("OPENALPACA_MASTER_KEY"));
        // SAFETY: as above — still holding ENV_LOCK.
        unsafe { std::env::set_var("OPENALPACA_MASTER_KEY", TEST_MASTER_KEY) };
        guard
    }
}

impl Drop for HomeStoreGuard {
    fn drop(&mut self) {
        // SAFETY: as above — still holding ENV_LOCK.
        if let Some(prev) = self.prev_master_key.take() {
            match prev {
                Some(v) => unsafe { std::env::set_var("OPENALPACA_MASTER_KEY", v) },
                None => unsafe { std::env::remove_var("OPENALPACA_MASTER_KEY") },
            }
        }
        match self.prev.take() {
            Some(v) => unsafe { std::env::set_var(openalpaca_storage::store::HOME_STORE_ENV, v) },
            None => unsafe { std::env::remove_var(openalpaca_storage::store::HOME_STORE_ENV) },
        }
    }
}
