//! The command layer under `tauri::test` (Q11): each command the webview
//! invokes, driven through the IPC round trip — `mock_builder()` with the
//! same [`super::with_commands`] list `run` builds from, `mock_context(
//! noop_assets())`, `get_ipc_response` — for its argument names, its
//! camelCase answer and its refusal strings.
//!
//! **The command layer, not the capability.** `mock_context` carries an
//! empty ACL authority (no app manifest), so tauri lets every app command
//! through from a local origin; whether `capabilities/default.json` admits a
//! command is G-bundle leg A's to prove, never these tests'.
//!
//! **Nothing here spawns (C-12).** Until Q04 removes the debug `PATH`
//! fallback in `spawn_daemon` (or injects its locator and spawner), the tests
//! drive only the paths that spawn nothing: `ensure_daemon_running`'s reuse of
//! a discovery file whose address an in-test loopback listener holds open (the
//! shell's liveness probe is a bare TCP connect, which the listener's backlog
//! completes), and the refusals. Its other arm — no live daemon — spawns
//! one and has no test; nor has `reveal_path`'s admitted path, which runs
//! `open -R` (Finder). As a backstop the guard points `PATH` at an empty
//! folder, and the reuse tests check that no `openalpacad` sits beside the
//! test binary and that no daemon log was opened: a reuse that regressed
//! fails to find a daemon instead of starting one.
//!
//! Every test holds [`HomeStoreGuard`], this crate's copy of the home-store
//! guard: a scratch `OPENALPACA_HOME_STORE`, `OPENALPACA_CONFIG_DIR`, `HOME`
//! and `PATH`, never `~/.openalpaca`.

use super::*;
use openalpaca_storage::discovery::Discovery;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::sync::MutexGuard;
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets};
use tauri::webview::InvokeRequest;

// ============================================================================
// The home-store guard
// ============================================================================

/// Serializes every test in this binary: each re-points process-global
/// variables that every store accessor reads on each call.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The variables the guard re-points, restored on drop.
const GUARDED_VARS: [&str; 4] = [
    store::HOME_STORE_ENV,
    "OPENALPACA_CONFIG_DIR",
    "HOME",
    "PATH",
];

/// Points the home store, the config dir, `HOME` and `PATH` into a scratch
/// root for the guard's lifetime, and removes the root on drop. No test ever
/// touches the real `~/.openalpaca`.
struct HomeStoreGuard {
    _lock: MutexGuard<'static, ()>,
    prev: Vec<(&'static str, Option<OsString>)>,
    root: PathBuf,
}

impl HomeStoreGuard {
    fn set(test: &str) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let made =
            std::env::temp_dir().join(format!("openalpaca_gui-test-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&made);
        std::fs::create_dir_all(&made).expect("scratch root");
        // Canonical from the moment it is made: on macOS the temp dir leads
        // into `/private/`, and the refusals name resolved paths.
        let root = made.canonicalize().expect("canonical scratch root");
        let store_dir = root.join("store");
        let config_dir = store_dir.join("config");
        let home_dir = root.join("home");
        let empty_bin = root.join("bin");
        for dir in [&config_dir, &home_dir, &empty_bin] {
            std::fs::create_dir_all(dir).expect("scratch dir");
        }
        let values: [&Path; 4] = [&store_dir, &config_dir, &home_dir, &empty_bin];
        let prev = GUARDED_VARS
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect();
        for (name, value) in GUARDED_VARS.iter().zip(values) {
            // SAFETY: serialized by ENV_LOCK — this binary's only writer.
            unsafe { std::env::set_var(name, value) };
        }
        // No test inherits another's spawned daemon (Q21's `daemonPath`).
        forget_spawned();
        Self {
            _lock: lock,
            prev,
            root,
        }
    }

    /// The scratch home store (`OPENALPACA_HOME_STORE`).
    fn store(&self) -> PathBuf {
        self.root.join("store")
    }

    /// The scratch root itself, which holds the store, `HOME` and `PATH`.
    fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for HomeStoreGuard {
    fn drop(&mut self) {
        for (name, value) in self.prev.drain(..) {
            // SAFETY: as above — still holding ENV_LOCK.
            match value {
                Some(v) => unsafe { std::env::set_var(name, v) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Storage's `store::projects::register` refuses an unguarded home store only
/// in a build that carries its `test-home-guard` feature, which this crate
/// turns on through `[dev-dependencies]` (storage design, Stage 1, S1.9).
#[test]
fn the_storage_test_home_guard_is_armed_in_this_test_build() {
    assert!(openalpaca_storage::store::projects::test_home_guard_armed());
}

// ============================================================================
// The IPC round trip
// ============================================================================

/// A mock app built from [`with_commands`] and its one webview. The app is
/// returned beside the webview so that it lives as long as the test needs it.
fn mock_webview() -> (
    tauri::App<tauri::test::MockRuntime>,
    tauri::WebviewWindow<tauri::test::MockRuntime>,
) {
    let app = with_commands(mock_builder())
        .build(mock_context(noop_assets()))
        .expect("mock app");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("mock webview");
    (app, webview)
}

/// The webview's `invoke(cmd, args)` as the IPC layer receives it.
fn invoke_request(cmd: &str, args: Value) -> InvokeRequest {
    InvokeRequest {
        cmd: cmd.into(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: "tauri://localhost".parse().expect("url"),
        body: InvokeBody::Json(args),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_string(),
    }
}

/// One `invoke(cmd, args)` from a webview of a mock app built from
/// [`with_commands`]: the command's answer as JSON, or its refusal.
fn invoke(cmd: &str, args: Value) -> Result<Value, Value> {
    let (_app, webview) = mock_webview();
    get_ipc_response(&webview, invoke_request(cmd, args))
        .map(|body| body.deserialize::<Value>().expect("a JSON answer"))
}

/// The shell's own refusal, as the webview receives it (Q13): an object
/// whose `kind` names what refused and whose `message` is the sentence.
fn refusal(answer: Result<Value, Value>) -> Value {
    answer.expect_err("a refusal")
}

/// `{ kind, message }`: a refusal of the shell's own, as the webview reads it.
fn shell_refusal(kind: &str, message: impl Into<String>) -> Value {
    json!({ "kind": kind, "message": message.into() })
}

/// Tauri's own refusal (an argument missing, a command not admitted): a bare
/// string, never the shell's typed refusal.
fn tauri_refusal(answer: Result<Value, Value>) -> String {
    match answer.expect_err("a refusal") {
        Value::String(text) => text,
        other => panic!("Tauri's own refusal that is not a string: {other}"),
    }
}

/// A discovery file for `127.0.0.1:<port>`, written where the shell reads it.
fn write_discovery(port: u16, expired: bool) -> Discovery {
    let mut d = discovery::make_discovery("127.0.0.1", port, "instance-q11".into(), "0.0.0");
    if expired {
        let mut value = serde_json::to_value(&d).expect("discovery as JSON");
        value["auth"]["expires_at"] = json!("2000-01-01T00:00:00Z");
        d = serde_json::from_value(value).expect("expired discovery");
    }
    discovery::write_discovery_atomic(&d).expect("write discovery.json");
    d
}

/// `{ baseUrl, token, instanceId, daemonPath }`, exactly those keys, with
/// `daemonPath` `null`: no test spawns, so no daemon is this run's (Q21).
fn assert_connection_info(answer: &Value, d: &Discovery) {
    let mut keys: Vec<&str> = answer
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["baseUrl", "daemonPath", "instanceId", "token"]);
    assert_eq!(answer["daemonPath"], Value::Null);
    assert_eq!(
        answer["baseUrl"],
        format!("http://127.0.0.1:{}", d.listen.port)
    );
    assert_eq!(answer["token"], d.auth.token.as_str());
    assert_eq!(answer["instanceId"], "instance-q11");
}

/// C-12's backstop for the reuse tests: no daemon beside the test binary for
/// a regressed reuse to find, `PATH` already empty (the guard).
fn assert_no_daemon_beside_the_test_binary() {
    let exe = std::env::current_exe().expect("test binary");
    let beside = exe.parent().expect("its folder").join("openalpacad");
    assert!(
        !beside.exists(),
        "{} exists: a regressed reuse would spawn it",
        beside.display()
    );
}

// ============================================================================
// get_connection_info
// ============================================================================

#[test]
fn get_connection_info_refuses_without_a_discovery_file() {
    let _guard = HomeStoreGuard::set("info-none");
    assert_eq!(
        refusal(invoke("get_connection_info", json!({}))),
        shell_refusal(
            "not_running",
            "Discovery not found: daemon may not be running"
        )
    );
}

#[test]
fn get_connection_info_answers_in_camel_case_from_the_discovery_file() {
    let _guard = HomeStoreGuard::set("info-live");
    let d = write_discovery(1, false);
    let answer = invoke("get_connection_info", json!({})).expect("connection info");
    assert_connection_info(&answer, &d);
}

#[test]
fn get_connection_info_refuses_an_expired_discovery_file() {
    let _guard = HomeStoreGuard::set("info-expired");
    write_discovery(1, true);
    assert_eq!(
        refusal(invoke("get_connection_info", json!({}))),
        shell_refusal("expired", "Discovery token has expired")
    );
}

/// T21: the shell reads the file a daemon up for more than a day has
/// rotated — the token it names now, not the start's, and no refusal of an
/// expiry, where the file as the start wrote it is refused after a day.
#[test]
fn get_connection_info_reads_the_rotated_file() {
    let _guard = HomeStoreGuard::set("info-rotated");
    let made = discovery::make_discovery("127.0.0.1", 1, "instance-q11".into(), "0.0.0");
    // Twenty-five hours ago (the shell has no chrono of its own).
    let started = made.started_at - discovery::TOKEN_LIFETIME - discovery::ROTATE_AFTER / 12;
    let first = made.with_rotated_auth(started);
    discovery::write_discovery_atomic(&first).expect("the start's discovery.json");
    assert_eq!(
        refusal(invoke("get_connection_info", json!({}))),
        shell_refusal("expired", "Discovery token has expired")
    );

    let mut d = first.clone();
    for at in [
        started + discovery::ROTATE_AFTER,
        started + discovery::ROTATE_AFTER * 2,
    ] {
        assert!(d.rotation_due(at));
        d = d.with_rotated_auth(at);
    }
    discovery::write_discovery_atomic(&d).expect("the rotated discovery.json");

    let answer = invoke("get_connection_info", json!({})).expect("connection info");
    assert_connection_info(&answer, &d);
    assert_ne!(answer["token"], first.auth.token.as_str());
}

#[test]
fn get_connection_info_refuses_a_discovery_file_it_cannot_parse() {
    let _guard = HomeStoreGuard::set("info-unparsable");
    let path = store::discovery_path().expect("discovery path");
    std::fs::create_dir_all(path.parent().expect("its folder")).expect("its folder");
    std::fs::write(&path, "not discovery").expect("write discovery.json");
    assert_eq!(
        refusal(invoke("get_connection_info", json!({}))),
        shell_refusal("discovery_unreadable", "Failed to parse discovery.json")
    );
}

/// A discovery file it cannot read (a folder where `discovery.json` should
/// be): the read's own sentence, not the parse's.
#[test]
fn get_connection_info_refuses_a_discovery_file_it_cannot_read() {
    let _guard = HomeStoreGuard::set("info-unreadable");
    let path = store::discovery_path().expect("discovery path");
    std::fs::create_dir_all(&path).expect("a folder at discovery.json's path");
    assert_eq!(
        refusal(invoke("get_connection_info", json!({}))),
        shell_refusal("discovery_unreadable", "Failed to read discovery.json")
    );
}

// ============================================================================
// ensure_daemon_running — the reuse arm only (C-12)
// ============================================================================

#[test]
fn ensure_daemon_running_reuses_a_discovery_file_whose_address_answers() {
    let guard = HomeStoreGuard::set("ensure-live");
    assert_no_daemon_beside_the_test_binary();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let d = write_discovery(listener.local_addr().expect("port").port(), false);

    let answer = invoke("ensure_daemon_running", json!({})).expect("reused");
    assert_connection_info(&answer, &d);
    drop(listener);
    // The spawn arm opens the daemon log before it spawns; it never ran.
    let log = store::daemon_log_path().expect("log path");
    assert!(log.starts_with(guard.store()));
    assert!(!log.exists(), "the spawn arm ran: {}", log.display());
}

#[test]
fn ensure_daemon_running_trusts_liveness_over_an_expired_token() {
    let _guard = HomeStoreGuard::set("ensure-expired");
    assert_no_daemon_beside_the_test_binary();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let d = write_discovery(listener.local_addr().expect("port").port(), true);

    let answer = invoke("ensure_daemon_running", json!({})).expect("reused");
    assert_connection_info(&answer, &d);
    drop(listener);
    assert!(!store::daemon_log_path().expect("log path").exists());
}

// ============================================================================
// read_daemon_log_tail
// ============================================================================

#[test]
fn read_daemon_log_tail_is_empty_with_no_log() {
    let _guard = HomeStoreGuard::set("log-none");
    assert_eq!(
        invoke("read_daemon_log_tail", json!({ "lines": 5 })).expect("a tail"),
        ""
    );
}

#[test]
fn read_daemon_log_tail_returns_the_last_lines_newest_last() {
    let _guard = HomeStoreGuard::set("log-tail");
    store::logs_dir().expect("logs dir");
    let log = store::daemon_log_path().expect("log path");
    std::fs::write(&log, "one\ntwo\nthree\nfour\nfive\n").expect("write log");
    assert_eq!(
        invoke("read_daemon_log_tail", json!({ "lines": 2 })).expect("a tail"),
        "four\nfive"
    );
}

/// A log it cannot read (a folder where `daemon.log` should be) is refused
/// with the path and the system's reason.
#[cfg(unix)]
#[test]
fn read_daemon_log_tail_refuses_a_log_it_cannot_read() {
    let _guard = HomeStoreGuard::set("log-unreadable");
    let log = store::daemon_log_path().expect("log path");
    std::fs::create_dir_all(&log).expect("a folder at the log's path");
    assert_eq!(
        refusal(invoke("read_daemon_log_tail", json!({ "lines": 5 }))),
        shell_refusal(
            "log_unreadable",
            format!(
                "Could not read {}: Is a directory (os error 21)",
                log.display()
            )
        )
    );
}

/// Tauri's own refusal of a call missing an argument stays its bare string:
/// the webview shows it as it is.
#[test]
fn read_daemon_log_tail_refuses_a_call_without_lines() {
    let _guard = HomeStoreGuard::set("log-no-arg");
    let text = tauri_refusal(invoke("read_daemon_log_tail", json!({ "count": 2 })));
    assert!(
        text.contains("command read_daemon_log_tail missing required key lines"),
        "{text}"
    );
}

// ============================================================================
// await_daemon_stopped
// ============================================================================

#[test]
fn await_daemon_stopped_reports_not_running_in_camel_case() {
    let _guard = HomeStoreGuard::set("stop-none");
    assert_eq!(
        invoke("await_daemon_stopped", json!({})).expect("a report"),
        json!({ "outcome": "not_running", "pid": null, "waitedMs": 0 })
    );
}

/// The three outcomes a daemon must be running to reach, as the webview
/// reads them — the conversion alone, no daemon.
#[test]
fn a_stop_report_spells_every_outcome_in_camel_case() {
    let waited = Duration::from_millis(1_500);
    let cases = [
        (
            StopOutcome::Stopped { waited },
            json!({ "outcome": "stopped", "pid": null, "waitedMs": 1500 }),
        ),
        (
            StopOutcome::LockStillHeld { waited },
            json!({ "outcome": "lock_still_held", "pid": null, "waitedMs": 1500 }),
        ),
        (
            StopOutcome::StillAlive { pid: 4242, waited },
            json!({ "outcome": "still_alive", "pid": 4242, "waitedMs": 1500 }),
        ),
    ];
    for (outcome, expected) in cases {
        let report = serde_json::to_value(DaemonStopReport::from(outcome)).expect("as JSON");
        assert_eq!(report, expected);
    }
}

// ============================================================================
// reveal_path — the refusals only (the admitted path runs `open -R`)
// ============================================================================

#[test]
fn reveal_path_refuses_a_path_that_is_not_there() {
    let guard = HomeStoreGuard::set("reveal-missing");
    let gone = guard.store().join("artifacts").join("gone.md");
    assert_eq!(
        refusal(invoke("reveal_path", json!({ "path": gone }))),
        shell_refusal(
            "reveal_refused",
            format!(
                "{} is not there any more, so it cannot be shown in its folder.",
                gone.display()
            )
        )
    );
}

#[test]
fn reveal_path_refuses_a_folder() {
    let guard = HomeStoreGuard::set("reveal-folder");
    let folder = guard.store().join("artifacts");
    std::fs::create_dir_all(&folder).expect("folder");
    assert_eq!(
        refusal(invoke("reveal_path", json!({ "path": folder }))),
        shell_refusal(
            "reveal_refused",
            format!(
                "{} is not a file, so it cannot be shown in its folder.",
                folder.display()
            )
        )
    );
}

#[test]
fn reveal_path_refuses_a_file_outside_every_store() {
    let guard = HomeStoreGuard::set("reveal-outside");
    let outside = guard.root.join("elsewhere.md");
    std::fs::write(&outside, "not the store's").expect("file");
    assert_eq!(
        refusal(invoke("reveal_path", json!({ "path": outside }))),
        shell_refusal(
            "reveal_refused",
            format!(
                "{} is not in an OpenAlpaca store or one of your projects, so it is not \
                 shown in its folder.",
                outside.display()
            )
        )
    );
}

#[test]
fn reveal_path_refuses_a_call_without_path() {
    let _guard = HomeStoreGuard::set("reveal-no-arg");
    let text = tauri_refusal(invoke("reveal_path", json!({ "file": "/x" })));
    assert!(
        text.contains("command reveal_path missing required key path"),
        "{text}"
    );
}

// ============================================================================
// Off the window's thread and off the runtime's (Q12)
// ============================================================================

/// One `invoke(cmd, args)` dispatched from this thread with a responder of
/// the test's own: the thread the answer was produced on, and the answer.
fn invoke_answered_on(cmd: &str, args: Value) -> (std::thread::ThreadId, Result<Value, Value>) {
    let (_app, webview) = mock_webview();
    let (tx, rx) = std::sync::mpsc::channel();
    webview.on_message(
        invoke_request(cmd, args),
        Box::new(move |_webview, _cmd, response, _callback, _error| {
            let _ = tx.send((std::thread::current().id(), response));
        }),
    );
    let (thread, response) = rx
        .recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|_| panic!("{cmd} gave no answer within 30 s"));
    let answer = match response {
        tauri::ipc::InvokeResponse::Ok(body) => {
            Ok(body.deserialize::<Value>().expect("a JSON answer"))
        }
        tauri::ipc::InvokeResponse::Err(tauri::ipc::InvokeError(value)) => Err(value),
    };
    (thread, answer)
}

/// Every command answers on a thread of the async runtime, never on the
/// thread that dispatched the `invoke`. In the app that thread is the main
/// one, which draws the window, and a command without `async` runs on it
/// (Tauri's *Calling Rust from the Frontend*: "Commands without the async
/// keyword are executed on the main thread"): a slow disk under
/// `get_connection_info`, or `read_daemon_log_tail`'s 64 KiB read, froze the
/// window while it ran. The mock app dispatches from the test's own thread,
/// so an answer produced on it is a command that ran inline.
#[test]
fn every_command_answers_off_the_thread_that_invoked_it() {
    let guard = HomeStoreGuard::set("off-the-invoking-thread");
    assert_no_daemon_beside_the_test_binary();
    let here = std::thread::current().id();
    let gone = guard.store().join("artifacts").join("gone.md");
    let mut inline = Vec::new();
    // Before discovery.json exists: `await_daemon_stopped` then answers
    // `not_running` at once.
    for (cmd, args) in [
        ("await_daemon_stopped", json!({})),
        ("read_daemon_log_tail", json!({ "lines": 5 })),
        ("reveal_path", json!({ "path": gone })),
        ("get_connection_info", json!({})),
    ] {
        let (thread, _answer) = invoke_answered_on(cmd, args);
        if thread == here {
            inline.push(cmd);
        }
    }
    // `ensure_daemon_running`'s reuse arm only (C-12): an address that answers.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    write_discovery(listener.local_addr().expect("port").port(), false);
    let (thread, answer) = invoke_answered_on("ensure_daemon_running", json!({}));
    answer.expect("reused");
    if thread == here {
        inline.push("ensure_daemon_running");
    }
    drop(listener);
    assert!(
        inline.is_empty(),
        "answered on the thread that invoked them (the window's, in the app): {inline:?}"
    );
}

/// A named pipe standing in for a slow file: opening it to read blocks until
/// something opens it to write, so a read of it holds its thread for exactly
/// as long as the test keeps it shut — a network volume, or a disk waking.
#[cfg(unix)]
struct BlockedFile {
    path: PathBuf,
    released: bool,
}

#[cfg(unix)]
impl BlockedFile {
    fn at(path: PathBuf) -> Self {
        use std::os::unix::ffi::OsStrExt;

        std::fs::create_dir_all(path.parent().expect("its folder")).expect("its folder");
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("a C path");
        // SAFETY: a NUL-terminated path that outlives the call.
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(
            rc,
            0,
            "mkfifo {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
        Self {
            path,
            released: false,
        }
    }

    /// Lets the reader through with `bytes`: opens the pipe to write once a
    /// reader holds it open (until then the open is refused, `ENXIO`), writes
    /// and closes. Gives up after ten seconds, when nothing is reading.
    fn release(&mut self, bytes: &[u8]) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        if std::mem::replace(&mut self.released, true) {
            return;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&self.path)
            {
                Ok(mut pipe) => {
                    let _ = pipe.write_all(bytes);
                    return;
                }
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    }
}

#[cfg(unix)]
impl Drop for BlockedFile {
    /// A test that failed before letting the reader through still does, so
    /// no thread is left waiting on the pipe.
    fn drop(&mut self) {
        self.release(b"");
    }
}

/// Runs `command()` on a runtime of one thread while `file` is held shut,
/// and reports whether that thread stayed free to run another task — what a
/// command must keep so that one slow read never holds a runtime thread
/// every other `invoke` may be waiting for — with the command's answer once
/// `file` is let through with `bytes`. The ten seconds are a backstop for a
/// command that holds the thread, never a measurement.
#[cfg(unix)]
fn runtime_free_while_blocked<T, F>(
    mut file: BlockedFile,
    bytes: &[u8],
    command: impl FnOnce() -> F + Send + 'static,
) -> (bool, T)
where
    T: Send + 'static,
    F: std::future::Future<Output = T> + 'static,
{
    let (beat, heard) = std::sync::mpsc::channel();
    let runner = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a one-thread runtime")
            .block_on(async move {
                tokio::spawn(async move {
                    let _ = beat.send(());
                });
                command().await
            })
    });
    let free = heard.recv_timeout(Duration::from_secs(10)).is_ok();
    file.release(bytes);
    let answer = runner.join().expect("the command's thread");
    (free, answer)
}

#[cfg(unix)]
#[test]
fn get_connection_info_reads_discovery_off_the_runtime() {
    let _guard = HomeStoreGuard::set("info-blocked");
    let file = BlockedFile::at(store::discovery_path().expect("discovery path"));
    let d = discovery::make_discovery("127.0.0.1", 1, "instance-q11".into(), "0.0.0");
    let bytes = serde_json::to_vec(&d).expect("discovery as JSON");
    let (free, answer) = runtime_free_while_blocked(file, &bytes, get_connection_info);
    assert!(
        free,
        "get_connection_info held the runtime's thread while discovery.json would not open"
    );
    let info = answer.expect("connection info");
    assert_connection_info(&serde_json::to_value(info).expect("as JSON"), &d);
}

#[cfg(unix)]
#[test]
fn read_daemon_log_tail_reads_the_log_off_the_runtime() {
    let _guard = HomeStoreGuard::set("log-blocked");
    let file = BlockedFile::at(store::daemon_log_path().expect("log path"));
    // Whatever a pipe gives a seek, the command answers once let through.
    let (free, _answer) =
        runtime_free_while_blocked(file, b"one\ntwo\n", || read_daemon_log_tail(5));
    assert!(
        free,
        "read_daemon_log_tail held the runtime's thread while daemon.log would not open"
    );
}

#[cfg(unix)]
#[test]
fn ensure_daemon_running_reads_discovery_off_the_runtime() {
    let guard = HomeStoreGuard::set("ensure-blocked");
    assert_no_daemon_beside_the_test_binary();
    // The reuse arm only (C-12): the address the file names answers.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let port = listener.local_addr().expect("port").port();
    let file = BlockedFile::at(store::discovery_path().expect("discovery path"));
    let d = discovery::make_discovery("127.0.0.1", port, "instance-q11".into(), "0.0.0");
    let bytes = serde_json::to_vec(&d).expect("discovery as JSON");
    let (free, answer) = runtime_free_while_blocked(file, &bytes, ensure_daemon_running);
    drop(listener);
    assert!(
        free,
        "ensure_daemon_running held the runtime's thread while discovery.json would not open"
    );
    let info = answer.expect("reused");
    assert_connection_info(&serde_json::to_value(info).expect("as JSON"), &d);
    let log = store::daemon_log_path().expect("log path");
    assert!(log.starts_with(guard.store()));
    assert!(!log.exists(), "the spawn arm ran: {}", log.display());
}

// ============================================================================
// The liveness probe (Q12: tokio's connect under tokio's timeout)
// ============================================================================

/// `future` run to its end on a runtime of one thread.
fn on_one_thread<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a one-thread runtime")
        .block_on(future)
}

#[test]
fn the_liveness_probe_finds_an_address_that_answers() {
    let _guard = HomeStoreGuard::set("probe-live");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let port = listener.local_addr().expect("port").port();
    let d = discovery::make_discovery("127.0.0.1", port, "instance-q11".into(), "0.0.0");
    assert!(on_one_thread(daemon_is_alive(&d)));
}

#[test]
fn the_liveness_probe_finds_nothing_at_a_closed_port() {
    let _guard = HomeStoreGuard::set("probe-closed");
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        listener.local_addr().expect("port").port()
    };
    let d = discovery::make_discovery("127.0.0.1", port, "instance-q11".into(), "0.0.0");
    assert!(!on_one_thread(daemon_is_alive(&d)));
}

/// An address whose connect never completes costs the probe its timeout per
/// address and no more, and the probe says no.
#[test]
fn the_liveness_probe_gives_each_silent_address_its_timeout_and_no_more() {
    let _guard = HomeStoreGuard::set("probe-silent");
    assert_eq!(
        LIVENESS_PROBE_TIMEOUT,
        Duration::from_millis(300),
        "the probe's wait per address"
    );
    let addrs: [std::net::SocketAddr; 2] = [
        "127.0.0.1:1".parse().expect("an address"),
        "[::1]:1".parse().expect("an address"),
    ];
    let started = std::time::Instant::now();
    let answered = on_one_thread(any_address_answers(addrs, |_| {
        std::future::pending::<std::io::Result<()>>()
    }));
    let waited = started.elapsed();
    assert!(!answered);
    assert!(
        waited >= LIVENESS_PROBE_TIMEOUT * 2,
        "gave up after {waited:?}, before two timeouts"
    );
    assert!(
        waited < Duration::from_secs(5),
        "still waiting after {waited:?}"
    );
}

// ============================================================================
// The typed refusals (Q13)
// ============================================================================

/// One of each refusal, with the values `src/lib/shell-errors.json`'s rows
/// carry, in its order. The exhaustive match below only points the compiler
/// here when a variant is added; what forces its value (and so its row) is
/// `every_variant_in_the_source_has_a_refusal`, which reads the variants
/// from `lib.rs` itself.
fn every_refusal() -> Vec<ShellError> {
    let all = vec![
        ShellError::NotRunning {
            message: "Discovery not found: daemon may not be running".into(),
        },
        ShellError::Expired {
            message: "Discovery token has expired".into(),
        },
        ShellError::DiscoveryUnreadable {
            message: "Failed to parse discovery.json".into(),
        },
        did_not_start(
            Path::new("/s/state/logs/daemon.log"),
            "Error: this database is newer than this build".into(),
        ),
        ShellError::SpawnFailed {
            message: "Failed to spawn daemon: No such file or directory (os error 2)".into(),
            log_tail: None,
        },
        ShellError::LogUnreadable {
            message: "Could not read /s/state/logs/daemon.log: Is a directory (os error 21)".into(),
        },
        ShellError::StopUnconfirmed {
            message: "Could not wait for the daemon to stop: task 7 panicked".into(),
        },
        ShellError::RevealRefused {
            message: "/s/artifacts/gone.md is not there any more, so it cannot be shown in its \
                      folder."
                .into(),
        },
        ShellError::RevealFailed {
            message: "open could not show /s/artifacts/notes.md in its folder (exit status: 1)."
                .into(),
        },
    ];
    for error in &all {
        match error {
            ShellError::NotRunning { .. }
            | ShellError::Expired { .. }
            | ShellError::DiscoveryUnreadable { .. }
            | ShellError::SpawnFailed { .. }
            | ShellError::LogUnreadable { .. }
            | ShellError::StopUnconfirmed { .. }
            | ShellError::RevealRefused { .. }
            | ShellError::RevealFailed { .. } => {}
        }
    }
    all
}

/// Each refusal's wire form is its row in `src/lib/shell-errors.json`, the
/// table the webview's test reads too: the TypeScript mirror is held to the
/// same rows (`lib/shell-error.test.ts`).
#[test]
fn every_refusal_is_its_row_in_the_table_the_webview_reads() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/lib/shell-errors.json");
    let table: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("shell-errors.json")).expect("JSON");
    let rows = table["refusals"].as_array().expect("its refusals");
    let wire: Vec<Value> = every_refusal()
        .iter()
        .map(|e| serde_json::to_value(e).expect("as JSON"))
        .collect();
    assert_eq!(&wire, rows);
}

/// `ShellError`'s variants, read from `lib.rs`'s source, in snake_case: a
/// line indented four spaces inside `enum ShellError { … }` that starts with
/// a capital (doc comments, attributes and fields do not).
fn shell_error_kinds_in_the_source() -> BTreeSet<String> {
    let source = include_str!("lib.rs");
    let anchor = "\nenum ShellError {\n";
    assert_eq!(
        source.matches(anchor).count(),
        1,
        "lib.rs declares `enum ShellError {{` once"
    );
    let start = source.find(anchor).expect("the enum") + anchor.len();
    let end = source[start..].find("\n}\n").expect("the enum's end");
    let kinds: BTreeSet<String> = source[start..start + end]
        .lines()
        .filter_map(|line| line.strip_prefix("    "))
        .filter(|rest| rest.starts_with(|c: char| c.is_ascii_uppercase()))
        .map(|rest| {
            let name: String = rest
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            let mut kind = String::new();
            for (i, c) in name.chars().enumerate() {
                if c.is_ascii_uppercase() && i > 0 {
                    kind.push('_');
                }
                kind.push(c.to_ascii_lowercase());
            }
            kind
        })
        .collect();
    assert!(!kinds.is_empty(), "no variant read from lib.rs");
    kinds
}

/// Every variant `lib.rs` declares has a value in `every_refusal`, and so,
/// by the table test, a row in `shell-errors.json`, which the TypeScript
/// side's `SHELL_ERROR_KINDS` is held to: a new variant fails here until it
/// has both.
#[test]
fn every_variant_in_the_source_has_a_refusal() {
    let built: BTreeSet<String> = every_refusal()
        .iter()
        .map(|e| {
            serde_json::to_value(e).expect("as JSON")["kind"]
                .as_str()
                .expect("its kind")
                .to_owned()
        })
        .collect();
    assert_eq!(built, shell_error_kinds_in_the_source());
}

/// `message` is the sentence, and it is also the error's `Display`.
#[test]
fn a_refusal_displays_its_sentence() {
    for error in every_refusal() {
        let wire = serde_json::to_value(&error).expect("as JSON");
        assert_eq!(
            error.to_string(),
            wire["message"].as_str().expect("message")
        );
    }
}

/// A start that timed out: the two sentences, byte for byte, and the tail
/// beside the sentence only when the run wrote one.
#[test]
fn a_start_that_timed_out_quotes_what_this_run_wrote() {
    let log = Path::new("/s/state/logs/daemon.log");
    assert_eq!(
        serde_json::to_value(did_not_start(log, String::new())).expect("as JSON"),
        json!({
            "kind": "spawn_failed",
            "message": "The daemon did not start within 5 seconds, and wrote nothing to its log \
                        (/s/state/logs/daemon.log). Check that the daemon binary is installed \
                        beside the app.",
            "log_tail": null,
        })
    );
    assert_eq!(
        serde_json::to_value(did_not_start(log, "one\ntwo".into())).expect("as JSON"),
        json!({
            "kind": "spawn_failed",
            "message": "The daemon did not start within 5 seconds. Its log \
                        (/s/state/logs/daemon.log) ends with:\n\none\ntwo",
            "log_tail": "one\ntwo",
        })
    );
}

// ============================================================================
// The shell's log (Q14)
// ============================================================================

/// What `gui.log` holds now under the guard's scratch store.
fn gui_log_text() -> String {
    std::fs::read_to_string(store::gui_log_path().expect("the gui.log path")).unwrap_or_default()
}

/// `body` run with the shell's log subscriber as this thread's default,
/// over the file [`open_gui_log`] opened, at `info` whatever `RUST_LOG` the
/// test runs under. A thread's default, never the process's: a global one
/// would outlive this test's scratch root.
fn with_the_shell_log<T>(body: impl FnOnce() -> T) -> T {
    let (_, file, rotation) = open_gui_log().expect("gui.log opens");
    assert!(rotation.is_none(), "a fresh log rotates without failing");
    let subscriber = gui_log_subscriber(file, EnvFilter::new("info"));
    tracing::subscriber::with_default(subscriber, body)
}

/// `gui.log` is the store's `state/logs/gui.log`, under the scratch store
/// `OPENALPACA_HOME_STORE` names — never `~/Library/Logs` (D1) — and is
/// appended to, never truncated: the previous launch's lines stay.
#[test]
fn the_shell_log_is_the_stores_gui_log_and_appended_to() {
    let guard = HomeStoreGuard::set("gui-log-append");
    let path = store::gui_log_path().expect("the gui.log path");
    assert_eq!(path, guard.store().join("state/logs/gui.log"));
    std::fs::create_dir_all(path.parent().expect("logs dir")).expect("logs dir");
    std::fs::write(&path, "an earlier launch's line\n").expect("an earlier log");

    let (opened, _, _) = open_gui_log().expect("gui.log opens");
    assert_eq!(opened, path);
    with_the_shell_log(|| tracing::info!("this launch's line"));

    let text = gui_log_text();
    assert!(
        text.starts_with("an earlier launch's line\n"),
        "the earlier launch's line was kept: {text:?}"
    );
    assert!(text.contains("this launch's line"), "{text:?}");
    assert!(
        !text.contains('\u{1b}'),
        "no colour escapes in a file: {text:?}"
    );
}

/// A `gui.log` past the store's 16 MB cap is rotated to `gui.log.1` before
/// the launch opens a fresh one.
#[test]
fn the_shell_log_is_rotated_past_the_stores_cap_before_it_is_opened() {
    let _guard = HomeStoreGuard::set("gui-log-rotate");
    let path = store::gui_log_path().expect("the gui.log path");
    std::fs::create_dir_all(path.parent().expect("logs dir")).expect("logs dir");
    std::fs::File::create(&path)
        .expect("a full log")
        .set_len(store::DAEMON_LOG_MAX_BYTES + 1)
        .expect("a sparse 16 MB");

    let (_, file, rotation) = open_gui_log().expect("gui.log opens");
    assert!(rotation.is_none());
    assert_eq!(
        file.metadata().expect("metadata").len(),
        0,
        "a fresh gui.log"
    );
    let rotated = path.with_file_name("gui.log.1");
    assert_eq!(
        std::fs::metadata(&rotated).expect("gui.log.1").len(),
        store::DAEMON_LOG_MAX_BYTES + 1
    );
}

/// The startup marks in the one form a reader parses (plan-rev3 §7.5's
/// scenario (1), Q10): T0 first, then each page load's start and end, each
/// with the milliseconds since T0; a page's origin, never its path or query.
#[test]
fn the_startup_marks_are_logged_in_one_form() {
    let _guard = HomeStoreGuard::set("gui-log-marks");
    let t0 = Instant::now();
    let url: tauri::Url = "tauri://localhost/index.html?secret=1#chat"
        .parse()
        .expect("a page URL");
    with_the_shell_log(|| {
        t0_mark(t0);
        page_load_mark(t0, "main", PageLoadEvent::Started, &url);
        page_load_mark(t0, "main", PageLoadEvent::Finished, &url);
    });

    let text = gui_log_text();
    let marks: Vec<&str> = text
        .lines()
        .filter(|line| line.contains(" INFO openalpaca_gui::startup: startup mark "))
        .collect();
    assert_eq!(marks.len(), 3, "three marks: {text:?}");
    let (t0_line, started, finished) = (marks[0], marks[1], marks[2]);
    assert!(t0_line.contains(" mark=\"t0\" elapsed_ms="), "{t0_line}");
    assert!(
        t0_line.contains(&format!(" pid={}", std::process::id())),
        "{t0_line}"
    );
    for (line, mark) in [
        (started, "page_load_started"),
        (finished, "page_load_finished"),
    ] {
        assert!(
            line.contains(&format!(" mark=\"{mark}\" elapsed_ms=")),
            "{line}"
        );
        assert!(line.contains(" webview=\"main\""), "{line}");
        assert!(line.ends_with(" origin=tauri://localhost"), "{line}");
    }
    assert!(!text.contains("secret"), "no query in the log: {text:?}");
    assert!(!text.contains("index.html"), "no path in the log: {text:?}");

    // A dev launch's page keeps its port.
    let dev: tauri::Url = "http://localhost:1420/#/chat".parse().expect("a dev URL");
    assert_eq!(page_origin(&dev), "http://localhost:1420");
}

/// A web content process that ended is a line in `gui.log`, and the window
/// is reloaded, once; a reload that failed is a line too. The reload is
/// counted through [`log_and_reload`]'s parameter; the hook's own
/// `Webview::reload` over the mock runtime answers `Ok`; the real reload,
/// and the reconnect to the same `instance_id`, are G-bundle leg A's.
#[test]
fn a_web_content_process_that_ended_is_logged_and_the_window_reloaded() {
    const ENDED: &str = "The window's web content process ended; reloading the window.";
    let _guard = HomeStoreGuard::set("gui-log-webcontent");
    let (_app, webview) = mock_webview();
    let reloads = std::cell::Cell::new(0);
    with_the_shell_log(|| {
        reload_after_web_content_ended(webview.as_ref());
        log_and_reload("main", || {
            reloads.set(reloads.get() + 1);
            Ok(())
        });
    });
    assert_eq!(reloads.get(), 1, "the window is reloaded, once");

    let text = gui_log_text();
    let lines: Vec<&str> = text.lines().filter(|line| line.contains(ENDED)).collect();
    assert_eq!(lines.len(), 2, "a line for each ended process: {text:?}");
    for line in lines {
        assert!(line.contains(" WARN openalpaca_gui: "), "{line}");
        assert!(line.contains(" webview=\"main\""), "{line}");
    }
    assert!(
        !text.contains("Could not reload"),
        "the reload answered: {text:?}"
    );

    // A reload that fails is said, after the line for the ended process.
    with_the_shell_log(|| log_and_reload("main", || Err(tauri::Error::WebviewNotFound)));
    let text = gui_log_text();
    let failed = text
        .lines()
        .find(|line| {
            line.contains("Could not reload the window after its web content process ended")
        })
        .unwrap_or_else(|| panic!("no line for the failed reload: {text:?}"));
    assert!(failed.contains(" ERROR openalpaca_gui: "), "{failed}");
    assert!(failed.contains(" webview=\"main\""), "{failed}");
}

/// The panic hook writes a panic to `gui.log` under the scratch store
/// `OPENALPACA_HOME_STORE` names, as one line naming where and what — an
/// `assert_eq!`'s message of three lines too, its line breaks written `\n` —
/// and hands the panic on to the hook that was there before.
#[test]
fn the_panic_hook_writes_a_panic_to_gui_log() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let _guard = HomeStoreGuard::set("gui-log-panic");
    let (path, _, _) = open_gui_log().expect("gui.log opens");

    // The stand-in for the hook before the shell's counts this test's
    // panics only; another test's, on another thread, goes on to the
    // original hook, so its failure is still printed.
    let original: Arc<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync> =
        Arc::from(std::panic::take_hook());
    let this_thread = std::thread::current().id();
    let handed_on = Arc::new(AtomicUsize::new(0));
    let (seen, others) = (handed_on.clone(), original.clone());
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == this_thread {
            seen.fetch_add(1, Ordering::SeqCst);
        } else {
            others(info);
        }
    }));
    install_panic_hook(path);
    let line = line!() + 1;
    let caught = std::panic::catch_unwind(|| panic!("the shell's test panic {}", 14));
    let line_of_assert = line!() + 1;
    let caught_assert = std::panic::catch_unwind(|| assert_eq!(1 + 1, 3, "two lines"));
    // This test's hook and the one before it go; the original comes back.
    drop(std::panic::take_hook());
    std::panic::set_hook(Box::new(move |info| original(info)));

    assert!(caught.is_err() && caught_assert.is_err());
    assert_eq!(
        handed_on.load(Ordering::SeqCst),
        2,
        "the hook that was there before ran too, for each panic"
    );
    let text = gui_log_text();
    let thread = format!(
        " on thread '{}' ",
        std::thread::current().name().unwrap_or("<unnamed>")
    );
    let panics: Vec<&str> = text
        .lines()
        .filter(|l| l.contains(" ERROR openalpaca_gui::panic: the shell panicked "))
        .filter(|l| l.contains(&thread))
        .collect();
    assert_eq!(panics.len(), 2, "a line for each panic: {text:?}");
    let (panic_line, assert_line) = (panics[0], panics[1]);
    assert!(
        panic_line.contains(&format!("src/tests.rs:{line}:")),
        "where: {panic_line}"
    );
    assert!(
        panic_line.ends_with(": the shell's test panic 14"),
        "what: {panic_line}"
    );
    assert!(
        assert_line.contains(&format!("src/tests.rs:{line_of_assert}:")),
        "where: {assert_line}"
    );
    assert!(
        assert_line
            .ends_with(": assertion `left == right` failed: two lines\\n  left: 2\\n right: 3"),
        "the whole message, on its one line: {assert_line}"
    );
    // Every line of the log is one the subscriber's form begins: a time.
    for l in text.lines() {
        assert!(
            l.starts_with("20") && l.as_bytes().get(4) == Some(&b'-'),
            "a line without its time: {l:?} in {text:?}"
        );
    }
}

// ============================================================================
// The daemon's PATH (Q21)
// ============================================================================

use login_path::{
    Account, CAPTURE_COMMAND, Capture, CaptureFailure, DaemonPathReport, PathSource,
    capture_login_path, fallback_path,
};

/// A capture of `shell`'s `PATH`, the fallback list's home `home`: the
/// account as the capture's thread would resolve it, given, with no key
/// listed beside `PATH`.
fn capture_of(
    shell: Option<PathBuf>,
    home: Option<PathBuf>,
    deadline: Duration,
) -> std::sync::Arc<Capture> {
    capture_listing(shell, home, Vec::new(), deadline)
}

/// [`capture_of`] with `keys` listed beside `PATH` (N-DAEMON-2).
fn capture_listing(
    shell: Option<PathBuf>,
    home: Option<PathBuf>,
    keys: Vec<String>,
    deadline: Duration,
) -> std::sync::Arc<Capture> {
    Capture::start(move || Account { shell, home, keys }, deadline)
}

/// The `PATH` the noisy login shell exports, and the two values it exports
/// beside it that must never reach the daemon.
const CAPTURED_PATH: &str = "/q21/captured/bin:/usr/bin:/bin";
const CAPTURED_API_KEY: &str = "sk-q21-captured-not-passed";
const CAPTURED_PROFILE: &str = "q21-profile-captured-not-passed";
const CAPTURED_KUBECONFIG: &str = "/q21/kube/config-captured-not-passed";
const CAPTURED_GOOGLE: &str = "/q21/google/credentials-captured-not-passed.json";
const CAPTURED_PROFILE_BACKUP: &str = "q21-backup-captured-not-passed";

/// A stand-in login shell: a `/bin/sh` script at `<root>/<name>` running
/// `body`, which reaches the command it is given as `"$3"` (`-l -c <cmd>`).
/// Every program in it is named by its absolute path: the guard's `PATH`
/// is an empty folder.
#[cfg(unix)]
fn fake_login_shell(root: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("the fake login shell");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("executable");
    path
}

/// A login shell that prints noise around the markers — `PATH=` lines
/// before and after them, one NUL-terminated as `env -0` writes — and
/// exports an API key, the three credential pointers and
/// `AWS_PROFILE_BACKUP` (a name extending a listed one) beside its `PATH`;
/// it records each run's arguments.
#[cfg(unix)]
fn noisy_login_shell(root: &Path) -> PathBuf {
    let runs = root.join("runs");
    fake_login_shell(
        root,
        "noisy-shell",
        &format!(
            "printf '%s\\n' \"$@\" >> '{runs}'\n\
             echo 'PATH=/noise/before/bin'\n\
             PATH='{CAPTURED_PATH}'; export PATH\n\
             OPENAI_API_KEY='{CAPTURED_API_KEY}'; export OPENAI_API_KEY\n\
             AWS_PROFILE='{CAPTURED_PROFILE}'; export AWS_PROFILE\n\
             KUBECONFIG='{CAPTURED_KUBECONFIG}'; export KUBECONFIG\n\
             GOOGLE_APPLICATION_CREDENTIALS='{CAPTURED_GOOGLE}'; \
             export GOOGLE_APPLICATION_CREDENTIALS\n\
             AWS_PROFILE_BACKUP='{CAPTURED_PROFILE_BACKUP}'; export AWS_PROFILE_BACKUP\n\
             eval \"$3\"\n\
             echo 'PATH=/noise/after/bin'\n\
             printf 'PATH=/noise/after/nul/bin\\0'",
            runs = runs.display()
        ),
    )
}

/// A login shell that prints its environment, then fails.
#[cfg(unix)]
fn failing_login_shell(root: &Path) -> PathBuf {
    fake_login_shell(
        root,
        "failing-shell",
        &format!("PATH='{CAPTURED_PATH}'; export PATH\neval \"$3\"\nexit 3"),
    )
}

/// Every variable the daemon's command would start with: the app's own,
/// with what the command sets over them.
fn effective_env(cmd: &Command) -> std::collections::BTreeMap<OsString, OsString> {
    let mut env: std::collections::BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    for (key, value) in cmd.get_envs() {
        match value {
            Some(value) => env.insert(key.to_owned(), value.to_owned()),
            None => env.remove(key),
        };
    }
    env
}

/// Whether `pid` is still a process (a zombie counts until it is reaped).
#[cfg(unix)]
fn process_exists(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Kills `pid` on drop if it is still there, so a test that failed leaves
/// nothing running.
#[cfg(unix)]
struct KillOnDrop(i32);

#[cfg(unix)]
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if self.0 > 0 && process_exists(self.0) {
            // SAFETY: a plain signal to the test's own stray child.
            unsafe { libc::kill(self.0, libc::SIGKILL) };
        }
    }
}

/// The login shell's `PATH` is taken from between the markers and nothing
/// else: the noise around them, the `PATH=` lines among it, the API key and the
/// credential pointer it exports are not; it is run as `-l -c <the
/// command>`, once, however often the daemon asks. The daemon's command
/// gets that `PATH` and the source beside it, and neither exported value.
#[cfg(unix)]
#[test]
fn the_login_shells_path_and_nothing_else_reaches_the_daemons_command() {
    let guard = HomeStoreGuard::set("path-captured");
    let shell = noisy_login_shell(guard.root());
    let home = guard.root().join("home");
    let capture = capture_of(Some(shell), Some(home), Duration::from_secs(10));
    let path = capture.wait();

    assert_eq!(path.source, PathSource::LoginShell);
    assert_eq!(path.failure, None);
    assert_eq!(path.value(), &OsString::from(CAPTURED_PATH));
    let again = capture.wait();
    assert_eq!(again.value(), path.value());
    assert_eq!(
        again.capture, path.capture,
        "the one capture, answered again"
    );
    let runs = std::fs::read_to_string(guard.root().join("runs")).expect("the shell ran");
    assert_eq!(
        runs,
        format!("-l\n-c\n{CAPTURE_COMMAND}\n"),
        "run once, as a non-interactive login command"
    );

    let app_dir = guard.store();
    let config_dir = guard.store().join("config");
    let cmd = daemon_command(
        Path::new("/nonexistent/openalpacad"),
        &app_dir,
        &config_dir,
        Some(&path),
    );
    let set: std::collections::BTreeMap<_, _> = cmd
        .get_envs()
        .map(|(k, v)| (k.to_owned(), v.map(ToOwned::to_owned)))
        .collect();
    assert_eq!(
        set.keys().cloned().collect::<Vec<_>>(),
        [
            OsString::from("OPENALPACA_CONFIG_DIR"),
            OsString::from(store::MANAGED_LOG_ENV),
            OsString::from(store::PATH_SOURCE_ENV),
            OsString::from("PATH"),
        ],
        "the command sets these four and removes nothing"
    );
    assert_eq!(
        set[&OsString::from("PATH")],
        Some(OsString::from(CAPTURED_PATH))
    );
    assert_eq!(
        set[&OsString::from(store::PATH_SOURCE_ENV)],
        Some(OsString::from(format!("login-shell {}", path.capture_ms())))
    );
    assert_eq!(cmd.get_current_dir(), Some(app_dir.as_path()));
    assert_no_captured_value_but(&cmd, &[]);
}

/// The values the noisy login shell exports beside its `PATH`.
const CAPTURED_VALUES: [&str; 5] = [
    CAPTURED_API_KEY,
    CAPTURED_PROFILE,
    CAPTURED_KUBECONFIG,
    CAPTURED_GOOGLE,
    CAPTURED_PROFILE_BACKUP,
];

/// No value the noisy login shell exports beside its `PATH` is in the
/// daemon's effective environment, but those in `passed`.
fn assert_no_captured_value_but(cmd: &Command, passed: &[&str]) {
    for value in effective_env(cmd).values() {
        let value = value.to_string_lossy();
        for captured in CAPTURED_VALUES {
            assert!(
                passed.contains(&captured) || !value.contains(captured),
                "a captured value the list does not name reached the daemon ({captured})"
            );
        }
    }
}

/// A login shell still running at the deadline is killed with its process
/// group — the child it started is gone too — and the daemon gets the
/// fallback list, `~/.cargo/bin` last, under the home given.
#[cfg(unix)]
#[test]
fn a_shell_past_the_deadline_is_killed_with_its_group() {
    let guard = HomeStoreGuard::set("path-deadline");
    let pid_file = guard.root().join("child.pid");
    let shell = fake_login_shell(
        guard.root(),
        "hanging-shell",
        &format!("/bin/sleep 47 &\necho $! > '{}'\nwait", pid_file.display()),
    );
    let deadline = Duration::from_millis(1500);
    let started = Instant::now();
    let outcome = capture_login_path(&shell, started, deadline, &[]);
    let took = started.elapsed();
    let child: i32 = std::fs::read_to_string(&pid_file)
        .expect("the shell started its child")
        .trim()
        .parse()
        .expect("a pid");
    let _stray = KillOnDrop(child);

    assert_eq!(outcome, Err(CaptureFailure::TimedOut(deadline)));
    assert!(
        took >= deadline && took < deadline + Duration::from_millis(900),
        "settled at the deadline, not before or long after: {took:?}"
    );
    let gone_by = Instant::now() + Duration::from_secs(5);
    while process_exists(child) && Instant::now() < gone_by {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_exists(child),
        "the shell's child outlived the kill"
    );

    let home = guard.root().join("home");
    let path = capture_of(Some(shell), Some(home.clone()), deadline).wait();
    assert_eq!(path.source, PathSource::FallbackList);
    assert_eq!(path.failure, Some(CaptureFailure::TimedOut(deadline)));
    assert_eq!(
        path.value(),
        &OsString::from(format!(
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:{}/.cargo/bin",
            home.display()
        ))
    );
    let second: i32 = std::fs::read_to_string(&pid_file)
        .expect("the second run's child")
        .trim()
        .parse()
        .expect("a pid");
    let _stray_too = KillOnDrop(second);
    let gone_by = Instant::now() + Duration::from_secs(5);
    while process_exists(second) && Instant::now() < gone_by {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_exists(second),
        "the second shell's child outlived the kill"
    );
}

/// A login shell that exits non-zero gives the fallback list, though it
/// printed a `PATH`; one that prints no markers, or an environment with no
/// `PATH`, does too; no shell at all does too.
#[cfg(unix)]
#[test]
fn a_failed_capture_gives_the_fallback_list() {
    let guard = HomeStoreGuard::set("path-failed");
    let home = guard.root().join("home");
    let list = fallback_path(Some(&home));
    let deadline = Duration::from_secs(10);

    let failing = failing_login_shell(guard.root());
    let path = capture_of(Some(failing), Some(home.clone()), deadline).wait();
    assert_eq!(path.source, PathSource::FallbackList);
    assert_eq!(
        path.failure,
        Some(CaptureFailure::Exited("exit status: 3".into()))
    );
    assert_eq!(path.value(), &list);

    let silent = fake_login_shell(guard.root(), "silent-shell", "echo hello");
    let no_path = fake_login_shell(guard.root(), "no-path-shell", "unset PATH\neval \"$3\"");
    for (shell, failure) in [
        (Some(silent), CaptureFailure::NoMarkers),
        (Some(no_path), CaptureFailure::NoPath),
        (None, CaptureFailure::NoShell),
    ] {
        let path = capture_of(shell, Some(home.clone()), deadline).wait();
        assert_eq!(path.source, PathSource::FallbackList);
        assert_eq!(path.failure, Some(failure));
        assert_eq!(path.value(), &list);
    }
    assert_eq!(
        fallback_path(None),
        OsString::from("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
    );
}

/// The capture starts without waiting for the account's lookup or the login
/// shell (the window's first paint never waits on either: the lookup runs on
/// the capture's thread too), and a daemon spawned meanwhile waits for it at
/// most until its deadline, then gets the fallback list.
#[cfg(unix)]
#[test]
fn the_capture_holds_no_caller_and_the_wait_ends_at_its_deadline() {
    let guard = HomeStoreGuard::set("path-async");
    let pid_file = guard.root().join("shell.pid");
    let shell = fake_login_shell(
        guard.root(),
        "sleeping-shell",
        &format!("echo $$ > '{}'\nexec /bin/sleep 47", pid_file.display()),
    );
    let deadline = Duration::from_millis(1200);
    let before = Instant::now();
    let capture = Capture::start(
        move || {
            // A slow user database: the lookup holds the capture's thread.
            std::thread::sleep(Duration::from_millis(400));
            Account {
                shell: Some(shell),
                home: None,
                keys: Vec::new(),
            }
        },
        deadline,
    );
    assert!(
        before.elapsed() < Duration::from_millis(200),
        "start waited on the login shell: {:?}",
        before.elapsed()
    );
    let path = capture.wait();
    let waited = before.elapsed();
    if let Some(pid) = std::fs::read_to_string(&pid_file)
        .ok()
        .and_then(|text| text.trim().parse().ok())
    {
        drop(KillOnDrop(pid));
    }
    assert_eq!(path.source, PathSource::FallbackList);
    assert!(
        waited >= deadline && waited < deadline + Duration::from_millis(900),
        "the wait ended at the deadline: {waited:?}"
    );
}

/// The wait's own bound: a capture whose thread has concluded nothing by
/// the deadline (here its account's lookup is still running) is taken as
/// timed out there by the wait itself — the fallback list, without
/// `~/.cargo/bin` while the home is unknown — and every later wait answers
/// the same, the thread's own conclusion after it ignored.
#[cfg(unix)]
#[test]
fn the_wait_takes_the_fallback_list_itself_at_the_deadline() {
    let guard = HomeStoreGuard::set("path-wait-bound");
    let home = guard.root().join("home");
    let deadline = Duration::from_millis(600);
    let before = Instant::now();
    let capture = Capture::start(
        move || {
            std::thread::sleep(Duration::from_secs(4));
            Account {
                shell: None,
                home: Some(home),
                keys: Vec::new(),
            }
        },
        deadline,
    );
    let path = capture.wait();
    let waited = before.elapsed();

    assert!(
        waited >= deadline && waited < deadline + Duration::from_millis(900),
        "the wait ended at the deadline: {waited:?}"
    );
    assert_eq!(path.source, PathSource::FallbackList);
    assert_eq!(path.failure, Some(CaptureFailure::TimedOut(deadline)));
    assert_eq!(path.value(), &fallback_path(None));
    let again = capture.wait();
    assert_eq!(again.failure, path.failure);
    assert_eq!(
        again.capture, path.capture,
        "the same conclusion, answered again"
    );
}

/// The capture is settled when the login shell exits, not when its output
/// ends: a child the profile put in the background, holding the shell's
/// standard output open, holds nothing — the `PATH` is taken at once, long
/// before the deadline.
#[cfg(unix)]
#[test]
fn the_capture_settles_on_the_shells_exit_not_the_end_of_its_output() {
    let guard = HomeStoreGuard::set("path-settle-on-exit");
    let pid_file = guard.root().join("background.pid");
    let shell = fake_login_shell(
        guard.root(),
        "backgrounding-shell",
        &format!(
            "/bin/sleep 47 &\n\
             echo $! > '{pid}'\n\
             PATH='{CAPTURED_PATH}'; export PATH\n\
             eval \"$3\"",
            pid = pid_file.display()
        ),
    );
    let deadline = Duration::from_secs(10);
    let (sent, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let started = Instant::now();
        let outcome = capture_login_path(&shell, started, deadline, &[]);
        let _ = sent.send((outcome, started.elapsed()));
    });
    let answer = received.recv_timeout(Duration::from_secs(5));
    let background: i32 = std::fs::read_to_string(&pid_file)
        .expect("the shell started its background child")
        .trim()
        .parse()
        .expect("a pid");
    let _stray = KillOnDrop(background);

    let (outcome, took) = answer.expect("the capture settled while the child held the pipe");
    assert_eq!(
        outcome.map(|captured| captured.path),
        Ok(OsString::from(CAPTURED_PATH))
    );
    assert!(
        took < Duration::from_secs(2),
        "settled on the exit: {took:?}"
    );
}

/// The announcement names the source and the capture's duration and never
/// the value: `gui.log`'s lines for a captured `PATH` and for the fallback
/// list, and the variable the daemon's boot line reads.
#[cfg(unix)]
#[test]
fn the_announcement_names_the_source_and_the_duration_never_the_value() {
    let guard = HomeStoreGuard::set("path-announced");
    let home = guard.root().join("home");
    let noisy = noisy_login_shell(guard.root());
    let failing = failing_login_shell(guard.root());
    let deadline = Duration::from_secs(10);
    let (captured, fallback) = with_the_shell_log(|| {
        let captured = capture_of(Some(noisy), Some(home.clone()), deadline).wait();
        announce_daemon_path(&captured);
        let fallback = capture_of(Some(failing), Some(home.clone()), deadline).wait();
        announce_daemon_path(&fallback);
        (captured, fallback)
    });

    let text = gui_log_text();
    for (path, level, source) in [
        (&captured, " INFO ", "login-shell"),
        (&fallback, " WARN ", "fallback-list"),
    ] {
        let fields = format!("source=\"{source}\" capture_ms={}", path.capture_ms());
        assert!(
            text.lines()
                .any(|l| l.contains(level) && l.contains(&fields)),
            "the capture's line, {fields}: {text:?}"
        );
        assert!(
            text.lines()
                .any(|l| l.contains("Daemon PATH source") && l.contains(&fields)),
            "the spawn's line, {fields}: {text:?}"
        );
        assert_eq!(
            path.announcement(),
            format!("{source} {}", path.capture_ms())
        );
    }
    assert!(
        text.contains("reason=the login shell failed (exit status: 3)"),
        "{text:?}"
    );
    for value in [
        CAPTURED_PATH,
        "/q21/captured",
        "/opt/homebrew/bin",
        ".cargo/bin",
    ] {
        assert!(
            !text.contains(value),
            "gui.log names the PATH ({value}): {text:?}"
        );
    }
}

/// The connection answers `daemonPath` for the daemon this run spawned —
/// its `pid` the one `discovery.json` names — and `null` for any other.
#[test]
fn the_connection_names_the_path_of_the_daemon_this_app_spawned() {
    let _guard = HomeStoreGuard::set("path-carried");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let d = write_discovery(listener.local_addr().expect("port").port(), false);
    let report = DaemonPathReport {
        source: PathSource::LoginShell,
        capture_ms: 212,
        reason: None,
        passed_keys: Vec::new(),
    };

    record_spawned(d.pid, Some(report));
    for cmd in ["ensure_daemon_running", "get_connection_info"] {
        let answer = invoke(cmd, json!({})).expect("connection info");
        assert_eq!(
            answer["daemonPath"],
            json!({
                "source": "login-shell",
                "captureMs": 212,
                "reason": null,
                "passedKeys": []
            }),
            "{cmd}"
        );
        assert_eq!(answer["instanceId"], "instance-q11");
    }

    forget_spawned();
    record_spawned(
        d.pid.wrapping_add(1),
        Some(DaemonPathReport {
            source: PathSource::LoginShell,
            capture_ms: 212,
            reason: None,
            passed_keys: Vec::new(),
        }),
    );
    for cmd in ["ensure_daemon_running", "get_connection_info"] {
        let answer = invoke(cmd, json!({})).expect("connection info");
        assert_connection_info(&answer, &d);
    }
    drop(listener);
}

/// Two starts that overlap each spawn a daemon, and the one `discovery.json`
/// names — the one that took the store's lock — may be the first: a later
/// spawn does not take its `daemonPath` away.
#[test]
fn a_daemon_this_run_spawned_keeps_its_path_after_a_later_spawn() {
    let _guard = HomeStoreGuard::set("path-two-spawns");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let d = write_discovery(listener.local_addr().expect("port").port(), false);
    let report = DaemonPathReport {
        source: PathSource::FallbackList,
        capture_ms: 3000,
        reason: Some(CaptureFailure::TimedOut(Duration::from_secs(3)).to_string()),
        passed_keys: Vec::new(),
    };

    record_spawned(d.pid, Some(report.clone()));
    record_spawned(d.pid.wrapping_add(1), Some(report));
    for cmd in ["ensure_daemon_running", "get_connection_info"] {
        let answer = invoke(cmd, json!({})).expect("connection info");
        assert_eq!(
            answer["daemonPath"]["source"],
            json!("fallback-list"),
            "{cmd}: the first spawn's daemon keeps its PATH line"
        );
    }
    drop(listener);
}

/// Each `daemonPath`'s wire form is its row in `src/lib/daemon-path.json`,
/// which the TypeScript side reads too; a failure's reason is its sentence.
#[test]
fn every_daemon_path_is_its_row_in_the_table_the_webview_reads() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/lib/daemon-path.json");
    let table: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("daemon-path.json")).expect("JSON");
    let rows = [
        (
            "login_shell",
            DaemonPathReport {
                source: PathSource::LoginShell,
                capture_ms: 212,
                reason: None,
                passed_keys: Vec::new(),
            },
        ),
        (
            "login_shell_with_keys",
            DaemonPathReport {
                source: PathSource::LoginShell,
                capture_ms: 212,
                reason: None,
                passed_keys: vec!["AWS_PROFILE".into(), "KUBECONFIG".into()],
            },
        ),
        (
            "fallback_list",
            DaemonPathReport {
                source: PathSource::FallbackList,
                capture_ms: 3000,
                reason: Some(CaptureFailure::TimedOut(Duration::from_secs(3)).to_string()),
                passed_keys: Vec::new(),
            },
        ),
    ];
    assert_eq!(
        table.as_object().expect("an object").len(),
        rows.len(),
        "one row per case"
    );
    for (name, report) in rows {
        assert_eq!(
            serde_json::to_value(&report).expect("serializes"),
            table[name],
            "{name}"
        );
    }
}

// ============================================================================
// The credential-pointer keys (N-DAEMON-2)
// ============================================================================

use login_path::{configured_passthrough_keys, passthrough_keys};

/// Write `body` as the scratch store's `config/daemon.toml`, the file the
/// daemon the app spawns reads (`OPENALPACA_CONFIG_DIR`), and return its
/// folder.
fn write_daemon_toml(guard: &HomeStoreGuard, body: &str) -> PathBuf {
    let config_dir = guard.store().join("config");
    std::fs::write(config_dir.join("daemon.toml"), body).expect("daemon.toml");
    config_dir
}

/// The variables the daemon's command sets, by name, with their values.
fn set_on(cmd: &Command) -> std::collections::BTreeMap<OsString, Option<OsString>> {
    cmd.get_envs()
        .map(|(k, v)| (k.to_owned(), v.map(ToOwned::to_owned)))
        .collect()
}

/// The command for the daemon given `path`, under the guard's store.
fn command_for(guard: &HomeStoreGuard, path: &login_path::DaemonPath) -> Command {
    daemon_command(
        Path::new("/nonexistent/openalpacad"),
        &guard.store(),
        &guard.store().join("config"),
        Some(path),
    )
}

/// The entry's first case: a login shell exporting `AWS_PROFILE`,
/// `KUBECONFIG` and `GOOGLE_APPLICATION_CREDENTIALS`, with
/// `[daemon.env] passthrough_keys = []`, passes none of them — the command
/// sets the four variables Q21 sets and nothing else.
#[cfg(unix)]
#[test]
fn with_the_list_empty_no_credential_pointer_reaches_the_daemon() {
    let guard = HomeStoreGuard::set("keys-empty");
    write_daemon_toml(&guard, "[daemon.env]\npassthrough_keys = []\n");
    let keys = configured_passthrough_keys();
    assert!(keys.is_empty(), "{keys:?}");
    let shell = noisy_login_shell(guard.root());
    let path = capture_listing(Some(shell), None, keys, Duration::from_secs(10)).wait();

    assert_eq!(path.source, PathSource::LoginShell);
    assert!(path.passed_keys().is_empty());
    let cmd = command_for(&guard, &path);
    assert_eq!(
        set_on(&cmd).keys().cloned().collect::<Vec<_>>(),
        [
            OsString::from("OPENALPACA_CONFIG_DIR"),
            OsString::from(store::MANAGED_LOG_ENV),
            OsString::from(store::PATH_SOURCE_ENV),
            OsString::from("PATH"),
        ]
    );
    assert_eq!(
        path.announcement(),
        format!("login-shell {}", path.capture_ms())
    );
    assert_no_captured_value_but(&cmd, &[]);
}

/// The entry's second case: with `passthrough_keys = ["AWS_PROFILE"]` the
/// daemon's command sets `AWS_PROFILE` to the login shell's value beside
/// `PATH`, and `KUBECONFIG` and `GOOGLE_APPLICATION_CREDENTIALS`, which the
/// shell exports too, are not passed. A listed key is matched by its whole
/// name: `AWS`, listed and not exported, is not taken from `AWS_PROFILE`
/// (whatever order `env` prints them in), and `AWS_PROFILE_BACKUP`, which
/// the shell exports, neither reaches the daemon nor stands in for
/// `AWS_PROFILE`'s value.
#[cfg(unix)]
#[test]
fn a_listed_key_reaches_the_daemon_and_an_unlisted_one_does_not() {
    let guard = HomeStoreGuard::set("keys-listed");
    write_daemon_toml(
        &guard,
        "[execution.artifacts]\n\n[daemon.env]\npassthrough_keys = [\"AWS_PROFILE\", \"AWS\"]\n",
    );
    let keys = configured_passthrough_keys();
    assert_eq!(keys, ["AWS_PROFILE", "AWS"]);
    let shell = noisy_login_shell(guard.root());
    let path = capture_listing(Some(shell), None, keys, Duration::from_secs(10)).wait();

    assert_eq!(path.source, PathSource::LoginShell);
    assert_eq!(path.value(), &OsString::from(CAPTURED_PATH));
    assert_eq!(path.passed_keys(), ["AWS_PROFILE"]);
    let cmd = command_for(&guard, &path);
    let set = set_on(&cmd);
    assert_eq!(
        set.keys().cloned().collect::<Vec<_>>(),
        [
            OsString::from("AWS_PROFILE"),
            OsString::from("OPENALPACA_CONFIG_DIR"),
            OsString::from(store::MANAGED_LOG_ENV),
            OsString::from(store::PATH_SOURCE_ENV),
            OsString::from("PATH"),
        ],
        "the listed key beside Q21's four, and nothing removed"
    );
    assert_eq!(
        set[&OsString::from("AWS_PROFILE")],
        Some(OsString::from(CAPTURED_PROFILE))
    );
    assert_no_captured_value_but(&cmd, &[CAPTURED_PROFILE]);
    assert_eq!(
        path.report().passed_keys,
        ["AWS_PROFILE"],
        "Settings → Connection is told the key by name"
    );
}

/// The entry's third case: the announcement names the keys passed and never
/// their values — `gui.log`'s capture and spawn lines, the variable the
/// daemon's boot line reads, the connection's `passedKeys` — and a listed
/// key the login shell does not export is named as absent. A capture that
/// failed passes no key.
#[cfg(unix)]
#[test]
fn the_announcement_names_the_keys_passed_never_their_values() {
    let guard = HomeStoreGuard::set("keys-announced");
    let noisy = noisy_login_shell(guard.root());
    let failing = failing_login_shell(guard.root());
    let keys = vec!["AWS_PROFILE".to_owned(), "Q21_NOT_EXPORTED".to_owned()];
    let deadline = Duration::from_secs(10);
    let (passed, fallback) = with_the_shell_log(|| {
        let passed = capture_listing(Some(noisy), None, keys.clone(), deadline).wait();
        announce_daemon_path(&passed);
        let fallback = capture_listing(Some(failing), None, keys.clone(), deadline).wait();
        announce_daemon_path(&fallback);
        (passed, fallback)
    });

    assert_eq!(
        passed.announcement(),
        format!("login-shell {} AWS_PROFILE", passed.capture_ms())
    );
    assert_eq!(fallback.source, PathSource::FallbackList);
    assert!(fallback.passed_keys().is_empty());
    assert_eq!(
        fallback.announcement(),
        format!("fallback-list {}", fallback.capture_ms())
    );
    let text = gui_log_text();
    let captured = format!(
        "source=\"login-shell\" capture_ms={} passed_keys=\"AWS_PROFILE\" \
         absent_keys=\"Q21_NOT_EXPORTED\"",
        passed.capture_ms()
    );
    assert!(
        text.lines()
            .any(|l| l.contains("The daemon's PATH is the login shell's.") && l.contains(&captured)),
        "the capture's line, {captured}: {text:?}"
    );
    assert!(
        text.lines()
            .any(|l| l.contains("Daemon PATH source") && l.contains("passed_keys=\"AWS_PROFILE\"")),
        "the spawn's line names the key: {text:?}"
    );
    assert!(
        text.lines()
            .any(|l| l.contains("The daemon's PATH is the fallback list.")
                && l.contains("passed_keys=\"\"")),
        "a failed capture passes no key: {text:?}"
    );
    for value in CAPTURED_VALUES {
        assert!(
            !text.contains(value),
            "gui.log names a captured value ({value}): {text:?}"
        );
    }
    let report = serde_json::to_value(passed.report()).expect("serializes");
    assert_eq!(report["passedKeys"], json!(["AWS_PROFILE"]));
    assert!(!report.to_string().contains(CAPTURED_PROFILE));
}

/// The list takes variable names and nothing else: an entry that is not a
/// string or not a name — `KEY=value` among them, whose value is never
/// written to `gui.log` — is dropped, as is a variable the app sets on the
/// daemon itself (`PATH` and its three own); a name listed twice is passed
/// once; no file, no table, or a file that is not TOML is an empty list.
#[test]
fn the_list_takes_variable_names_only() {
    let guard = HomeStoreGuard::set("keys-list");
    let config_dir = guard.store().join("config");
    let read = |body: Option<&str>| {
        let file = config_dir.join("daemon.toml");
        match body {
            Some(body) => std::fs::write(&file, body).expect("daemon.toml"),
            None => {
                let _ = std::fs::remove_file(&file);
            }
        }
        passthrough_keys(&config_dir)
    };
    let lists = with_the_shell_log(|| {
        [
            read(None),
            read(Some("")),
            read(Some("[daemon]\nlog = 1\n")),
            read(Some(
                "[daemon.env]\npassthrough_keys = [\"KUBECONFIG\", \"AWS_PROFILE\", \"KUBECONFIG\"]\n",
            )),
            read(Some(
                "[daemon.env]\npassthrough_keys = [\"AWS_PROFILE=q21-value-in-the-list\", 7, \
                 \"\", \"1BAD\", \"BAD-NAME\", \"PATH\", \"OPENALPACA_CONFIG_DIR\", \
                 \"OPENALPACA_MANAGED_LOG\", \"OPENALPACA_PATH_SOURCE\", \
                 \"GOOGLE_APPLICATION_CREDENTIALS\"]\n",
            )),
            read(Some("[daemon.env]\npassthrough_keys = \"AWS_PROFILE\"\n")),
            read(Some("daemon = 3\n")),
            read(Some("[daemon.env\npassthrough_keys = [\"AWS_PROFILE\"]\n")),
        ]
    });
    let empty: Vec<String> = Vec::new();
    assert_eq!(
        lists,
        [
            empty.clone(),
            empty.clone(),
            empty.clone(),
            vec!["KUBECONFIG".to_owned(), "AWS_PROFILE".to_owned()],
            vec!["GOOGLE_APPLICATION_CREDENTIALS".to_owned()],
            empty.clone(),
            empty.clone(),
            empty,
        ]
    );
    let text = gui_log_text();
    assert!(!text.contains("q21-value-in-the-list"), "{text:?}");
    for name in [
        "PATH",
        "OPENALPACA_CONFIG_DIR",
        "OPENALPACA_MANAGED_LOG",
        "OPENALPACA_PATH_SOURCE",
    ] {
        assert!(
            text.lines()
                .any(|l| l.contains(" WARN ") && l.contains(&format!("key=\"{name}\""))),
            "the app's own {name} is named as not passed: {text:?}"
        );
    }
    assert!(
        text.lines()
            .any(|l| l.contains(" WARN ") && l.contains("not a variable name")),
        "{text:?}"
    );
}

/// The `daemon.toml` the daemon seeds into a new store names the list, and
/// names no key: by default nothing but `PATH` is passed.
#[test]
fn the_seeded_daemon_toml_lists_no_key() {
    let guard = HomeStoreGuard::set("keys-seeded");
    let seeded = include_str!("../../../../scripts/release/templates/config/daemon.toml");
    let table: toml::Table = seeded.parse().expect("the template is TOML");
    assert_eq!(
        table["daemon"]["env"]["passthrough_keys"],
        toml::Value::Array(Vec::new())
    );
    let config_dir = write_daemon_toml(&guard, seeded);
    assert!(passthrough_keys(&config_dir).is_empty());
}

/// The account the app's own capture starts with carries the list the
/// store's `daemon.toml` names: the one place `start()` reads it.
#[cfg(unix)]
#[test]
fn the_apps_capture_reads_the_list_from_the_stores_daemon_toml() {
    let guard = HomeStoreGuard::set("keys-account");
    write_daemon_toml(
        &guard,
        "[daemon.env]\npassthrough_keys = [\"KUBECONFIG\", \"AWS_PROFILE\"]\n",
    );
    assert_eq!(
        login_path::this_account().keys,
        ["KUBECONFIG", "AWS_PROFILE"]
    );
}
