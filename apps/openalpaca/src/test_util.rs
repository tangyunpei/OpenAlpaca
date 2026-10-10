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

const SANDBOXED: [&str; 3] = [
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
    /// Points every sandboxed variable into `root`: the home store is
    /// `root/home` and the config directory is `root/config` (created here).
    /// Held until the guard is dropped, which restores every variable as it
    /// was.
    pub(crate) fn enter(root: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let saved = SANDBOXED
            .iter()
            .map(|var| (*var, std::env::var_os(var)))
            .collect();
        let config = root.join("config");
        std::fs::create_dir_all(&config).unwrap();
        // SAFETY: serialized by ENV_LOCK, held from here until the guard is
        // dropped; every test in this binary that touches these variables
        // holds it through this sandbox.
        unsafe {
            std::env::set_var("OPENALPACA_HOME_STORE", root.join("home"));
            std::env::set_var("OPENALPACA_CONFIG_DIR", &config);
            std::env::remove_var("OPENALPACA_MASTER_KEY");
        }
        Self { _lock: lock, saved }
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

/// One canned HTTP answer per connection, served in order on a loopback
/// port with `Connection: close`, so a `DaemonClient` opens a fresh
/// connection for each request. Returns the address and, on join, the raw
/// requests it received — what `persona edit` and `memory edit` are checked
/// against.
pub(crate) fn serve(
    answers: Vec<String>,
) -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let addr = listener.local_addr().expect("the bound address");
    let server = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for answer in answers {
            let (mut socket, _) = listener.accept().expect("a request arrives");
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            // Headers first, then as many body bytes as Content-Length says.
            loop {
                let n = socket.read(&mut buf).expect("the request is readable");
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&raw).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let head = &text[..end];
                    let length: usize = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if raw.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            seen.push(String::from_utf8_lossy(&raw).to_string());
            socket
                .write_all(answer.as_bytes())
                .expect("the answer is written");
            socket.flush().ok();
        }
        seen
    });
    (addr, server)
}

/// Whether the volume `dir` is on ignores letter case — what a test of a
/// folder renamed by case only (ST-23) needs before it can rename one. Probes
/// with a file and a stat of its other-case name, and removes the probe. A
/// copy of storage's own probe (`store::tests::volume_ignores_case`), which is
/// test-only there and not exported across crates.
pub(crate) fn volume_ignores_case(dir: &Path) -> bool {
    let made = dir.join("case-probe-Ab");
    std::fs::write(&made, b"").unwrap();
    let ignores = std::fs::symlink_metadata(dir.join("CASE-PROBE-AB")).is_ok();
    std::fs::remove_file(&made).unwrap();
    ignores
}

/// A JSON answer for [`serve`], with an `ETag` header.
pub(crate) fn json_answer(status: &str, etag: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nETag: \"{etag}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
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

/// Storage's `store::projects::register` refuses an unguarded home store only
/// in a build that carries its `test-home-guard` feature, which this crate
/// turns on through `[dev-dependencies]` (storage design, Stage 1, S1.9).
#[test]
fn the_storage_test_home_guard_is_armed_in_this_test_build() {
    assert!(openalpaca_storage::store::projects::test_home_guard_armed());
}
