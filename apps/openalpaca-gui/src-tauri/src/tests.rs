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

/// One `invoke(cmd, args)` from a webview of a mock app built from
/// [`with_commands`]: the command's answer as JSON, or its refusal.
fn invoke(cmd: &str, args: Value) -> Result<Value, Value> {
    let app = with_commands(mock_builder())
        .build(mock_context(noop_assets()))
        .expect("mock app");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("mock webview");
    get_ipc_response(
        &webview,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().expect("url"),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
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
