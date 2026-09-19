//! The daemon binary's command line (W1), driven for real.
//!
//! The unit tests in `src/args.rs` pin the parse; this pins the thing that
//! actually went wrong — the *binary* booting when someone typed `--help`.
//! Only an end-to-end run can show that, because what it asserts is the
//! absence of side effects: no store root, no master key, no lock, no
//! discovery file, no database.
//!
//! **Isolation is the test's first job.** Every child gets
//! `OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR` inside its own
//! tempdir, and its working directory is that tempdir too, so even a binary
//! that ignores its arguments entirely can only write there — never to the
//! developer's real `~/.openalpaca` and never into the checkout. Each run is
//! bounded and killed at the deadline, so a daemon that *does* boot is stopped
//! rather than left running.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Long enough for a process that only prints and exits; short enough that a
/// daemon which wrongly boots is killed promptly.
const RUN_TIMEOUT: Duration = Duration::from_secs(15);

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

/// Run the daemon binary with `args` inside `dir`, bounded by [`RUN_TIMEOUT`].
fn run_daemon(dir: &Path, args: &[&str]) -> Run {
    let mut child = Command::new(env!("CARGO_BIN_EXE_openalpacad"))
        .args(args)
        .current_dir(dir)
        // Both, always: the safety rule for this binary is that it never runs
        // without them pointing somewhere disposable.
        .env("OPENALPACA_HOME_STORE", dir)
        .env("OPENALPACA_CONFIG_DIR", dir.join("config"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn openalpacad");

    let deadline = Instant::now() + RUN_TIMEOUT;
    let mut status = None;
    let mut timed_out = false;
    loop {
        match child.try_wait().expect("try_wait") {
            Some(s) => {
                status = Some(s);
                break;
            }
            None if Instant::now() >= deadline => {
                timed_out = true;
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    }

    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }

    Run {
        code: status.and_then(|s| s.code()),
        stdout,
        stderr,
        timed_out,
    }
}

/// What "it did not boot" means, spelled out: `ensure_store` seeds a
/// `README.md` and a `.layout` at the root on its very first touch — before
/// the lock, the master key, the discovery file or the database — and nothing
/// else in this test writes anything at all. So an empty directory is proof
/// the daemon stopped at the command line.
fn assert_nothing_was_created(dir: &Path) {
    let leftovers: Vec<String> = std::fs::read_dir(dir)
        .expect("read the tempdir")
        .map(|e| {
            e.expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "the daemon touched its store while only being asked for usage: {leftovers:?}"
    );
}

#[test]
fn help_prints_usage_and_creates_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let run = run_daemon(tmp.path(), &["--help"]);

    assert!(
        !run.timed_out,
        "`--help` booted a daemon instead of printing"
    );
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
    assert!(
        run.stdout.contains("Usage:") && run.stdout.contains("openalpacad"),
        "usage goes to stdout: {:?}",
        run.stdout
    );
    assert!(
        run.stdout.contains("OPENALPACA_HOME_STORE")
            && run.stdout.contains("OPENALPACA_CONFIG_DIR")
            && run.stdout.contains("openalpaca daemon start"),
        "usage names what steers the daemon: {:?}",
        run.stdout
    );
    assert_nothing_was_created(tmp.path());
}

#[test]
fn version_prints_the_version_and_creates_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let run = run_daemon(tmp.path(), &["-V"]);

    assert!(!run.timed_out, "`-V` booted a daemon instead of printing");
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
    assert_eq!(
        run.stdout.trim(),
        format!("openalpacad {}", env!("CARGO_PKG_VERSION"))
    );
    assert_nothing_was_created(tmp.path());
}

#[test]
fn an_unknown_argument_is_refused_with_exit_2() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let run = run_daemon(tmp.path(), &["--port=8080"]);

    assert!(
        !run.timed_out,
        "an unknown argument booted a daemon instead of being refused"
    );
    assert_eq!(run.code, Some(2), "stdout: {}", run.stdout);
    assert!(
        run.stderr.contains("--port=8080") && run.stderr.contains("Usage:"),
        "the refusal names the argument and shows usage, on stderr: {:?}",
        run.stderr
    );
    assert!(run.stdout.is_empty(), "nothing on stdout: {:?}", run.stdout);
    assert_nothing_was_created(tmp.path());
}
