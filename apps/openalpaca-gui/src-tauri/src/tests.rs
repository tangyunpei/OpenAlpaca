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

/// A refusal's text: the shell's own are one string each.
fn refusal(answer: Result<Value, Value>) -> String {
    match answer.expect_err("a refusal") {
        Value::String(text) => text,
        other => panic!("a refusal that is not a string: {other}"),
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

/// `{ baseUrl, token, instanceId }`, exactly those keys.
fn assert_connection_info(answer: &Value, d: &Discovery) {
    let mut keys: Vec<&str> = answer
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["baseUrl", "instanceId", "token"]);
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
        "Discovery not found: daemon may not be running"
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
        "Discovery token has expired"
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

#[test]
fn read_daemon_log_tail_refuses_a_call_without_lines() {
    let _guard = HomeStoreGuard::set("log-no-arg");
    let text = refusal(invoke("read_daemon_log_tail", json!({ "count": 2 })));
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
        format!(
            "{} is not there any more, so it cannot be shown in its folder.",
            gone.display()
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
        format!(
            "{} is not a file, so it cannot be shown in its folder.",
            folder.display()
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
        format!(
            "{} is not in an OpenAlpaca store or one of your projects, so it is not shown in \
             its folder.",
            outside.display()
        )
    );
}

#[test]
fn reveal_path_refuses_a_call_without_path() {
    let _guard = HomeStoreGuard::set("reveal-no-arg");
    let text = refusal(invoke("reveal_path", json!({ "file": "/x" })));
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
