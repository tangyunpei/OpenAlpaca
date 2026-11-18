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
//! **Nothing here spawns a daemon (C-12).** The sidecar is only ever the
//! `openalpacad` beside the app's canonical path (Q04, D7 (a)) — the debug
//! build's fallback to `PATH` is gone — and none sits beside the test
//! binary, so `ensure_daemon_running`'s spawn arm, reached, refuses at the
//! locator before it touches the store or the log. The tests drive its reuse
//! of a discovery file whose address an in-test server answers `GET
//! /v1/health` from ([`FakeServer`]), its `starting` answer for a stand-in
//! daemon process that does not answer yet, and that refusal; every path
//! past a spawn — the wait, the CLI's daemon winning the lock — is
//! `reuse::ensure_with`'s, driven over a world its own tests write
//! (`reuse/tests.rs`). `reveal_path`'s admitted path, which runs `open -R`
//! (Finder), has no test. As a backstop the guard points `PATH` at an empty
//! folder, and the reuse tests check that no `openalpacad` sits beside the
//! test binary and that no daemon log was opened.
//!
//! The Q21 capture tests do spawn, but no daemon: each runs a stand-in
//! `/bin/sh` login shell or, on macOS, the real `/bin/zsh` or `/bin/bash`
//! under the guard's scratch `HOME` with `ZDOTDIR` removed (LOGIN-CAPTURE),
//! and some start a `/bin/sleep 47` from it; the capture kills the shell's
//! process group when the shell exits or at its deadline, and `KillOnDrop`
//! kills a child still there when its test ends.
//!
//! Every test holds [`HomeStoreGuard`], this crate's copy of the home-store
//! guard: a scratch `OPENALPACA_HOME_STORE`, `OPENALPACA_CONFIG_DIR`, `HOME`
//! and `PATH`, never `~/.openalpaca`.

use super::reuse::{HEALTH_PROBE_TIMEOUT, first_answer, probe_health};
use super::*;
use openalpaca_app_support::discovery::Discovery;
use openalpaca_app_support::paths;
use openalpaca_app_support::protocol::Health;
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
    paths::HOME_STORE_ENV,
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
        // No test inherits another's spawned daemon (Q21's `daemonPath`), nor
        // the first silent look at another's starting daemon (Q04).
        forget_spawned();
        super::reuse::STARTING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
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
    let mut d = discovery::make_discovery("127.0.0.1", port, "instance-q11".into(), SHELL_VERSION);
    if expired {
        let mut value = serde_json::to_value(&d).expect("discovery as JSON");
        value["auth"]["expires_at"] = json!("2000-01-01T00:00:00Z");
        d = serde_json::from_value(value).expect("expired discovery");
    }
    discovery::write_discovery_atomic(&d).expect("write discovery.json");
    d
}

/// The version every stand-in daemon here reports: the shell's own, so no
/// answer carries Q04's note on another version unless a test asks for one.
const SHELL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `{ baseUrl, token, instanceId, version, protocol, daemonPath, note }`,
/// exactly those keys, with `daemonPath` `null` (no test spawns, so no daemon
/// is this run's, Q21), the daemon's `version` and `protocol` (Q04's
/// additive fields) and no `note`: the daemon speaks this shell's protocol
/// and is its version.
fn assert_connection_info(answer: &Value, d: &Discovery) {
    let mut keys: Vec<&str> = answer
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "baseUrl",
            "daemonPath",
            "instanceId",
            "note",
            "protocol",
            "token",
            "version"
        ]
    );
    assert_eq!(answer["daemonPath"], Value::Null);
    assert_eq!(answer["note"], Value::Null);
    assert_eq!(answer["version"], d.build.version.as_str());
    assert_eq!(answer["protocol"], d.build.protocol);
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
    let exe = std::env::current_exe()
        .expect("test binary")
        .canonicalize()
        .expect("canonical test binary");
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
    let made = discovery::make_discovery("127.0.0.1", 1, "instance-q11".into(), SHELL_VERSION);
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
    let path = paths::discovery_path().expect("discovery path");
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
    let path = paths::discovery_path().expect("discovery path");
    std::fs::create_dir_all(&path).expect("a folder at discovery.json's path");
    assert_eq!(
        refusal(invoke("get_connection_info", json!({}))),
        shell_refusal("discovery_unreadable", "Failed to read discovery.json")
    );
}

/// The reconnect ladder's rung reads the file, not health (Q04): a file
/// whose `build.protocol` this shell does not work with, over a live
/// `openalpacad`, is refused as the mismatch `ensure_daemon_running` would
/// give, so the window parks instead of connecting to a daemon it cannot use.
#[cfg(unix)]
#[test]
fn get_connection_info_refuses_a_live_daemon_of_a_protocol_it_does_not_speak() {
    let guard = HomeStoreGuard::set("info-mismatch");
    let daemon = FakeDaemonProcess::start(&guard);
    let mut d = write_discovery_for(daemon.pid, closed_port());
    d.build.protocol = openalpaca_app_support::protocol::PROTOCOL + 1;
    d.build.version = "9.9.9".into();
    discovery::write_discovery_atomic(&d).expect("write discovery.json");

    let refused = refusal(invoke("get_connection_info", json!({})));
    drop(daemon);
    assert_eq!(refused["kind"], "mismatch", "{refused}");
    assert_eq!(
        refused["daemon_protocol"],
        openalpaca_app_support::protocol::PROTOCOL + 1
    );
    assert_eq!(refused["daemon_version"], "9.9.9");
    assert_eq!(
        refused["shell_protocol"],
        openalpaca_app_support::protocol::PROTOCOL
    );
    let message = refused["message"].as_str().expect("a sentence");
    assert!(
        message.starts_with(&format!(
            "The daemon running (pid {}, version 9.9.9, protocol {})",
            d.pid,
            openalpaca_app_support::protocol::PROTOCOL + 1
        )),
        "{message}"
    );
    assert_nothing_spawned(&guard);
}

/// A live daemon of another protocol for the ladder to refuse: version
/// 9.9.9, this shell's protocol plus one, at a closed port.
#[cfg(unix)]
fn write_newer_discovery_for(pid: u32) -> Discovery {
    let mut d = write_discovery_for(pid, closed_port());
    d.build.protocol = openalpaca_app_support::protocol::PROTOCOL + 1;
    d.build.version = "9.9.9".into();
    discovery::write_discovery_atomic(&d).expect("write discovery.json");
    d
}

/// LADDER-LINE (*Rev 114* (3) (b)): the ladder's refusal is the asking
/// window's verdict, by its label (`main`, the mock webview's), in the
/// app's one record — the one `ensure_daemon_running` keeps — so the same
/// mismatch for that window logs no second line, and another window's
/// does.
#[cfg(unix)]
#[test]
fn get_connection_info_s_mismatch_is_the_asking_window_s_verdict() {
    let guard = HomeStoreGuard::set("info-mismatch-verdict");
    let daemon = FakeDaemonProcess::start(&guard);
    let d = write_newer_discovery_for(daemon.pid);

    let refused = refusal(invoke("get_connection_info", json!({})));
    drop(daemon);
    assert_eq!(refused["kind"], "mismatch", "{refused}");
    let mismatch = openalpaca_app_support::protocol::file_mismatch(&d, &reuse::THIS_SHELL)
        .expect("a file of another protocol");
    let mut lines = Vec::new();
    reuse::ladder_mismatch(&reuse::STARTING, "main", &mismatch, |l| lines.push(l));
    assert_eq!(
        lines,
        Vec::<String>::new(),
        "`main`'s verdict was not recorded"
    );
    reuse::ladder_mismatch(&reuse::STARTING, "second", &mismatch, |l| lines.push(l));
    assert_eq!(
        lines,
        [format!(
            "Daemon verdict for window `second`: refusal (mismatch) — pid {}, version 9.9.9, \
             protocol {}; this app speaks protocol {}; nothing was stopped or started.",
            d.pid,
            openalpaca_app_support::protocol::PROTOCOL + 1,
            openalpaca_app_support::protocol::PROTOCOL
        )]
    );
}

/// The ladder's refusal is one `INFO` line of `gui.log`, asked again none.
#[cfg(unix)]
#[test]
fn the_ladder_s_mismatch_is_one_line_of_gui_log() {
    let guard = HomeStoreGuard::set("info-mismatch-gui-log");
    let daemon = FakeDaemonProcess::start(&guard);
    let d = write_newer_discovery_for(daemon.pid);
    let kinds = with_the_shell_log(|| {
        (0..3)
            .map(|_| {
                let refused = read_connection_info("main").expect_err("refused");
                serde_json::to_value(refused).expect("as JSON")["kind"].clone()
            })
            .collect::<Vec<_>>()
    });
    drop(daemon);
    assert_eq!(kinds, vec![json!("mismatch"); 3]);
    let line = format!(
        "Daemon verdict for window `main`: refusal (mismatch) — pid {}, version 9.9.9",
        d.pid
    );
    assert_eq!(gui_log_count(&line), 1, "{}", gui_log_text());
    assert!(
        gui_log_text()
            .lines()
            .any(|l| l.contains(" INFO ") && l.contains(&line)),
        "{}",
        gui_log_text()
    );
}

/// The same file over a pid that is no daemon (here the test's own) is
/// answered as before Q04: a mismatch is never said of a process that is
/// gone, and the socket's failure keeps the ladder going.
#[test]
fn get_connection_info_answers_a_stale_file_of_another_protocol_as_before() {
    let _guard = HomeStoreGuard::set("info-mismatch-stale");
    let mut d = write_discovery_for(std::process::id(), 1);
    d.build.protocol = openalpaca_app_support::protocol::PROTOCOL + 1;
    discovery::write_discovery_atomic(&d).expect("write discovery.json");

    let answer = invoke("get_connection_info", json!({})).expect("connection info");
    assert_eq!(
        answer["protocol"],
        openalpaca_app_support::protocol::PROTOCOL + 1
    );
    assert_eq!(answer["note"], Value::Null);
}

// ============================================================================
// ensure_daemon_running against a stand-in daemon (Q04)
// ============================================================================

/// A loopback listener standing in for a daemon's address: every connection
/// gets `response`, written once whole, after the request's headers are read
/// (or 2 s pass), and then closed. It stops when dropped.
struct FakeServer {
    port: u16,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeServer {
    fn answering(response: String) -> Self {
        Self::serving(response, false)
    }

    /// As [`FakeServer::answering`], but every connection is held open
    /// after the answer, until the server is dropped: a daemon that keeps
    /// the connection alive whatever the request asked.
    fn answering_and_holding(response: String) -> Self {
        Self::serving(response, true)
    }

    fn serving(response: String, hold: bool) -> Self {
        use std::io::Read;
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let port = listener.local_addr().expect("port").port();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = std::sync::Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming() {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 16 * 1024 {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(&chunk[..n]),
                    }
                }
                let _ = stream.write_all(response.as_bytes());
                if hold {
                    held.push(stream);
                } else {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
            }
            drop(held);
        });
        Self {
            port,
            stop,
            thread: Some(thread),
        }
    }

    /// A daemon's `GET /v1/health` answer: `200` and the body the daemon
    /// writes, for `instance` speaking `protocol` (none: a daemon from before
    /// Q04, whose body has no `protocol`).
    fn health(instance: &str, protocol: Option<u32>, version: &str) -> Self {
        let mut body = json!({
            "instance_id": instance,
            "pid": 1,
            "status": "ok",
            "version": version,
        });
        if let Some(protocol) = protocol {
            body["protocol"] = json!(protocol);
        }
        Self::answering(http_response(
            "200 OK",
            "application/json",
            &body.to_string(),
        ))
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One whole HTTP/1.1 response, `Content-Length` set, the connection closed.
fn http_response(status: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A port nothing listens on: bound, then let go.
fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    listener.local_addr().expect("port").port()
}

/// C-12's check that nothing spawned: the spawn arm opens and rotates the
/// daemon log before it spawns, so the log is not there.
fn assert_nothing_spawned(guard: &HomeStoreGuard) {
    let log = paths::daemon_log_path().expect("log path");
    assert!(log.starts_with(guard.store()));
    assert!(!log.exists(), "the spawn arm ran: {}", log.display());
}

#[test]
fn ensure_daemon_running_reuses_a_daemon_whose_health_answers() {
    let guard = HomeStoreGuard::set("ensure-live");
    assert_no_daemon_beside_the_test_binary();
    let server = FakeServer::health("instance-q11", Some(1), SHELL_VERSION);
    let d = write_discovery(server.port, false);

    let answer = invoke("ensure_daemon_running", json!({})).expect("reused");
    assert_connection_info(&answer, &d);
    drop(server);
    assert_nothing_spawned(&guard);
}

/// A reused daemon's `version` and `protocol` are what its health answered,
/// not its file's: here a daemon of another version (the seventh row's note)
/// whose file says otherwise, and one from before Q04 whose health names no
/// protocol (the file's then stands).
#[test]
fn ensure_daemon_running_answers_the_version_and_protocol_health_gave() {
    let guard = HomeStoreGuard::set("ensure-health-fields");
    assert_no_daemon_beside_the_test_binary();
    let server = FakeServer::health(
        "instance-q11",
        Some(openalpaca_app_support::protocol::PROTOCOL),
        "0.0.1-other",
    );
    write_discovery(server.port, false);
    let answer = invoke("ensure_daemon_running", json!({})).expect("reused");
    drop(server);
    assert_eq!(answer["version"], "0.0.1-other");
    assert_eq!(
        answer["protocol"],
        openalpaca_app_support::protocol::PROTOCOL
    );
    let note = answer["note"].as_str().expect("a note on another version");
    assert!(
        note.starts_with(&format!(
            "This daemon is version 0.0.1-other and this app is {SHELL_VERSION}"
        )),
        "{note}"
    );

    let server = FakeServer::health("instance-q11", None, SHELL_VERSION);
    let mut d = write_discovery(server.port, false);
    d.build.version = "0.0.2-file".into();
    discovery::write_discovery_atomic(&d).expect("write discovery.json");
    let answer = invoke("ensure_daemon_running", json!({})).expect("reused");
    drop(server);
    assert_eq!(
        answer["version"], SHELL_VERSION,
        "health's version, not the file's"
    );
    assert_eq!(
        answer["protocol"], d.build.protocol,
        "the file's, health naming none"
    );
    assert_eq!(answer["note"], Value::Null);
    assert_nothing_spawned(&guard);
}

/// The rotated-token row (T21, Q04's table): a daemon up for more than a day
/// has rotated its file, and the file read now names the token it honours.
/// A lapsed file over a daemon whose health answers — hung before its
/// rotation, or from before T21 — is reused too: liveness is the
/// authority, as before Q04 (the no-change side, *Rev 95* (3)).
#[test]
fn ensure_daemon_running_trusts_a_live_daemon_over_an_expired_token() {
    let guard = HomeStoreGuard::set("ensure-expired");
    assert_no_daemon_beside_the_test_binary();
    let server = FakeServer::health("instance-q11", Some(1), SHELL_VERSION);
    let d = write_discovery(server.port, true);

    let answer = invoke("ensure_daemon_running", json!({})).expect("reused");
    assert_connection_info(&answer, &d);
    drop(server);
    assert_nothing_spawned(&guard);
}

/// A wrong server at the address an old discovery file names (its pid not a
/// live `openalpacad`: here the test's own): its `404` page is not a
/// daemon's health, so the file is stale and the shell goes to spawn — which
/// refuses at once, with no daemon beside the test binary, before it has
/// touched the store or the log.
#[test]
fn ensure_daemon_running_does_not_reuse_a_wrong_server_at_the_old_address() {
    let guard = HomeStoreGuard::set("ensure-wrong-server");
    assert_no_daemon_beside_the_test_binary();
    let server = FakeServer::answering(http_response(
        "404 Not Found",
        "text/html",
        "<html><body>Not Found</body></html>",
    ));
    write_discovery(server.port, false);

    let refused = refusal(invoke("ensure_daemon_running", json!({})));
    drop(server);
    assert_eq!(refused["kind"], "spawn_failed");
    let beside = std::env::current_exe()
        .expect("test binary")
        .canonicalize()
        .expect("canonical test binary")
        .parent()
        .expect("its folder")
        .join("openalpacad");
    assert_eq!(
        refused["message"],
        format!(
            "Failed to spawn daemon: Daemon binary not found at {}. The application may be \
             incorrectly installed.",
            beside.display()
        )
    );
    assert_nothing_spawned(&guard);
}

/// A stand-in for a daemon that is up but not serving yet: a copy of
/// `/bin/sleep` named `openalpacad`, so the process table calls it one. It
/// is killed when dropped.
#[cfg(unix)]
struct FakeDaemonProcess {
    _kill: KillOnDrop,
    pid: u32,
}

#[cfg(unix)]
impl FakeDaemonProcess {
    fn start(guard: &HomeStoreGuard) -> Self {
        let dir = guard.root().join("fake-daemon");
        std::fs::create_dir_all(&dir).expect("its folder");
        let program = dir.join("openalpacad");
        std::fs::copy("/bin/sleep", &program).expect("a copy of /bin/sleep");
        // macOS kills a bare copy of `/bin/sleep` at its start (SIGKILL,
        // seen on Darwin 27.2, 2026-11-16), often after the process table
        // has listed it, so a test racing that kill read it as a daemon or
        // not by chance; signed afresh, ad hoc, the copy runs its 61 s.
        #[cfg(target_os = "macos")]
        {
            let signed = Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-"])
                .arg(&program)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("codesign runs");
            assert!(signed.success(), "codesign refused the stand-in: {signed}");
        }
        let child = Command::new(&program)
            .arg("61")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the stand-in daemon");
        let pid = child.id();
        // Owned before anything can fail, so a failed wait below still kills
        // it on the way out.
        let kill = KillOnDrop(i32::try_from(pid).expect("a pid"));
        // Reaped from a thread, so a kill leaves no zombie the table reads.
        std::thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
        });
        let started = Instant::now();
        while !daemon_lifecycle::pid_is_daemon(pid) {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the stand-in never read as an openalpacad"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        Self { _kill: kill, pid }
    }
}

/// A discovery file naming `pid` at `port`.
fn write_discovery_for(pid: u32, port: u16) -> Discovery {
    let mut d = discovery::make_discovery("127.0.0.1", port, "instance-q11".into(), SHELL_VERSION);
    d.pid = pid;
    discovery::write_discovery_atomic(&d).expect("write discovery.json");
    d
}

/// The sixth case (C-09, A-18): the discovery pid is a live `openalpacad`
/// whose health does not answer — a daemon still booting, the CLI's or an
/// earlier run's. The shell answers `starting` at once and spawns nothing:
/// no second daemon, no log rotated.
#[cfg(unix)]
#[test]
fn ensure_daemon_running_parks_a_live_daemon_that_does_not_answer_yet() {
    let guard = HomeStoreGuard::set("ensure-starting");
    assert_no_daemon_beside_the_test_binary();
    let daemon = FakeDaemonProcess::start(&guard);
    write_discovery_for(daemon.pid, closed_port());

    let refused = refusal(invoke("ensure_daemon_running", json!({})));
    assert_eq!(refused["kind"], "starting", "{refused}");
    assert_nothing_spawned(&guard);
    drop(daemon);
}

// ============================================================================
// The sidecar's locator (Q04, D7 (a))
// ============================================================================

/// An app's folder under the scratch root: `<name>/App/openalpaca_gui`, with
/// the sidecar beside it when `with_sidecar`.
fn app_folder(guard: &HomeStoreGuard, name: &str, with_sidecar: bool) -> PathBuf {
    let dir = guard.root().join(name).join("App");
    std::fs::create_dir_all(&dir).expect("the app's folder");
    std::fs::write(dir.join("openalpaca_gui"), "the app").expect("the app");
    if with_sidecar {
        std::fs::write(dir.join(SIDECAR_NAME), "the daemon").expect("the sidecar");
    }
    dir
}

#[test]
fn the_locator_finds_the_sidecar_beside_the_app() {
    let guard = HomeStoreGuard::set("locator-beside");
    let dir = app_folder(&guard, "real", true);
    assert_eq!(
        sidecar_beside(&dir.join("openalpaca_gui")).expect("found"),
        dir.join(SIDECAR_NAME)
    );
}

/// D7 (a): a symlinked folder above the app is followed, not refused — the
/// path is canonicalized and the sidecar is the one in the real folder.
#[cfg(unix)]
#[test]
fn the_locator_follows_a_symlinked_ancestor_to_the_real_folder() {
    let guard = HomeStoreGuard::set("locator-ancestor");
    let dir = app_folder(&guard, "real", true);
    let link = guard.root().join("linked");
    std::os::unix::fs::symlink(guard.root().join("real"), &link).expect("a symlinked ancestor");
    assert_eq!(
        sidecar_beside(&link.join("App").join("openalpaca_gui")).expect("found"),
        dir.join(SIDECAR_NAME)
    );
}

/// The app itself reached through a symlink elsewhere: the sidecar is the
/// one beside the real app, not one beside the link.
#[cfg(unix)]
#[test]
fn the_locator_follows_a_symlinked_app_to_its_own_folder() {
    let guard = HomeStoreGuard::set("locator-link");
    let dir = app_folder(&guard, "real", true);
    let elsewhere = app_folder(&guard, "elsewhere", false);
    let link = elsewhere.join("openalpaca_gui-link");
    std::os::unix::fs::symlink(dir.join("openalpaca_gui"), &link).expect("a symlinked app");
    assert_eq!(
        sidecar_beside(&link).expect("found"),
        dir.join(SIDECAR_NAME)
    );
}

/// The one refusal, in every profile: no sidecar in the app's (canonical)
/// folder. Its sentence names the path it looked at, and no symlink case;
/// nothing on `PATH` is tried (the debug fallback is gone).
#[test]
fn the_locator_refuses_an_absent_sidecar_naming_the_path() {
    let guard = HomeStoreGuard::set("locator-absent");
    let dir = app_folder(&guard, "real", false);
    std::fs::write(guard.root().join("bin").join(SIDECAR_NAME), "on PATH").expect("on PATH");
    let refused = sidecar_beside(&dir.join("openalpaca_gui")).expect_err("refused");
    assert_eq!(
        format!("{refused:#}"),
        format!(
            "Daemon binary not found at {}. The application may be incorrectly installed.",
            dir.join(SIDECAR_NAME).display()
        )
    );
    assert!(!format!("{refused:#}").contains("symlink"));
}

/// The locator's refusal is told apart from every other failure to spawn,
/// so `gui.log` logs it as the sidecar missing (Q04-VERDICT); the window's
/// sentence is the one it was.
#[test]
fn an_absent_sidecar_is_a_spawn_refusal_of_its_own() {
    let guard = HomeStoreGuard::set("locator-refusal-kind");
    let dir = app_folder(&guard, "real", false);
    let refused = sidecar_beside(&dir.join("openalpaca_gui")).expect_err("refused");
    match reuse::spawn_refusal(&refused) {
        reuse::SpawnRefusal::SidecarMissing(error) => assert_eq!(
            serde_json::to_value(error).expect("as JSON"),
            json!({
                "kind": "spawn_failed",
                "message": format!(
                    "Failed to spawn daemon: Daemon binary not found at {}. The application \
                     may be incorrectly installed.",
                    dir.join(SIDECAR_NAME).display()
                ),
                "log_tail": null,
            })
        ),
        other => panic!("not the sidecar missing: {other:?}"),
    }
    let elsewhere = anyhow::anyhow!("Failed to open the daemon log: permission denied");
    assert!(matches!(
        reuse::spawn_refusal(&elsewhere),
        reuse::SpawnRefusal::Failed(_)
    ));
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
    paths::logs_dir().expect("logs dir");
    let log = paths::daemon_log_path().expect("log path");
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
    let log = paths::daemon_log_path().expect("log path");
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
    // `ensure_daemon_running`'s reuse arm only (C-12): a daemon whose
    // health answers (Q04).
    let server = FakeServer::health("instance-q11", Some(1), SHELL_VERSION);
    write_discovery(server.port, false);
    let (thread, answer) = invoke_answered_on("ensure_daemon_running", json!({}));
    answer.expect("reused");
    if thread == here {
        inline.push("ensure_daemon_running");
    }
    drop(server);
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
    let file = BlockedFile::at(paths::discovery_path().expect("discovery path"));
    let d = discovery::make_discovery("127.0.0.1", 1, "instance-q11".into(), SHELL_VERSION);
    let bytes = serde_json::to_vec(&d).expect("discovery as JSON");
    let (free, answer) =
        runtime_free_while_blocked(file, &bytes, || connection_info_for_window("main".into()));
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
    let file = BlockedFile::at(paths::daemon_log_path().expect("log path"));
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
    // The reuse arm only (C-12): the daemon the file names answers health.
    let server = FakeServer::health("instance-q11", Some(1), SHELL_VERSION);
    let port = server.port;
    let file = BlockedFile::at(paths::discovery_path().expect("discovery path"));
    let d = discovery::make_discovery("127.0.0.1", port, "instance-q11".into(), SHELL_VERSION);
    let bytes = serde_json::to_vec(&d).expect("discovery as JSON");
    let (free, answer) =
        runtime_free_while_blocked(file, &bytes, || ensure_for_window("main".into()));
    drop(server);
    assert!(
        free,
        "ensure_daemon_running held the runtime's thread while discovery.json would not open"
    );
    let info = answer.expect("reused");
    assert_connection_info(&serde_json::to_value(info).expect("as JSON"), &d);
    let log = paths::daemon_log_path().expect("log path");
    assert!(log.starts_with(guard.store()));
    assert!(!log.exists(), "the spawn arm ran: {}", log.display());
}

// ============================================================================
// The health probe (Q04's hand-written GET, Q12's tokio under its timeout)
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
fn the_health_probe_reads_a_daemon_that_answers() {
    let _guard = HomeStoreGuard::set("probe-live");
    let server = FakeServer::health("instance-q11", Some(1), SHELL_VERSION);
    assert_eq!(
        on_one_thread(probe_health("127.0.0.1", server.port)),
        Health::Answered {
            instance_id: "instance-q11".into(),
            protocol: Some(1),
            version: Some(SHELL_VERSION.into()),
        }
    );
}

/// A daemon that answers and keeps the connection open is read as soon as
/// its answer is whole, well inside the probe's timeout.
#[test]
fn the_health_probe_reads_a_whole_answer_on_a_connection_kept_open() {
    let _guard = HomeStoreGuard::set("probe-held");
    let body = json!({
        "instance_id": "instance-q11",
        "pid": 1,
        "protocol": 1,
        "status": "ok",
        "version": SHELL_VERSION,
    })
    .to_string();
    let server =
        FakeServer::answering_and_holding(http_response("200 OK", "application/json", &body));
    let started = std::time::Instant::now();
    assert!(matches!(
        on_one_thread(probe_health("127.0.0.1", server.port)),
        Health::Answered { .. }
    ));
    assert!(started.elapsed() < HEALTH_PROBE_TIMEOUT);
}

#[test]
fn the_health_probe_finds_nothing_at_a_closed_port() {
    let _guard = HomeStoreGuard::set("probe-closed");
    assert!(matches!(
        on_one_thread(probe_health("127.0.0.1", closed_port())),
        Health::Unanswered(_)
    ));
}

/// A port that accepts and never answers (a daemon bound and not yet
/// serving): the probe gives it its timeout and says no.
#[test]
fn the_health_probe_reads_a_bound_port_that_never_answers_as_no_answer() {
    let _guard = HomeStoreGuard::set("probe-bound");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let port = listener.local_addr().expect("port").port();
    let started = std::time::Instant::now();
    assert!(matches!(
        on_one_thread(probe_health("127.0.0.1", port)),
        Health::Unanswered(_)
    ));
    assert!(started.elapsed() >= HEALTH_PROBE_TIMEOUT);
    drop(listener);
}

/// An address whose attempt never completes costs the probe its timeout per
/// address and no more, and the probe says no.
#[test]
fn the_health_probe_gives_each_silent_address_its_timeout_and_no_more() {
    let _guard = HomeStoreGuard::set("probe-silent");
    assert_eq!(
        HEALTH_PROBE_TIMEOUT,
        Duration::from_millis(300),
        "the probe's wait per address"
    );
    let addrs: [std::net::SocketAddr; 2] = [
        "127.0.0.1:1".parse().expect("an address"),
        "[::1]:1".parse().expect("an address"),
    ];
    let started = std::time::Instant::now();
    let answered = on_one_thread(first_answer(addrs, |_| {
        std::future::pending::<std::io::Result<()>>()
    }));
    let waited = started.elapsed();
    assert!(answered.is_none());
    assert!(
        waited >= HEALTH_PROBE_TIMEOUT * 2,
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
        ShellError::Mismatch {
            message: "The daemon running (pid 51823, version 0.2.0, protocol 2) is not one this \
                      app can use: the app speaks protocol 1. Nothing was stopped or started. \
                      Update the app to one that speaks protocol 2, or stop that daemon with \
                      `openalpaca daemon stop` and press Reconnect in Settings → Connection to \
                      start this app's own."
                .into(),
            daemon_version: Some("0.2.0".into()),
            daemon_protocol: Some(2),
            shell_protocol: 1,
            remedy: "Update the app to one that speaks protocol 2, or stop that daemon with \
                     `openalpaca daemon stop` and press Reconnect in Settings → Connection to \
                     start this app's own."
                .into(),
        },
        ShellError::Starting {
            message: "The daemon (pid 51823) is starting: it has written its discovery file but \
                      does not answer yet. Waiting for it, up to 45 seconds."
                .into(),
        },
        ShellError::StillStarting {
            message: "The daemon (pid 51823) is still starting after 45 seconds: it is running \
                      but does not answer. Nothing was started, stopped or restarted. The last \
                      lines of its log (/s/state/logs/daemon.log):\n\nINFO Connecting to MCP \
                      server\n\nIf it does not finish starting, stop it with `openalpaca daemon \
                      stop` and press Reconnect in Settings → Connection to start a new one."
                .into(),
            log_tail: Some("INFO Connecting to MCP server".into()),
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
            | ShellError::Mismatch { .. }
            | ShellError::Starting { .. }
            | ShellError::StillStarting { .. }
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
    std::fs::read_to_string(paths::gui_log_path().expect("the gui.log path")).unwrap_or_default()
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
    let path = paths::gui_log_path().expect("the gui.log path");
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
    let path = paths::gui_log_path().expect("the gui.log path");
    std::fs::create_dir_all(path.parent().expect("logs dir")).expect("logs dir");
    std::fs::File::create(&path)
        .expect("a full log")
        .set_len(paths::DAEMON_LOG_MAX_BYTES + 1)
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
        paths::DAEMON_LOG_MAX_BYTES + 1
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

/// A stand-in for the window whose web content process ended (RELOAD-DIALOG):
/// it counts each reload and quit and keeps each dialog it was asked to show
/// open until the test answers it, so a test sees what the shell did and
/// answers when it chooses. The real window ([`DialogWebview`]) differs only
/// in its three effects: `Webview::reload`, `AppHandle::exit` and the dialog
/// plugin's message dialog, whose click is step 19's check at the app.
#[derive(Clone)]
struct FakeWindow {
    label: String,
    /// Whether each reload fails.
    failing: bool,
    /// Answered at once, inside `ask`, when set: a prompt that answers while
    /// `ask` is still running, on a thread `ask` waits for at most
    /// [`AT_ONCE_DEADLINE`], so a lock held across the dialog fails the test
    /// at once instead of hanging the run.
    answers_at_once: Option<(ReloadAnswer, Instant)>,
    seen: Arc<Mutex<FakeSeen>>,
}

#[derive(Default)]
struct FakeSeen {
    reloads: usize,
    quits: usize,
    asks: usize,
    open: Vec<Box<dyn FnOnce(ReloadAnswer, Instant) + Send>>,
}

impl FakeWindow {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            failing: false,
            answers_at_once: None,
            seen: Arc::default(),
        }
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, FakeSeen> {
        self.seen.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `(reloads, quits, asks, dialogs still open)`.
    fn counts(&self) -> (usize, usize, usize, usize) {
        let seen = self.seen();
        (seen.reloads, seen.quits, seen.asks, seen.open.len())
    }

    /// The person answers the oldest open dialog at `at`.
    fn answer(&self, answer: ReloadAnswer, at: Instant) {
        let on_answer = self.seen().open.remove(0);
        on_answer(answer, at);
    }
}

impl EndedWindow for FakeWindow {
    fn label(&self) -> &str {
        &self.label
    }

    fn reload(&self) -> tauri::Result<()> {
        self.seen().reloads += 1;
        if self.failing {
            Err(tauri::Error::WebviewNotFound)
        } else {
            Ok(())
        }
    }

    fn quit(&self) {
        self.seen().quits += 1;
    }

    fn ask(&self, on_answer: Box<dyn FnOnce(ReloadAnswer, Instant) + Send>) {
        self.seen().asks += 1;
        match self.answers_at_once {
            Some((answer, at)) => {
                let dispatch = tracing::dispatcher::get_default(Clone::clone);
                let (done, answered) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    tracing::dispatcher::with_default(&dispatch, || on_answer(answer, at));
                    let _ = done.send(());
                });
                assert!(
                    answered.recv_timeout(AT_ONCE_DEADLINE).is_ok(),
                    "the answer was left waiting: the bound's lock is held across the dialog"
                );
            }
            None => self.seen().open.push(on_answer),
        }
    }
}

/// How long [`FakeWindow::ask`] waits for an answer given at once.
const AT_ONCE_DEADLINE: Duration = Duration::from_secs(5);

/// The line for an end the bound reloads.
const ENDED: &str = "The window's web content process ended; reloading the window.";
/// The line for an end past the bound, which asks (RELOAD-BOUND's line,
/// its tail now the dialog's).
const ASKED: &str = "The window's web content process ended again after 3 reloads within \
                     60 s; the window is not reloaded by itself: a dialog asks whether to \
                     reload it or quit the app.";
/// The line for an end while the window's dialog is open.
const STILL_ASKING: &str = "The window's web content process ended again while its dialog \
                            is open; the window is not reloaded by itself, and no second \
                            dialog is shown.";
const CHOSE_RELOAD: &str = "Reload was chosen in the dialog; reloading the window.";
const CHOSE_QUIT: &str = "The dialog answered Quit (its button, or an end of the dialog \
                          that was not a click); quitting the app.";
const CHOSE_NEITHER: &str = "The dialog answered neither Reload nor Quit; the window is \
                             left as it is, and its next end asks again.";

/// How many lines of `gui.log` contain `needle`.
fn gui_log_count(needle: &str) -> usize {
    gui_log_text()
        .lines()
        .filter(|l| l.contains(needle))
        .count()
}

/// A web content process that ended is a line in `gui.log`, and the window
/// is reloaded, once; a reload that failed is a line too. The real window's
/// reload, `Webview::reload` over the mock runtime, answers `Ok`; the real
/// reload, and the reconnect to the same `instance_id`, are G-bundle leg A's.
#[test]
fn a_web_content_process_that_ended_is_logged_and_the_window_reloaded() {
    let _guard = HomeStoreGuard::set("gui-log-webcontent");
    let (_app, webview) = mock_webview();
    let window = FakeWindow::new("main");
    with_the_shell_log(|| {
        let real = DialogWebview(webview.as_ref().clone());
        web_content_ended(&real, &Arc::default(), Instant::now());
        web_content_ended(&window, &Arc::default(), Instant::now());
    });
    assert_eq!(
        window.counts(),
        (1, 0, 0, 0),
        "the window is reloaded, once"
    );

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
    let failing = FakeWindow {
        failing: true,
        ..FakeWindow::new("main")
    };
    with_the_shell_log(|| web_content_ended(&failing, &Arc::default(), Instant::now()));
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

/// RELOAD-BOUND (`tasks/pending-decisions.md` §1's *Rev 90* (b)): a window
/// whose web content process keeps ending is reloaded by itself at most
/// three times within 60 s, on an injected clock. A fourth end inside the
/// span is not reloaded by itself: one `ERROR` line in `gui.log`, in place of
/// the reloading line, and the dialog (RELOAD-DIALOG), answered here with
/// neither button, so the bound alone is seen. The span slides over the
/// reloads performed: an end refused never counts, a reload exactly 60 s
/// old no longer counts, so the next end after it is reloaded and the one
/// after that refused again until the next counted reload ages out. A
/// reload that failed counts; another window's ends never spend this one's.
/// The hook's registration with the builder and WebKit's own event are
/// G-bundle leg A's; the hook's one bound is a test of its own.
#[test]
fn a_fourth_reload_inside_60_s_is_refused_and_one_after_allowed() {
    let _guard = HomeStoreGuard::set("gui-log-reload-bound");
    let bound = Arc::default();
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let main = FakeWindow::new("main");
    // `window`'s process ends `ms` milliseconds after T0: whether it was
    // reloaded by itself. A dialog it asks is answered with neither button.
    let ended_at = |window: &FakeWindow, ms: u64| {
        let (before, _, asks, _) = window.counts();
        with_the_shell_log(|| {
            web_content_ended(window, &bound, at(ms));
            if window.counts().2 > asks {
                window.answer(ReloadAnswer::Neither, at(ms));
            }
        });
        window.counts().0 > before
    };

    for ms in [0, 1_000, 2_000] {
        assert!(ended_at(&main, ms), "a reload within the bound, at {ms} ms");
    }
    assert!(
        !ended_at(&main, 3_000),
        "a fourth end inside 60 s is not reloaded by itself"
    );
    assert_eq!(
        (gui_log_count(ENDED), gui_log_count(ASKED)),
        (3, 1),
        "{}",
        gui_log_text()
    );
    let text = gui_log_text();
    let asked = text
        .lines()
        .find(|l| l.contains(ASKED))
        .expect("a line for the refusal");
    assert!(asked.contains(" ERROR openalpaca_gui: "), "{asked}");
    assert!(asked.contains(" webview=\"main\""), "{asked}");

    // Still inside the span of the reload at 0 ms: refused, and not counted.
    assert!(!ended_at(&main, 59_999));
    // The reload at 0 ms is 60 s old: no longer counted.
    assert!(
        ended_at(&main, 60_000),
        "a reload after the span is allowed"
    );
    // 1 s, 2 s and 60 s count: refused again ...
    assert!(!ended_at(&main, 60_500));
    // ... until the reload at 1 s is 60 s old.
    assert!(ended_at(&main, 61_000));
    assert_eq!(main.counts(), (5, 0, 3, 0));
    assert_eq!(
        (gui_log_count(ENDED), gui_log_count(ASKED)),
        (5, 3),
        "one line for each end, never two: {}",
        gui_log_text()
    );

    // Another window's ends spend its own reloads, never this one's.
    assert!(ended_at(&FakeWindow::new("second"), 61_000));
    assert!(
        !ended_at(&main, 61_500),
        "the main window's bound is unchanged"
    );

    // A reload that failed counts as one given.
    let failing = FakeWindow {
        failing: true,
        ..FakeWindow::new("main")
    };
    let failing_bound = Arc::default();
    with_the_shell_log(|| {
        for ms in [0, 1_000, 2_000, 3_000] {
            web_content_ended(&failing, &failing_bound, at(ms));
        }
    });
    assert_eq!(
        failing.counts(),
        (3, 0, 1, 1),
        "three failed reloads spend the bound"
    );
}

/// RELOAD-DIALOG (`tasks/pending-decisions.md` §1's *Rev 100* (1) (c)): an
/// end past the bound asks, once, with one dialog per window at a time, and
/// each answer does what its button says, on the injected clock. `Reload`
/// reloads the window once, whatever the bound says, and that reload counts
/// as one given from the click, so the bound is never reset: the next end
/// inside the span asks again. `Quit` quits the app. A dialog closed with
/// neither leaves the window as it is. An end once the span has passed is
/// still reloaded by itself, with no dialog; nothing ever answers for the
/// person, and nothing waits on a timer.
#[test]
fn a_window_past_the_bound_asks_once_and_each_answer_does_what_it_says() {
    let _guard = HomeStoreGuard::set("gui-log-reload-dialog");
    let bound = Arc::default();
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let main = FakeWindow::new("main");

    with_the_shell_log(|| {
        for ms in [0, 1_000, 2_000] {
            web_content_ended(&main, &bound, at(ms));
        }
    });
    assert_eq!(main.counts(), (3, 0, 0, 0), "three reloads, no dialog");

    // The fourth end inside the span asks, and is not reloaded by itself.
    with_the_shell_log(|| web_content_ended(&main, &bound, at(3_000)));
    assert_eq!(main.counts(), (3, 0, 1, 1), "one dialog, open");
    let text = gui_log_text();
    let last = text.lines().last().expect("a line for the end");
    assert!(
        last.contains(ASKED),
        "the asking line is the last: {text:?}"
    );
    assert!(last.contains(" ERROR openalpaca_gui: "), "{last}");
    assert!(last.contains(" webview=\"main\""), "{last}");

    // An end while it is open: no reload, no second dialog, one line.
    with_the_shell_log(|| web_content_ended(&main, &bound, at(4_000)));
    assert_eq!(main.counts(), (3, 0, 1, 1), "still the one dialog");
    let text = gui_log_text();
    let last = text.lines().last().expect("a line for the end");
    assert!(last.contains(STILL_ASKING), "{text:?}");
    assert!(last.contains(" WARN openalpaca_gui: "), "{last}");
    assert!(last.contains(" webview=\"main\""), "{last}");

    // Reload: the window is reloaded once, and nothing quits.
    with_the_shell_log(|| main.answer(ReloadAnswer::Reload, at(5_000)));
    assert_eq!(main.counts(), (4, 0, 1, 0), "Reload reloads, once");
    assert_eq!(gui_log_count(CHOSE_RELOAD), 1, "{}", gui_log_text());

    // The bound is not reset: the next end inside the span asks again.
    with_the_shell_log(|| web_content_ended(&main, &bound, at(6_000)));
    assert_eq!(
        main.counts(),
        (4, 0, 2, 1),
        "a new dialog, the old answered"
    );

    // Quit: the app quits, and the window is not reloaded.
    with_the_shell_log(|| main.answer(ReloadAnswer::Quit, at(7_000)));
    assert_eq!(main.counts(), (4, 1, 2, 0), "Quit quits");
    assert_eq!(gui_log_count(CHOSE_QUIT), 1, "{}", gui_log_text());

    // The reload clicked at 5 s counts: at 60.5 s the reload at 0 s is out
    // of the span, and 1 s, 2 s and 5 s are three, so the end asks, where
    // without the click it would be reloaded by itself.
    with_the_shell_log(|| web_content_ended(&main, &bound, at(60_500)));
    assert_eq!(main.counts(), (4, 1, 3, 1), "the clicked reload counted");

    // A dialog answered with neither button: nothing reloads, nothing quits.
    with_the_shell_log(|| main.answer(ReloadAnswer::Neither, at(60_600)));
    assert_eq!(main.counts(), (4, 1, 3, 0), "neither leaves the window");
    assert_eq!(gui_log_count(CHOSE_NEITHER), 1, "{}", gui_log_text());

    // At 65 s the clicked reload is 60 s old: the end is reloaded by itself.
    with_the_shell_log(|| web_content_ended(&main, &bound, at(65_000)));
    assert_eq!(main.counts(), (5, 1, 3, 0), "a reload after the span");
    assert_eq!(
        (
            gui_log_count(ENDED),
            gui_log_count(ASKED),
            gui_log_count(STILL_ASKING)
        ),
        (4, 3, 1),
        "one line for each end: {}",
        gui_log_text()
    );

    // One window's open dialog never holds another's.
    let second = FakeWindow::new("second");
    with_the_shell_log(|| {
        web_content_ended(&main, &bound, at(65_100));
        web_content_ended(&main, &bound, at(65_200));
        web_content_ended(&main, &bound, at(65_300));
        for ms in [65_100, 65_200, 65_300, 65_400] {
            web_content_ended(&second, &bound, at(ms));
        }
    });
    assert_eq!(main.counts(), (7, 1, 4, 1), "main asks");
    assert_eq!(second.counts(), (3, 0, 1, 1), "second asks too");

    // A clicked reload that fails is said, and counted like any reload.
    with_the_shell_log(|| main.answer(ReloadAnswer::Reload, at(65_500)));
    let failing = FakeWindow {
        failing: true,
        ..FakeWindow::new("third")
    };
    with_the_shell_log(|| {
        for ms in [0, 1_000, 2_000, 3_000] {
            web_content_ended(&failing, &bound, at(ms));
        }
        failing.answer(ReloadAnswer::Reload, at(4_000));
    });
    assert_eq!(failing.counts(), (4, 0, 1, 0), "the click reloads");
    assert_eq!(
        gui_log_count("Could not reload the window"),
        4,
        "each failed reload is said: {}",
        gui_log_text()
    );

    // A prompt that answers while `ask` is still running: the bound's lock
    // is not held across the dialog, so the answer is not left waiting on
    // it (a lock held fails `FakeWindow::ask`'s deadline, not a hang).
    let at_once = FakeWindow {
        answers_at_once: Some((ReloadAnswer::Reload, at(3_500))),
        ..FakeWindow::new("fourth")
    };
    with_the_shell_log(|| {
        for ms in [0, 1_000, 2_000, 3_000] {
            web_content_ended(&at_once, &bound, at(ms));
        }
    });
    assert_eq!(at_once.counts(), (4, 0, 1, 0), "answered at once");

    // An end once the span has passed, while the dialog is still open: the
    // open dialog acts, so no reload by itself and no second dialog.
    let waiting = FakeWindow::new("fifth");
    with_the_shell_log(|| {
        for ms in [0, 1_000, 2_000, 3_000, 70_000] {
            web_content_ended(&waiting, &bound, at(ms));
        }
    });
    assert_eq!(waiting.counts(), (3, 0, 1, 1), "the open dialog holds");
    let text = gui_log_text();
    let last = text.lines().last().expect("a line for the end");
    assert!(last.contains(STILL_ASKING), "{text:?}");
    assert!(last.contains(" webview=\"fifth\""), "{last}");
}

/// The dialog's two buttons and how its result is read (RELOAD-DIALOG):
/// `Reload` first, the default button, then `Quit`. For these buttons
/// tauri-plugin-dialog 2.8.3 delivers only `Custom(label)`: a click is its
/// button's label, and an end that is not a click (rfd's `Cancel`, a failed
/// alert too) arrives as `Custom("Quit")`, so it reads as `Quit`
/// (`tasks/pending-decisions.md` §1's *Rev 109*). The other results, which
/// the plugin does not deliver for these buttons, read as neither.
#[test]
fn the_dialogs_buttons_read_as_reload_or_quit_by_their_labels() {
    use tauri_plugin_dialog::{MessageDialogButtons, MessageDialogResult};
    let _guard = HomeStoreGuard::set("gui-reload-dialog-buttons");
    match reload_dialog_buttons() {
        MessageDialogButtons::OkCancelCustom(first, second) => {
            assert_eq!((first.as_str(), second.as_str()), ("Reload", "Quit"));
        }
        other => panic!("two buttons, Reload then Quit: {other:?}"),
    }
    for (result, answer) in [
        (
            MessageDialogResult::Custom("Reload".to_owned()),
            ReloadAnswer::Reload,
        ),
        (
            MessageDialogResult::Custom("Quit".to_owned()),
            ReloadAnswer::Quit,
        ),
        (MessageDialogResult::Cancel, ReloadAnswer::Neither),
        (MessageDialogResult::Ok, ReloadAnswer::Neither),
        (MessageDialogResult::Yes, ReloadAnswer::Neither),
        (MessageDialogResult::No, ReloadAnswer::Neither),
        (
            MessageDialogResult::Custom("Close".to_owned()),
            ReloadAnswer::Neither,
        ),
    ] {
        assert_eq!(ReloadAnswer::from_dialog(&result), answer, "{result:?}");
    }
}

/// The hook `run` registers (RELOAD-BOUND, RELOAD-DIALOG) owns one bound for
/// every end it is called for: four ends of the window's process, one after
/// another on the real clock, well inside 60 s, are three reloading lines
/// and one asking; a fifth while that dialog is open shows none. Built here
/// over [`FakeWindow`]: the real hook's window is the same function over
/// [`DialogWebview`], whose dialog no test shows. Its registration with the
/// builder and WebKit's own event are G-bundle leg A's, which no mock fires.
#[test]
fn the_registered_reload_hook_keeps_one_bound_across_ends() {
    let _guard = HomeStoreGuard::set("gui-log-reload-hook");
    let (_app, webview) = mock_webview();
    let window = FakeWindow::new("main");
    let shared = window.clone();
    let hook =
        reload_hook_over(
            move |webview: &tauri::Webview<tauri::test::MockRuntime>| FakeWindow {
                label: webview.label().to_owned(),
                ..shared.clone()
            },
        );
    with_the_shell_log(|| {
        for _ in 0..5 {
            hook(webview.as_ref());
        }
    });
    assert_eq!(window.counts(), (3, 0, 1, 1), "one dialog across the ends");
    assert_eq!(
        (
            gui_log_count(ENDED),
            gui_log_count(ASKED),
            gui_log_count(STILL_ASKING)
        ),
        (3, 1, 1),
        "{}",
        gui_log_text()
    );
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
    capture_login_path, capture_login_shell, fallback_path,
};

/// An empty home adds no `.cargo/bin` to the fallback list: never the
/// relative entry the daemon's directory would resolve. `std::env::home_dir`
/// answers an empty home field of the user database as `Some("")` (its
/// documentation at Rust 1.99.1) where `directories` (dirs-sys 0.5.0)
/// answered `None` (M18).
#[test]
fn an_empty_home_adds_nothing_to_the_fallback_list() {
    assert_eq!(fallback_path(Some(Path::new(""))), fallback_path(None));
}

/// A relative home is no home either (121-1, the owner's word of
/// 2026-11-17): its `.cargo/bin` would be an entry the daemon's directory
/// resolves, so the fallback list ends without it.
#[test]
fn a_relative_home_adds_nothing_to_the_fallback_list() {
    assert_eq!(
        fallback_path(Some(Path::new("relative-home"))),
        fallback_path(None)
    );
    let list = fallback_path(Some(Path::new("/Users/someone")));
    assert!(
        list.to_string_lossy()
            .ends_with(":/Users/someone/.cargo/bin"),
        "an absolute home keeps its entry: {list:?}"
    );
}

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
const CAPTURED_STORE: &str = "/q21/other-store-captured-not-passed";

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
/// `AWS_PROFILE_BACKUP` (a name extending a listed one) and an
/// `OPENALPACA_HOME_STORE` naming another store beside its `PATH`; it
/// records each run's arguments.
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
             OPENALPACA_HOME_STORE='{CAPTURED_STORE}'; export OPENALPACA_HOME_STORE\n\
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
            OsString::from(paths::MANAGED_LOG_ENV),
            OsString::from(paths::PATH_SOURCE_ENV),
            OsString::from("PATH"),
        ],
        "the command sets these four and removes nothing"
    );
    assert_eq!(
        set[&OsString::from("PATH")],
        Some(OsString::from(CAPTURED_PATH))
    );
    assert_eq!(
        set[&OsString::from(paths::PATH_SOURCE_ENV)],
        Some(OsString::from(format!("login-shell {}", path.capture_ms())))
    );
    assert_eq!(cmd.get_current_dir(), Some(app_dir.as_path()));
    assert_no_captured_value_but(&cmd, &[]);
}

/// The values the noisy login shell exports beside its `PATH`.
const CAPTURED_VALUES: [&str; 6] = [
    CAPTURED_API_KEY,
    CAPTURED_PROFILE,
    CAPTURED_KUBECONFIG,
    CAPTURED_GOOGLE,
    CAPTURED_PROFILE_BACKUP,
    CAPTURED_STORE,
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
    let listener = FakeServer::health("instance-q11", Some(1), SHELL_VERSION);
    let d = write_discovery(listener.port, false);
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
    let listener = FakeServer::health("instance-q11", Some(1), SHELL_VERSION);
    let d = write_discovery(listener.port, false);
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
            "login_shell_noninteractive",
            DaemonPathReport {
                source: PathSource::LoginShellNoninteractive,
                capture_ms: 1840,
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
            OsString::from(paths::MANAGED_LOG_ENV),
            OsString::from(paths::PATH_SOURCE_ENV),
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
            OsString::from(paths::MANAGED_LOG_ENV),
            OsString::from(paths::PATH_SOURCE_ENV),
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

// ============================================================================
// The interactive profile, the process group and `OPENALPACA_` names
// (LOGIN-CAPTURE)
// ============================================================================

/// The folders the scratch profiles below add to `PATH`, each in one file.
const ZSHRC_DIR: &str = "/login-capture/zshrc/bin";
const ZPROFILE_DIR: &str = "/login-capture/zprofile/bin";
const BASHRC_DIR: &str = "/login-capture/bashrc/bin";

/// A real login shell under the guard's scratch `HOME`: `<root>/real/<name>`,
/// a `/bin/sh` script that records its arguments in `<root>/real/<name>.runs`
/// and runs `target` with them, `ZDOTDIR`, `TERM_PROGRAM`, `ENV` and
/// `BASH_ENV` removed, so no file of the developer's own is read: zsh reads
/// its startup files from `HOME`, which the guard points at its scratch
/// `home`. The capture picks `-i` by this script's name.
#[cfg(unix)]
fn real_shell(root: &Path, name: &str, target: &str) -> PathBuf {
    let dir = root.join("real");
    std::fs::create_dir_all(&dir).expect("the real shells' folder");
    let runs = dir.join(format!("{name}.runs"));
    fake_login_shell(
        &dir,
        name,
        &format!(
            "printf '%s\\n' \"$@\" >> '{runs}'\n\
             exec /usr/bin/env -u ZDOTDIR -u TERM_PROGRAM -u ENV -u BASH_ENV {target} \"$@\"",
            runs = runs.display()
        ),
    )
}

/// The arguments each run of [`real_shell`]'s `name` was given.
fn real_shell_runs(root: &Path, name: &str) -> String {
    std::fs::read_to_string(root.join("real").join(format!("{name}.runs"))).unwrap_or_default()
}

/// The entries of a `PATH` value.
fn path_entries(value: &OsString) -> Vec<String> {
    value
        .to_string_lossy()
        .split(':')
        .map(str::to_owned)
        .collect()
}

/// Whether `pid` is gone within `within`.
#[cfg(unix)]
fn gone_within(pid: i32, within: Duration) -> bool {
    let by = Instant::now() + within;
    while process_exists(pid) && Instant::now() < by {
        std::thread::sleep(Duration::from_millis(20));
    }
    !process_exists(pid)
}

/// The pid a profile wrote to `file`, when it did.
fn pid_in(file: &Path) -> Option<i32> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|text| text.trim().parse().ok())
}

/// *Rev 91* (1): zsh is run as an interactive login shell, `-i -l -c`, as
/// ZCode does, so a folder only `~/.zshrc` adds to `PATH` — where this
/// project's installer and `nvm` write theirs — is in the daemon's `PATH`.
/// The real `/bin/zsh`, under the guard's scratch `HOME`.
#[cfg(target_os = "macos")]
#[test]
fn an_interactive_capture_finds_a_path_entry_only_zshrc_adds() {
    let guard = HomeStoreGuard::set("interactive-zshrc");
    let home = guard.root().join("home");
    std::fs::write(
        home.join(".zshrc"),
        format!("export PATH=\"{ZSHRC_DIR}:$PATH\"\n"),
    )
    .expect(".zshrc");
    let zsh = real_shell(guard.root(), "zsh", "/bin/zsh");
    let path = capture_of(Some(zsh), Some(home), Duration::from_secs(10)).wait();

    assert_eq!(path.failure, None);
    assert_eq!(path.source, PathSource::LoginShell);
    assert!(
        path_entries(path.value()).iter().any(|e| e == ZSHRC_DIR),
        "the folder ~/.zshrc adds is in the daemon's PATH"
    );
    assert_eq!(
        real_shell_runs(guard.root(), "zsh"),
        format!("-i\n-l\n-c\n{CAPTURE_COMMAND}\n"),
        "run once, as an interactive login command"
    );
}

/// *Rev 91* (1) for bash, as the facts on the day have it: bash is run as
/// `-i -l -c` too, but a login bash, interactive or not, never reads
/// `~/.bashrc` itself — only a `~/.bash_profile` that sources it does, and
/// then `-i` takes it past the usual "not interactive: return" guard at its
/// top. The real `/bin/bash`, under the guard's scratch `HOME`.
#[cfg(target_os = "macos")]
#[test]
fn bash_is_run_interactively_and_reads_bashrc_only_through_its_profile() {
    let guard = HomeStoreGuard::set("interactive-bashrc");
    let home = guard.root().join("home");
    std::fs::write(
        home.join(".bashrc"),
        format!("case $- in *i*) ;; *) return ;; esac\nexport PATH=\"{BASHRC_DIR}:$PATH\"\n"),
    )
    .expect(".bashrc");
    let bash = real_shell(guard.root(), "bash", "/bin/bash");
    let deadline = Duration::from_secs(10);

    let alone = capture_of(Some(bash.clone()), Some(home.clone()), deadline).wait();
    assert_eq!(alone.failure, None);
    assert!(
        !path_entries(alone.value()).iter().any(|e| e == BASHRC_DIR),
        "a login bash does not read ~/.bashrc by itself"
    );

    std::fs::write(home.join(".bash_profile"), ". \"$HOME/.bashrc\"\n").expect(".bash_profile");
    let sourced = capture_of(Some(bash), Some(home), deadline).wait();
    assert_eq!(sourced.failure, None);
    assert!(
        path_entries(sourced.value())
            .iter()
            .any(|e| e == BASHRC_DIR),
        "sourced from ~/.bash_profile, ~/.bashrc passes its interactive guard"
    );
    assert_eq!(
        real_shell_runs(guard.root(), "bash"),
        format!("-i\n-l\n-c\n{CAPTURE_COMMAND}\n").repeat(2)
    );
}

/// The one regression *Rev 91* (1)'s pick accepts: an interactive profile
/// that hangs — `~/.zshrc` waiting on a child — takes the capture to its
/// deadline and the daemon to the fallback list, killed with what it
/// started, and `gui.log` says why, where the login profile alone
/// (`~/.zprofile`, the same zsh run as `-l -c`) would have answered. A hang
/// leaves no time for a second run (CAPTURE-RETRY's runs only in the time
/// the first left): the fallback recorded is the deadline's fixed list.
#[cfg(target_os = "macos")]
#[test]
fn a_hanging_interactive_profile_gets_the_fallback_list() {
    let guard = HomeStoreGuard::set("interactive-hangs");
    let home = guard.root().join("home");
    let pid_file = guard.root().join("zshrc-child.pid");
    std::fs::write(
        home.join(".zprofile"),
        format!("export PATH=\"{ZPROFILE_DIR}:$PATH\"\n"),
    )
    .expect(".zprofile");
    std::fs::write(
        home.join(".zshrc"),
        format!(
            "/bin/sleep 47 &\necho $! > '{}'\nwait\n",
            pid_file.display()
        ),
    )
    .expect(".zshrc");

    // The login profile alone answers: the same zsh, by a name that is
    // neither zsh nor bash, is run as `-l -c` and never reads ~/.zshrc.
    let plain = real_shell(guard.root(), "login-zsh", "/bin/zsh");
    let alone = capture_login_path(&plain, Instant::now(), Duration::from_secs(10), &[])
        .expect("the login profile alone answers");
    assert!(path_entries(&alone.path).iter().any(|e| e == ZPROFILE_DIR));
    assert!(!pid_file.exists(), "-l -c did not read ~/.zshrc");

    // As the capture runs zsh: the deadline, then the fallback list.
    let zsh = real_shell(guard.root(), "zsh", "/bin/zsh");
    let deadline = Duration::from_millis(1500);
    let path = with_the_shell_log(|| capture_of(Some(zsh), Some(home.clone()), deadline).wait());
    let child = pid_in(&pid_file);
    let _stray = child.map(KillOnDrop);

    assert_eq!(path.source, PathSource::FallbackList);
    assert_eq!(path.failure, Some(CaptureFailure::TimedOut(deadline)));
    assert_eq!(path.value(), &fallback_path(Some(&home)));
    assert_eq!(
        real_shell_runs(guard.root(), "zsh"),
        format!("-i\n-l\n-c\n{CAPTURE_COMMAND}\n"),
        "run once, interactively, and not again"
    );
    let child = child.expect("the interactive profile started its child");
    assert!(
        gone_within(child, Duration::from_secs(5)),
        "the hanging profile's child outlived the deadline's kill"
    );
    let text = gui_log_text();
    assert!(
        text.lines().any(|l| l.contains(" WARN ")
            && l.contains("The daemon's PATH is the fallback list.")
            && l.contains("reason=the login shell did not finish within 1500 ms")),
        "gui.log says why: {text:?}"
    );
}

/// *Rev 99* (2), the advisor's pick (CAPTURE-RETRY): an interactive profile
/// that ends the shell before it prints the markers — `~/.zshrc` exiting
/// with a failure, exiting with 0, or replacing the shell (`exec /bin/sh`,
/// which reads its empty input and ends) — is read again, once, as `-l -c`
/// in the time left of the one deadline, so the daemon gets the login
/// profile's `PATH` (`~/.zprofile`'s folder, not the one `~/.zshrc` added
/// before it ended), its source named `login-shell-noninteractive` in
/// `gui.log`, on the daemon's command and on the connection; `gui.log` says
/// why the second run was made. The real `/bin/zsh`, under the guard's
/// scratch `HOME`.
#[cfg(target_os = "macos")]
#[test]
fn a_profile_that_leaves_the_interactive_shell_is_read_again_as_a_login_shell() {
    let guard = HomeStoreGuard::set("interactive-ends");
    let home = guard.root().join("home");
    std::fs::write(
        home.join(".zprofile"),
        format!("export PATH=\"{ZPROFILE_DIR}:$PATH\"\n"),
    )
    .expect(".zprofile");
    let zsh = real_shell(guard.root(), "zsh", "/bin/zsh");
    let runs_file = guard.root().join("real").join("zsh.runs");
    let cases = [
        ("exits 7", "exit 7\n"),
        ("exits 0", "exit 0\n"),
        ("execs", "exec /bin/sh\n"),
    ];
    let captured = with_the_shell_log(|| {
        cases
            .iter()
            .map(|(case, ending)| {
                std::fs::write(
                    home.join(".zshrc"),
                    format!("export PATH=\"{ZSHRC_DIR}:$PATH\"\n{ending}"),
                )
                .expect(".zshrc");
                let _ = std::fs::remove_file(&runs_file);
                let path = capture_of(
                    Some(zsh.clone()),
                    Some(home.clone()),
                    Duration::from_secs(10),
                )
                .wait();
                (*case, path, real_shell_runs(guard.root(), "zsh"))
            })
            .collect::<Vec<_>>()
    });

    for (case, path, runs) in &captured {
        assert_eq!(path.failure, None, "{case}");
        assert_eq!(path.source.as_str(), "login-shell-noninteractive", "{case}");
        let entries = path_entries(path.value());
        assert!(
            entries.iter().any(|e| e == ZPROFILE_DIR),
            "{case}: ~/.zprofile's folder is in the daemon's PATH"
        );
        assert!(
            !entries.iter().any(|e| e == ZSHRC_DIR),
            "{case}: the PATH is the second run's, which never read ~/.zshrc"
        );
        assert_eq!(
            runs,
            &format!("-i\n-l\n-c\n{CAPTURE_COMMAND}\n-l\n-c\n{CAPTURE_COMMAND}\n"),
            "{case}: run interactively, then once more as a login command"
        );
        assert_eq!(
            path.announcement(),
            format!("login-shell-noninteractive {}", path.capture_ms()),
            "{case}"
        );
        assert_eq!(
            serde_json::to_value(path.report()).expect("serializes")["source"],
            json!("login-shell-noninteractive"),
            "{case}"
        );
    }
    let text = gui_log_text();
    assert_eq!(
        text.lines()
            .filter(|l| l.contains(" INFO ")
                && l.contains("it is run once more as a login shell without -i"))
            .count(),
        3,
        "gui.log says why each second run was made: {text:?}"
    );
    for reason in [
        "reason=the login shell failed (exit status: 7)",
        "reason=the login shell did not print its environment",
    ] {
        assert!(
            text.lines()
                .any(|l| l.contains("run once more") && l.contains(reason)),
            "{reason}: {text:?}"
        );
    }
    assert_eq!(
        text.lines()
            .filter(|l| l.contains(" INFO ") && l.contains("source=\"login-shell-noninteractive\""))
            .count(),
        3,
        "gui.log names the second run's source: {text:?}"
    );
    assert_eq!(
        text.lines()
            .filter(|l| l.contains("source=\"login-shell-noninteractive\"")
                && l.contains("read as a login shell without -i"))
            .count(),
        3,
        "gui.log's capture line says the interactive files were not read: {text:?}"
    );
    assert!(!text.contains(ZPROFILE_DIR), "gui.log never holds the PATH");
}

/// *Rev 99* (2) for bash, the real `/bin/bash` under the guard's scratch
/// `HOME`: a `~/.bashrc` that `~/.bash_profile` sources and that exits when
/// interactive ends the `-i -l -c` run before its markers; the second run,
/// `-l -c`, still reads `~/.bash_profile` and the `~/.bashrc` it sources, up
/// to that interactive check, and answers with the profile's folder.
#[cfg(target_os = "macos")]
#[test]
fn a_bashrc_that_ends_the_interactive_shell_is_read_again_as_a_login_shell() {
    let guard = HomeStoreGuard::set("interactive-bash-ends");
    let home = guard.root().join("home");
    std::fs::write(
        home.join(".bash_profile"),
        format!("export PATH=\"{ZPROFILE_DIR}:$PATH\"\n. \"$HOME/.bashrc\"\n"),
    )
    .expect(".bash_profile");
    std::fs::write(
        home.join(".bashrc"),
        format!("case $- in *i*) exit 7 ;; esac\nexport PATH=\"{BASHRC_DIR}:$PATH\"\n"),
    )
    .expect(".bashrc");
    let bash = real_shell(guard.root(), "bash", "/bin/bash");
    let path = capture_of(Some(bash), Some(home), Duration::from_secs(10)).wait();

    assert_eq!(path.failure, None);
    assert_eq!(path.source.as_str(), "login-shell-noninteractive");
    let entries = path_entries(path.value());
    assert!(
        entries.iter().any(|e| e == ZPROFILE_DIR),
        "~/.bash_profile's"
    );
    assert!(
        entries.iter().any(|e| e == BASHRC_DIR),
        "~/.bashrc, sourced, read past its interactive check"
    );
    assert_eq!(
        real_shell_runs(guard.root(), "bash"),
        format!("-i\n-l\n-c\n{CAPTURE_COMMAND}\n-l\n-c\n{CAPTURE_COMMAND}\n")
    );
}

/// *Rev 99* (2): the second run has only the time the first left of the one
/// deadline. A first run that ends late with a failure and a second that
/// waits end together at that deadline — the fallback list, the second
/// shell killed with its group — never a second deadline of its own. A
/// profile that waits takes the whole deadline and leaves no time for a
/// second run at all (`a_hanging_interactive_profile_gets_the_fallback_list`).
/// A stand-in shell named `zsh`, so the capture runs it with `-i`; the
/// capture's own function is timed, not the wait (which ends at the deadline
/// by itself).
#[cfg(unix)]
#[test]
fn the_second_run_has_only_the_time_the_first_left() {
    let guard = HomeStoreGuard::set("retry-time-left");
    let dir = guard.root().join("late");
    std::fs::create_dir_all(&dir).expect("the late shell's folder");
    let runs = dir.join("runs");
    let pid_file = guard.root().join("second.pid");
    let zsh = fake_login_shell(
        &dir,
        "zsh",
        &format!(
            "printf '%s\\n' \"$@\" >> '{runs}'\n\
             case \"$1\" in\n\
             -i) /bin/sleep 1; exit 4 ;;\n\
             *) echo $$ > '{pid}'; exec /bin/sleep 47 ;;\n\
             esac",
            runs = runs.display(),
            pid = pid_file.display()
        ),
    );
    let deadline = Duration::from_millis(1500);
    let started = Instant::now();
    let outcome = capture_login_shell(&zsh, started, deadline, &[]);
    let took = started.elapsed();
    let second = pid_in(&pid_file);
    let _stray = second.map(KillOnDrop);

    assert_eq!(
        std::fs::read_to_string(&runs).expect("the shell ran"),
        format!("-i\n-l\n-c\n{CAPTURE_COMMAND}\n-l\n-c\n{CAPTURE_COMMAND}\n"),
        "the first run's failure made a second, as a login command"
    );
    assert_eq!(outcome, Err(CaptureFailure::TimedOut(deadline)));
    assert!(
        took >= deadline && took < deadline + Duration::from_millis(500),
        "the one deadline, not a second: {took:?}"
    );
    let second = second.expect("the second run started");
    assert!(
        gone_within(second, Duration::from_secs(2)),
        "the second run outlived the deadline's kill"
    );
}

/// *Rev 99* (2) with *Rev 99* (4) (a) unchanged: an interactive run that
/// puts a child in the background, holding the shell's output, and exits
/// with a failure before its markers is run again — the capture waits at
/// most its reader's grace for the start marker, not the whole deadline — and
/// the second run answers; the first run's group is left as a failed exit
/// leaves it, so its child is still running when the capture has answered.
#[cfg(unix)]
#[test]
fn a_failed_first_runs_child_is_left_and_the_second_run_answers() {
    let guard = HomeStoreGuard::set("retry-child-left");
    let dir = guard.root().join("child");
    std::fs::create_dir_all(&dir).expect("the shell's folder");
    let pid_file = guard.root().join("first-child.pid");
    let zsh = fake_login_shell(
        &dir,
        "zsh",
        &format!(
            "case \"$1\" in\n\
             -i) /bin/sleep 47 & echo $! > '{pid}'; exit 3 ;;\n\
             *) PATH='{CAPTURED_PATH}'; export PATH\n{LAST_ARG} ;;\n\
             esac",
            pid = pid_file.display()
        ),
    );
    let started = Instant::now();
    let outcome = capture_login_shell(&zsh, started, Duration::from_secs(10), &[]);
    let took = started.elapsed();
    let child = pid_in(&pid_file).expect("the first run started its child");
    let _stray = KillOnDrop(child);

    let captured = outcome.expect("the second run answers");
    assert_eq!(captured.path, OsString::from(CAPTURED_PATH));
    assert!(captured.second_run);
    assert!(
        took < Duration::from_secs(2),
        "the held pipe cost the reader's grace, not the deadline: {took:?}"
    );
    assert!(
        !gone_within(child, Duration::from_millis(300)),
        "the first run's group was killed after its failed exit"
    );
}

/// *Rev 99* (2): a first run that ends before its markers with no time left
/// is not run again. The interactive run puts a child in the background,
/// holding the shell's output, and exits with a failure; under a deadline
/// shorter than the reader's grace (250 ms) the wait for its start marker
/// runs into the deadline, and the capture answers the first run's failure —
/// not a second run's, which would answer at once, nor the deadline's. A
/// stand-in shell named `zsh`, so the capture runs it with `-i`.
#[cfg(unix)]
#[test]
fn a_first_run_that_ends_with_no_time_left_is_not_run_again() {
    let guard = HomeStoreGuard::set("retry-no-time-left");
    let dir = guard.root().join("no-time");
    std::fs::create_dir_all(&dir).expect("the shell's folder");
    let runs = dir.join("runs");
    let pid_file = guard.root().join("held.pid");
    let zsh = fake_login_shell(
        &dir,
        "zsh",
        &format!(
            "printf '%s\\n' \"$@\" >> '{runs}'\n\
             case \"$1\" in\n\
             -i) /bin/sleep 47 & echo $! > '{pid}'; exit 3 ;;\n\
             *) PATH='{CAPTURED_PATH}'; export PATH\n{LAST_ARG} ;;\n\
             esac",
            runs = runs.display(),
            pid = pid_file.display()
        ),
    );
    // Under the reader's grace, so the wait after the failed exit always
    // runs into the deadline, whenever the exit came.
    let deadline = Duration::from_millis(240);
    let outcome = capture_login_shell(&zsh, Instant::now(), deadline, &[]);
    let _stray = pid_in(&pid_file).map(KillOnDrop);

    assert_eq!(
        outcome,
        Err(CaptureFailure::Exited("exit status: 3".into())),
        "the first run's failure, with no time left for a second"
    );
    assert_eq!(
        std::fs::read_to_string(&runs).expect("the shell ran"),
        format!("-i\n-l\n-c\n{CAPTURE_COMMAND}\n"),
        "run once"
    );
}

/// Runs a stand-in shell's last argument, the capture's command, wherever
/// `-i` put it.
const LAST_ARG: &str = "for command; do :; done\neval \"$command\"";

/// *Rev 99* (2)'s words, "before it prints the markers": an interactive run
/// that printed its environment and then failed, or printed one with no
/// `PATH`, is not run again — the profile did not end the shell first — and
/// a shell run as `-l -c` from the start (neither zsh nor bash) is never run
/// again, whether it fails or exits 0 without its markers.
#[cfg(unix)]
#[test]
fn only_an_interactive_run_ended_before_its_markers_is_run_again() {
    let guard = HomeStoreGuard::set("retry-only-before-markers");
    let home = guard.root().join("home");
    let deadline = Duration::from_secs(10);
    let list = fallback_path(Some(&home));
    for (folder, name, body, failure, args) in [
        (
            "after",
            "bash",
            format!("PATH='{CAPTURED_PATH}'; export PATH\n{LAST_ARG}\nexit 3"),
            CaptureFailure::Exited("exit status: 3".into()),
            "-i\n-l\n-c\n",
        ),
        (
            "no-path",
            "zsh",
            format!("unset PATH\n{LAST_ARG}"),
            CaptureFailure::NoPath,
            "-i\n-l\n-c\n",
        ),
        (
            "plain",
            "dash",
            "exit 5".to_owned(),
            CaptureFailure::Exited("exit status: 5".into()),
            "-l\n-c\n",
        ),
        (
            "plain0",
            "dash",
            "exit 0".to_owned(),
            CaptureFailure::NoMarkers,
            "-l\n-c\n",
        ),
    ] {
        let dir = guard.root().join(folder);
        std::fs::create_dir_all(&dir).expect("the shell's folder");
        let runs = dir.join("runs");
        let shell = fake_login_shell(
            &dir,
            name,
            &format!(
                "printf '%s\\n' \"$@\" >> '{runs}'\n{body}",
                runs = runs.display()
            ),
        );
        let path = capture_of(Some(shell), Some(home.clone()), deadline).wait();

        assert_eq!(path.source, PathSource::FallbackList, "{folder}");
        assert_eq!(path.failure, Some(failure), "{folder}");
        assert_eq!(path.value(), &list, "{folder}");
        assert_eq!(
            std::fs::read_to_string(&runs).expect("the shell ran"),
            format!("{args}{CAPTURE_COMMAND}\n"),
            "{folder}: run once"
        );
    }
}

/// A stand-in login shell that puts `/bin/sleep 47` in the background,
/// writes its pid to `pid_file`, exports [`CAPTURED_PATH`] and runs the
/// capture's command.
#[cfg(unix)]
fn backgrounding_login_shell(root: &Path, pid_file: &Path) -> PathBuf {
    fake_login_shell(
        root,
        "backgrounding-shell",
        &format!(
            "/bin/sleep 47 &\n\
             echo $! > '{pid}'\n\
             PATH='{CAPTURED_PATH}'; export PATH\n\
             eval \"$3\"",
            pid = pid_file.display()
        ),
    )
}

/// *Rev 91* (3): after a successful capture the login shell's process group
/// is killed too, so a child a profile put in the background
/// (`some-agent &` in `~/.zprofile`) is gone once the capture has answered.
#[cfg(unix)]
#[test]
fn a_profiles_background_child_is_gone_after_a_successful_capture() {
    let guard = HomeStoreGuard::set("background-killed");
    let pid_file = guard.root().join("background.pid");
    let shell = backgrounding_login_shell(guard.root(), &pid_file);
    let outcome = capture_login_path(&shell, Instant::now(), Duration::from_secs(10), &[]);
    let child = pid_in(&pid_file).expect("the profile started its child");
    let _stray = KillOnDrop(child);

    assert_eq!(
        outcome.map(|captured| captured.path),
        Ok(OsString::from(CAPTURED_PATH))
    );
    assert!(
        gone_within(child, Duration::from_secs(2)),
        "the profile's background child outlived the capture"
    );
}

/// *Rev 91* (3) for zsh run interactively: its `&` children stay in the
/// capture's process group (no terminal, so no job control to give them
/// groups of their own), and are gone after a successful capture too.
#[cfg(target_os = "macos")]
#[test]
fn an_interactive_zshs_background_child_is_gone_after_a_successful_capture() {
    let guard = HomeStoreGuard::set("background-killed-zsh");
    let home = guard.root().join("home");
    let pid_file = guard.root().join("zprofile-child.pid");
    std::fs::write(
        home.join(".zprofile"),
        format!("/bin/sleep 47 &\necho $! > '{}'\n", pid_file.display()),
    )
    .expect(".zprofile");
    let zsh = real_shell(guard.root(), "zsh", "/bin/zsh");
    let path = capture_of(Some(zsh), Some(home), Duration::from_secs(10)).wait();
    let child = pid_in(&pid_file).expect("the profile started its child");
    let _stray = KillOnDrop(child);

    assert_eq!(path.failure, None);
    assert!(
        gone_within(child, Duration::from_secs(2)),
        "the interactive profile's background child outlived the capture"
    );
}

/// *Rev 99*'s fourth item as built, option (a): after a non-zero exit the
/// capture reaps the shell and leaves its process group as it was, so a
/// child a failing profile put in the background is still running when
/// the capture has answered. Option (b), the group killed after a failed
/// exit too, an enforcement point, is the owner's (built first in
/// LOGIN-CAPTURE and taken out on its review).
#[cfg(unix)]
#[test]
fn a_failed_captures_process_group_is_left_as_it_was() {
    let guard = HomeStoreGuard::set("failed-group-left");
    let pid_file = guard.root().join("background.pid");
    let shell = fake_login_shell(
        guard.root(),
        "failing-backgrounding-shell",
        &format!(
            "/bin/sleep 47 >/dev/null 2>&1 &\n\
             echo $! > '{pid}'\n\
             PATH='{CAPTURED_PATH}'; export PATH\n\
             eval \"$3\"\n\
             exit 3",
            pid = pid_file.display()
        ),
    );
    let outcome = capture_login_path(&shell, Instant::now(), Duration::from_secs(10), &[]);
    let child = pid_in(&pid_file).expect("the profile started its child");
    let _stray = KillOnDrop(child);

    assert_eq!(
        outcome.map(|captured| captured.path),
        Err(CaptureFailure::Exited("exit status: 3".into()))
    );
    assert!(
        !gone_within(child, Duration::from_millis(500)),
        "the group was killed after a failed exit"
    );
}

/// *Rev 92* (1): every `OPENALPACA_` name the list holds is dropped with a
/// `gui.log` warning naming it, as the app's own four are — a listed
/// `OPENALPACA_HOME_STORE` would point the daemon at a store the app does
/// not read `discovery.json` from — and none reaches the daemon's command,
/// though the login shell exports it. An entry that is not a variable name
/// is still named only by its place, `OPENALPACA_` or not.
#[cfg(unix)]
#[test]
fn a_listed_openalpaca_name_is_dropped_with_a_warning_naming_it() {
    let guard = HomeStoreGuard::set("keys-openalpaca");
    write_daemon_toml(
        &guard,
        "[daemon.env]\npassthrough_keys = [\"OPENALPACA_HOME_STORE\", \"AWS_PROFILE\", \
         \"OPENALPACA_Q21_ANY\", \"OPENALPACA_X=q21-value-in-the-list\"]\n",
    );
    let shell = noisy_login_shell(guard.root());
    let (keys, path) = with_the_shell_log(|| {
        let keys = configured_passthrough_keys();
        let path = capture_listing(Some(shell), None, keys.clone(), Duration::from_secs(10)).wait();
        (keys, path)
    });

    assert_eq!(keys, ["AWS_PROFILE"]);
    assert_eq!(path.source, PathSource::LoginShell);
    assert_eq!(path.passed_keys(), ["AWS_PROFILE"]);
    let cmd = command_for(&guard, &path);
    assert_eq!(
        set_on(&cmd).keys().cloned().collect::<Vec<_>>(),
        [
            OsString::from("AWS_PROFILE"),
            OsString::from("OPENALPACA_CONFIG_DIR"),
            OsString::from(paths::MANAGED_LOG_ENV),
            OsString::from(paths::PATH_SOURCE_ENV),
            OsString::from("PATH"),
        ]
    );
    assert_no_captured_value_but(&cmd, &[CAPTURED_PROFILE]);
    let text = gui_log_text();
    for name in ["OPENALPACA_HOME_STORE", "OPENALPACA_Q21_ANY"] {
        assert!(
            text.lines()
                .any(|l| l.contains(" WARN ") && l.contains(&format!("key=\"{name}\""))),
            "{name} is named as not passed: {text:?}"
        );
    }
    assert!(text.contains("entry 4 is not a variable name"), "{text:?}");
    assert!(!text.contains("q21-value-in-the-list"), "{text:?}");
}

/// *Rev 91* (3), the output's side: once the group is killed nothing holds
/// the shell's standard output, and the capture waits for its reader of it
/// to reach the end, so the reader has ended by the time the capture
/// answers — no thread is left behind per launch. The reader is held 100 ms
/// past the pipe's end (`READER_END_DELAY_MS`, under the 250 ms grace), so
/// a capture that answered once the end marker was in, without that wait,
/// would leave it running here every time, not only when the kill's end
/// of the pipe came late (*Rev 99*'s built-beyond item).
#[cfg(unix)]
#[test]
fn the_output_reader_has_ended_when_the_capture_answers() {
    use std::sync::atomic::Ordering;
    /// Puts the reader's delay back to 0 however the test ends.
    struct NoDelayOnDrop;
    impl Drop for NoDelayOnDrop {
        fn drop(&mut self) {
            login_path::READER_END_DELAY_MS.store(0, Ordering::SeqCst);
        }
    }
    let guard = HomeStoreGuard::set("reader-ended");
    // A reader an earlier test's capture left is given time to end first.
    let by = Instant::now() + Duration::from_secs(5);
    while login_path::OUTPUT_READERS.load(Ordering::SeqCst) != 0 && Instant::now() < by {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        login_path::OUTPUT_READERS.load(Ordering::SeqCst),
        0,
        "a reader from before this test is still running"
    );
    let _no_delay = NoDelayOnDrop;
    login_path::READER_END_DELAY_MS.store(100, Ordering::SeqCst);
    let pid_file = guard.root().join("background.pid");
    let shell = backgrounding_login_shell(guard.root(), &pid_file);
    let outcome = capture_login_path(&shell, Instant::now(), Duration::from_secs(10), &[]);
    let _stray = pid_in(&pid_file).map(KillOnDrop);

    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(
        login_path::OUTPUT_READERS.load(Ordering::SeqCst),
        0,
        "the output's reader had ended when the capture answered"
    );
}

/// Built beyond the words (`tasks/pending-decisions.md` §1 *Rev 99*): the
/// capture's shell leads a session of its own, not only a process group —
/// so an interactive shell has no terminal to take, whatever started the
/// app — and its pid is the session's and the group's id while it runs.
#[cfg(unix)]
#[test]
fn the_capture_runs_the_shell_in_a_session_of_its_own() {
    let guard = HomeStoreGuard::set("session-leader");
    let pid_file = guard.root().join("shell.pid");
    let shell = fake_login_shell(
        guard.root(),
        "waiting-shell",
        &format!("echo $$ > '{}'\nexec /bin/sleep 47", pid_file.display()),
    );
    let (sent, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let outcome = capture_login_path(&shell, Instant::now(), Duration::from_secs(3), &[]);
        let _ = sent.send(outcome);
    });
    let by = Instant::now() + Duration::from_secs(2);
    while pid_in(&pid_file).is_none() && Instant::now() < by {
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid = pid_in(&pid_file).expect("the shell wrote its pid");
    let _stray = KillOnDrop(pid);
    // SAFETY: plain queries of a live child's ids.
    let (session, group) = unsafe { (libc::getsid(pid), libc::getpgid(pid)) };

    assert_eq!(session, pid, "the shell leads a session of its own");
    assert_eq!(group, pid, "and the session's one process group");
    let outcome = received
        .recv_timeout(Duration::from_secs(6))
        .expect("the capture ends at its deadline");
    assert_eq!(
        outcome,
        Err(CaptureFailure::TimedOut(Duration::from_secs(3)))
    );
}

/// The variables an app started from a Terminal.app tab hands the capture:
/// set on this process for the test's span, under the guard's lock (this
/// binary's only writer of the environment), and put back on drop, before
/// the guard's own.
#[cfg(target_os = "macos")]
struct TerminalTab {
    prev: Vec<(&'static str, Option<OsString>)>,
}

#[cfg(target_os = "macos")]
impl TerminalTab {
    const VARS: [(&'static str, &'static str); 2] = [
        ("TERM_PROGRAM", "Apple_Terminal"),
        ("TERM_SESSION_ID", "login-capture-tab"),
    ];

    /// Only with a [`HomeStoreGuard`] held, whose lock it rides on.
    fn open(_guard: &HomeStoreGuard) -> Self {
        let prev = Self::VARS
            .iter()
            .map(|(name, _)| (*name, std::env::var_os(name)))
            .collect();
        for (name, value) in Self::VARS {
            // SAFETY: serialized by ENV_LOCK, which the guard holds.
            unsafe { std::env::set_var(name, value) };
        }
        Self { prev }
    }
}

#[cfg(target_os = "macos")]
impl Drop for TerminalTab {
    fn drop(&mut self) {
        for (name, value) in self.prev.drain(..) {
            // SAFETY: as above — the guard, and so ENV_LOCK, outlives this.
            match value {
                Some(v) => unsafe { std::env::set_var(name, v) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
    }
}

/// Every file under `dir`, by its path below it, with its bytes.
#[cfg(target_os = "macos")]
fn files_under(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(at) = pending.pop() {
        for entry in std::fs::read_dir(&at).expect("read a scratch folder") {
            let path = entry.expect("a scratch entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let bytes = std::fs::read(&path).expect("read a scratch file");
                found.push((path.strip_prefix(dir).expect("below").to_path_buf(), bytes));
            }
        }
    }
    found.sort();
    found
}

/// Built beyond the words (`tasks/pending-decisions.md` §1 *Rev 99*): an
/// app started from a Terminal.app tab hands the capture the tab's
/// `TERM_PROGRAM` and `TERM_SESSION_ID`; run as `-i`, zsh and bash would
/// then run Apple's session scripts, whose exit hook rewrites the tab's
/// session and history files and the history file in the person's home.
/// The capture drops the session id for its shell, so the real `/bin/zsh`
/// and `/bin/bash`, under the guard's scratch `HOME`, leave every file there
/// byte for byte as it was — and still answer.
#[cfg(target_os = "macos")]
#[test]
fn a_terminal_started_apps_capture_leaves_the_tabs_session_files_alone() {
    let guard = HomeStoreGuard::set("terminal-tab");
    let home = guard.root().join("home");
    let id = TerminalTab::VARS[1].1;
    for (history, history_line, sessions) in [
        (
            ".zsh_history",
            ": 1700000000:0;git status\n",
            ".zsh_sessions",
        ),
        (".bash_history", "git status\n", ".bash_sessions"),
    ] {
        std::fs::write(home.join(history), history_line).expect("a history file");
        let sessions = home.join(sessions);
        std::fs::create_dir_all(&sessions).expect("a sessions folder");
        std::fs::write(
            sessions.join(format!("{id}.session")),
            "echo the-tabs-own\n",
        )
        .expect("the tab's session file");
        std::fs::write(sessions.join(format!("{id}.historynew")), "tab-cmd-1\n")
            .expect("the tab's unsaved history");
    }
    let before = files_under(&home);

    let dir = guard.root().join("terminal");
    std::fs::create_dir_all(&dir).expect("the terminal shells' folder");
    let _tab = TerminalTab::open(&guard);
    for (name, target) in [("zsh", "/bin/zsh"), ("bash", "/bin/bash")] {
        // Unlike [`real_shell`], `TERM_PROGRAM` is kept: what this test
        // reads is what the capture itself passes on.
        let shell = fake_login_shell(
            &dir,
            name,
            &format!("exec /usr/bin/env -u ZDOTDIR -u ENV -u BASH_ENV {target} \"$@\""),
        );
        let path = capture_of(Some(shell), Some(home.clone()), Duration::from_secs(10)).wait();
        assert_eq!(path.failure, None, "{name} answered");
        assert_eq!(path.source, PathSource::LoginShell, "{name} answered");
    }

    assert_eq!(
        files_under(&home),
        before,
        "the capture left the scratch home's history and session files as they were"
    );
}
