//! Test support shared by every `#[cfg(test)]` module of this binary.
//!
//! A bin crate compiles all its test modules into **one** test binary and runs
//! them on parallel threads, so anything that mutates the process environment
//! must be serialized by a single lock. This module holds that lock; nothing
//! else in the crate may declare a second one.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

const SANDBOXED: [&str; 5] = [
    // `directories`' data dir — and therefore the pre-D1 app dir that
    // `legacy_app_dir()` resolves — comes from HOME, and on Linux from
    // XDG_DATA_HOME first when that is absolute. Overriding HOME and clearing
    // XDG_DATA_HOME confines it, and any stray legacy-root write, to the temp
    // dir instead of the developer's real one.
    "HOME",
    "XDG_DATA_HOME",
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
    /// `XDG_DATA_HOME` is unset, the home store is `root/home` and the config
    /// directory is `root/config` (created here). Held until the guard is
    /// dropped, which restores every variable as it was.
    pub(crate) fn enter(root: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let saved = SANDBOXED
            .iter()
            .map(|var| (*var, std::env::var_os(var)))
            .collect();
        let sandbox = Self { _lock: lock, saved };
        sandbox.apply(root);
        sandbox
    }

    /// Sets the sandboxed variables, then refuses to go on unless the legacy
    /// root resolves inside `root`. Called with `ENV_LOCK` held (it is a
    /// method on the guard).
    fn apply(&self, root: &Path) {
        let config = root.join("config");
        std::fs::create_dir_all(&config).unwrap();
        // SAFETY: serialized by ENV_LOCK; every test in this binary that
        // touches these variables holds it through this sandbox.
        unsafe {
            std::env::set_var("HOME", root);
            std::env::remove_var("XDG_DATA_HOME");
            std::env::set_var("OPENALPACA_HOME_STORE", root.join("home"));
            std::env::set_var("OPENALPACA_CONFIG_DIR", &config);
            std::env::remove_var("OPENALPACA_MASTER_KEY");
        }

        // Fail before writing anything if the sandbox is not airtight: a test
        // must never be able to reach the real legacy application data dir.
        let legacy = openalpaca_storage::store::legacy_root::legacy_app_dir()
            .expect("the legacy app dir must resolve");
        assert!(
            legacy.starts_with(root),
            "the HOME/XDG_DATA_HOME override did not sandbox the legacy root ({}); \
             refusing to run",
            legacy.display()
        );
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

/// Names the store root a [`LockHolder`] child is spawned for. The child
/// refuses to lock anything unless its inherited `OPENALPACA_HOME_STORE` is
/// exactly this.
const HOLD_LOCK_FOR: &str = "OPENALPACA_TEST_HOLD_LOCK_FOR";

/// What the child prints once it holds the lock.
const LOCKED: &str = "OPENALPACA_TEST_LOCK_HELD";

/// Another process holding the daemon's singleton lock on the sandboxed store
/// — what a running, or still booting, daemon looks like to anything that
/// tries to take that lock.
///
/// It has to be another process. The lock is a POSIX `fcntl` record lock, and
/// a process never conflicts with itself on one: a second acquisition in the
/// same process succeeds, and dropping it releases the first.
pub(crate) struct LockHolder {
    child: Child,
    stdout: BufReader<ChildStdout>,
}

impl LockHolder {
    /// Runs this test binary again with only [`singleton_lock_holder_process`]
    /// selected, and returns once that child holds the lock. Call it inside an
    /// [`EnvSandbox`]: the child inherits the sandboxed environment and
    /// refuses unless `OPENALPACA_HOME_STORE` is `store_root`. The lock is
    /// held until this is dropped.
    pub(crate) fn spawn(store_root: &Path) -> Self {
        Self::try_spawn(store_root)
            .expect("the lock-holder process exited before it held the lock")
    }

    /// [`Self::spawn`], answering `None` when the child could not take the
    /// lock — because something else holds it — instead of panicking.
    pub(crate) fn try_spawn(store_root: &Path) -> Option<Self> {
        let mut child = Command::new(std::env::current_exe().expect("this test binary"))
            .args([
                "test_util::singleton_lock_holder_process",
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(HOLD_LOCK_FOR, store_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // A child that could not take the lock panics by design; that
            // is an answer here, not output worth printing.
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the lock-holder process");
        let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut line = String::new();
        let held = loop {
            line.clear();
            let read = stdout.read_line(&mut line).unwrap_or(0);
            // libtest has already printed `test <name> ... ` on this line.
            if line.trim_end().ends_with(LOCKED) {
                break true;
            }
            if read == 0 {
                break false;
            }
        };
        let holder = Self { child, stdout };
        // A child that could not lock has exited; dropping the holder reaps it.
        held.then_some(holder)
    }
}

impl Drop for LockHolder {
    fn drop(&mut self) {
        // Closing its stdin is the child's signal to release and exit; its
        // remaining output is drained so it never blocks on a full pipe.
        drop(self.child.stdin.take());
        let mut rest = Vec::new();
        let _ = self.stdout.read_to_end(&mut rest);
        let _ = self.child.wait();
    }
}

/// Not a test: the body of the child [`LockHolder::spawn`] starts. Ignored,
/// and a no-op unless that spawn set [`HOLD_LOCK_FOR`], so `--ignored` runs
/// it harmlessly.
#[test]
#[ignore = "the child process of LockHolder::spawn; does nothing on its own"]
fn singleton_lock_holder_process() {
    let Some(root) = std::env::var_os(HOLD_LOCK_FOR) else {
        return;
    };
    assert_eq!(
        std::env::var_os("OPENALPACA_HOME_STORE"),
        Some(root),
        "the lock holder must inherit a sandboxed store; refusing to lock anything else"
    );
    let _lock = openalpaca_storage::discovery::acquire_single_instance_lock(false)
        .expect("take the singleton lock");
    println!("{LOCKED}");
    std::io::stdout().flush().expect("flush");
    let mut until_closed = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut until_closed);
}

/// A Linux box (or runner) that exports an absolute `XDG_DATA_HOME` puts the
/// legacy root there, outside the sandbox, whatever `HOME` says. Entering the
/// sandbox must clear it — and leaving must put it back.
#[test]
fn the_sandbox_clears_xdg_data_home_and_restores_it() {
    let tmp = tempfile::tempdir().unwrap();
    let before = {
        let sandbox = EnvSandbox::enter(tmp.path());
        let before = sandbox
            .saved
            .iter()
            .find(|(var, _)| *var == "XDG_DATA_HOME")
            .map(|(_, value)| value.clone())
            .expect("XDG_DATA_HOME is one of the sandboxed variables");
        // SAFETY: under ENV_LOCK, held by `sandbox`.
        unsafe { std::env::set_var("XDG_DATA_HOME", "/an/exported/xdg/data/home") };
        sandbox.apply(tmp.path());
        assert_eq!(
            std::env::var_os("XDG_DATA_HOME"),
            None,
            "the sandbox must clear XDG_DATA_HOME"
        );
        before
    };
    let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    assert_eq!(
        std::env::var_os("XDG_DATA_HOME"),
        before,
        "leaving the sandbox must restore XDG_DATA_HOME as it was"
    );
}
