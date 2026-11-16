//! `decide_reuse`'s table (Q04; plan-rev3 §7.2 G3-b, DU-29 (a), C-09, A-18,
//! A-20) and the start it drives, over a world the test writes: no file, no
//! process table, no socket and no clock of the machine's. Nothing here
//! spawns, reads the store or opens a port, so no test holds the home-store
//! guard.

use super::*;
use openalpaca_storage::discovery::{self, Discovery};
use std::collections::VecDeque;

// ============================================================================
// The table
// ============================================================================

/// A shell that speaks protocol 3 and reads back to 2, so the rows that need
/// a protocol older than this shell's but still supported, and one too old,
/// have values to run on (the build's own constants are both 1 today).
const SHELL: ShellBuild = ShellBuild {
    protocol: 3,
    oldest_supported: 2,
    version: "0.4.0",
};

const INSTANCE: &str = "0f1e2d3c-4b5a-4968-8776-655443322110";

/// A daemon's discovery file: pid 4242, version `version`, protocol
/// `protocol`.
fn discovery_of(version: &str, protocol: u32) -> Discovery {
    let mut d = discovery::make_discovery("127.0.0.1", 51823, INSTANCE.into(), version);
    d.pid = 4242;
    d.build.protocol = protocol;
    d
}

/// Health's answer for `instance`, as a daemon writes it.
fn answered(instance: &str, protocol: Option<u32>, version: &str) -> Health {
    Health::Answered {
        instance_id: instance.into(),
        protocol,
        version: Some(version.into()),
    }
}

fn silent() -> Health {
    Health::Unanswered("connection refused".into())
}

fn look(discovery: DiscoveryRead, health: Health, pid_live: bool) -> Look {
    Look {
        discovery,
        health,
        pid_live,
    }
}

fn parsed(d: &Discovery) -> DiscoveryRead {
    DiscoveryRead::Parsed(d.clone())
}

/// What the table's verdict is, without the discovery file a reuse carries.
#[derive(Debug, PartialEq)]
enum Row {
    Spawn,
    Reuse(Option<String>),
    Mismatch {
        daemon_version: Option<String>,
        daemon_protocol: Option<u32>,
    },
    Starting(u32),
}

fn row(verdict: Verdict) -> Row {
    match verdict {
        Verdict::Spawn => Row::Spawn,
        Verdict::Reuse(reused) => Row::Reuse(reused.note),
        Verdict::Mismatch(m) => Row::Mismatch {
            daemon_version: m.daemon_version,
            daemon_protocol: m.daemon_protocol,
        },
        Verdict::Starting { pid, .. } => Row::Starting(pid),
    }
}

fn decide(look: &Look) -> Row {
    row(decide_reuse(look, &SHELL))
}

#[test]
fn no_discovery_file_spawns() {
    assert_eq!(
        decide(&look(DiscoveryRead::Missing, silent(), false)),
        Row::Spawn
    );
}

#[test]
fn a_compatible_daemon_is_reused_with_no_note() {
    let d = discovery_of("0.4.0", 3);
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered(INSTANCE, Some(3), "0.4.0"),
            true
        )),
        Row::Reuse(None)
    );
}

/// The reused daemon is the file's, with the protocol and version its
/// health answered.
#[test]
fn a_reuse_carries_the_file_and_what_health_answered() {
    let d = discovery_of("0.4.0", 3);
    let Verdict::Reuse(reused) = decide_reuse(
        &look(parsed(&d), answered(INSTANCE, Some(3), "0.4.0"), true),
        &SHELL,
    ) else {
        panic!("not a reuse");
    };
    assert_eq!(reused.discovery.instance_id, INSTANCE);
    assert_eq!(reused.discovery.auth.token, d.auth.token);
    assert_eq!(reused.protocol, 3);
    assert_eq!(reused.version, "0.4.0");
}

/// An older protocol this shell still reads: reused, with a note naming
/// both protocols and the way to this app's own daemon.
#[test]
fn an_older_supported_daemon_is_reused_with_a_note() {
    let d = discovery_of("0.3.0", 2);
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered(INSTANCE, Some(2), "0.3.0"),
            true
        )),
        Row::Reuse(Some(
            "This daemon speaks protocol 2, older than this app's 3; the app works \
             with it. To run this app's own daemon (version 0.4.0), use Stop daemon…, \
             then Start daemon."
                .into()
        ))
    );
}

#[test]
fn a_newer_daemon_is_a_mismatch() {
    let d = discovery_of("0.5.0", 4);
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered(INSTANCE, Some(4), "0.5.0"),
            true
        )),
        Row::Mismatch {
            daemon_version: Some("0.5.0".into()),
            daemon_protocol: Some(4),
        }
    );
}

#[test]
fn a_daemon_older_than_the_oldest_supported_is_a_mismatch() {
    let d = discovery_of("0.1.0", 1);
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered(INSTANCE, Some(1), "0.1.0"),
            true
        )),
        Row::Mismatch {
            daemon_version: Some("0.1.0".into()),
            daemon_protocol: Some(1),
        }
    );
}

/// A health body with no `protocol` is a daemon from before Q04: the
/// protocol its discovery file states is taken (every daemon has written
/// one), so it is judged as any other — never a spawn over a live daemon.
#[test]
fn a_health_answer_without_protocol_is_judged_by_the_files_protocol() {
    for (file_protocol, expected) in [
        (3, Row::Reuse(None)),
        (
            4,
            Row::Mismatch {
                daemon_version: Some("0.4.0".into()),
                daemon_protocol: Some(4),
            },
        ),
    ] {
        let d = discovery_of("0.4.0", file_protocol);
        assert_eq!(
            decide(&look(parsed(&d), answered(INSTANCE, None, "0.4.0"), true)),
            expected,
            "file protocol {file_protocol}"
        );
    }
}

/// What health answered is the daemon's word now; the file's `build` is
/// what it wrote at its start.
#[test]
fn health_s_protocol_wins_over_the_file_s() {
    let d = discovery_of("0.4.0", 3);
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered(INSTANCE, Some(4), "0.4.0"),
            true
        )),
        Row::Mismatch {
            daemon_version: Some("0.4.0".into()),
            daemon_protocol: Some(4),
        }
    );
}

/// A file with no `build.protocol` (or any other field this build needs)
/// does not parse as this build's `Discovery`: over a live daemon's pid it
/// is a mismatch, never a spawn or a kill (*Rev 95* (4)).
#[test]
fn a_file_missing_its_protocol_over_a_live_daemon_is_a_mismatch() {
    let read = read_discovery_bytes(
        br#"{"schema":2,"pid":4242,"instance_id":"x","build":{"version":"0.9.0"}}"#,
    );
    assert_eq!(
        decide(&look(read, silent(), true)),
        Row::Mismatch {
            daemon_version: Some("0.9.0".into()),
            daemon_protocol: None,
        }
    );
}

/// The same file over a pid that is not a live daemon is no daemon.
#[test]
fn a_file_this_build_cannot_read_over_no_live_daemon_spawns() {
    let read = read_discovery_bytes(br#"{"schema":2,"pid":4242}"#);
    assert_eq!(decide(&look(read, silent(), false)), Row::Spawn);
}

/// A malformed file — not JSON, or no numeric `pid` — names nothing, so
/// there is nothing to reuse and nothing to protect: spawn (DU-29's first
/// step).
#[test]
fn a_malformed_file_spawns() {
    for bytes in [
        &b"not discovery"[..],
        &br#"{"pid":"4242"}"#[..],
        &br#"[4242]"#[..],
    ] {
        let read = read_discovery_bytes(bytes);
        assert!(
            matches!(read, DiscoveryRead::Unreadable),
            "{:?}",
            String::from_utf8_lossy(bytes)
        );
        assert_eq!(decide(&look(read, silent(), true)), Row::Spawn);
    }
}

/// A file this build writes parses as itself.
#[test]
fn this_build_s_file_parses() {
    let d = discovery_of("0.4.0", 3);
    let read = read_discovery_bytes(&serde_json::to_vec_pretty(&d).unwrap());
    let DiscoveryRead::Parsed(back) = read else {
        panic!("not parsed");
    };
    assert_eq!(back.instance_id, d.instance_id);
}

/// A stale file (a crash, a reboot): its pid is not a live daemon and its
/// port answers nothing.
#[test]
fn a_stale_pid_with_a_dead_port_spawns() {
    let d = discovery_of("0.4.0", 3);
    assert_eq!(decide(&look(parsed(&d), silent(), false)), Row::Spawn);
}

/// A wrong server at the old address (its 404 page is no health answer),
/// and the pid not a live daemon: the file is stale.
#[test]
fn a_wrong_server_at_the_old_address_spawns() {
    let d = discovery_of("0.4.0", 3);
    let health = parse_health_response(
        b"HTTP/1.1 404 Not Found\r\ncontent-type: text/html\r\ncontent-length: 9\r\n\r\nNot Found",
    );
    assert!(matches!(health, Health::Unanswered(_)), "{health:?}");
    assert_eq!(decide(&look(parsed(&d), health, false)), Row::Spawn);
}

/// Another instance answers at the address: the listener is not the
/// file's daemon. With the file's pid gone, spawn.
#[test]
fn an_instance_change_between_the_file_and_health_spawns_when_the_pid_is_gone() {
    let d = discovery_of("0.4.0", 3);
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered("another-instance", Some(3), "0.4.0"),
            false
        )),
        Row::Spawn
    );
}

/// A live pid beats every spawn verdict: whatever the address says — silent,
/// a 404, another instance — while the file's pid is a live `openalpacad`,
/// a spawn would only lose the singleton lock to it (and rotate its log
/// under it), so the shell parks in `starting`.
#[test]
fn a_live_pid_parks_in_starting_whatever_the_address_says() {
    let d = discovery_of("0.4.0", 3);
    for health in [
        silent(),
        Health::Unanswered("HTTP 404".into()),
        answered("another-instance", Some(3), "0.4.0"),
    ] {
        assert_eq!(decide(&look(parsed(&d), health, true)), Row::Starting(4242));
    }
}

/// The rotated-token row (T21): what is judged is the daemon, not the
/// token's age, so a file past its first day — rotated or lapsed — over a
/// daemon whose health answers is reused.
#[test]
fn a_live_daemon_s_rotated_or_lapsed_token_is_reused() {
    let mut d = discovery_of("0.4.0", 3);
    let rotated = d.with_rotated_auth(d.started_at + discovery::ROTATE_AFTER * 3);
    assert_eq!(
        decide(&look(
            parsed(&rotated),
            answered(INSTANCE, Some(3), "0.4.0"),
            true
        )),
        Row::Reuse(None)
    );
    d.auth.expires_at = d.started_at - discovery::TOKEN_LIFETIME;
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered(INSTANCE, Some(3), "0.4.0"),
            true
        )),
        Row::Reuse(None)
    );
}

/// The seventh row (A-20): the same protocol, another version — reused,
/// with a note naming both versions and Stop then Start, never a restart.
#[test]
fn the_same_protocol_at_another_version_is_reused_with_a_note() {
    let d = discovery_of("0.3.9", 3);
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered(INSTANCE, Some(3), "0.3.9"),
            true
        )),
        Row::Reuse(Some(
            "This daemon is version 0.3.9 and this app is 0.4.0; they speak the same \
             protocol, so the app uses it. To run this app's own version, use Stop \
             daemon…, then Start daemon."
                .into()
        ))
    );
}

// ============================================================================
// The mismatch's sentence
// ============================================================================

#[test]
fn a_newer_daemon_s_refusal_names_both_protocols_and_the_remedy() {
    let d = discovery_of("0.5.0", 4);
    let Verdict::Mismatch(m) = decide_reuse(
        &look(parsed(&d), answered(INSTANCE, Some(4), "0.5.0"), true),
        &SHELL,
    ) else {
        panic!("not a mismatch");
    };
    let error = serde_json::to_value(m.into_error()).unwrap();
    assert_eq!(
        error,
        serde_json::json!({
            "kind": "mismatch",
            "message": "The daemon running (pid 4242, version 0.5.0, protocol 4) is not one \
                        this app can use: the app speaks protocol 3, and reads back to \
                        protocol 2. Nothing was stopped or started. Update the app to one that \
                        speaks protocol 4, or stop that daemon with `openalpaca daemon stop` \
                        and press Reconnect in Settings → Connection to start this app's own.",
            "daemon_version": "0.5.0",
            "daemon_protocol": 4,
            "shell_protocol": 3,
            "remedy": "Update the app to one that speaks protocol 4, or stop that daemon with \
                       `openalpaca daemon stop` and press Reconnect in Settings → Connection \
                       to start this app's own.",
        })
    );
}

#[test]
fn an_older_daemon_s_refusal_names_the_stop_and_reconnect() {
    let d = discovery_of("0.1.0", 1);
    let Verdict::Mismatch(m) = decide_reuse(
        &look(parsed(&d), answered(INSTANCE, Some(1), "0.1.0"), true),
        &SHELL,
    ) else {
        panic!("not a mismatch");
    };
    let error = serde_json::to_value(m.into_error()).unwrap();
    assert_eq!(
        error["remedy"],
        "Stop that daemon with `openalpaca daemon stop` and press Reconnect in Settings → \
         Connection to start this app's own."
    );
    assert_eq!(
        error["message"],
        "The daemon running (pid 4242, version 0.1.0, protocol 1) is not one this app can \
         use: the app speaks protocol 3, and reads back to protocol 2. Nothing was stopped or \
         started. Stop that daemon with `openalpaca daemon stop` and press Reconnect in \
         Settings → Connection to start this app's own."
    );
}

#[test]
fn an_unreadable_file_s_refusal_says_another_build_wrote_it() {
    let read = read_discovery_bytes(br#"{"pid":4242,"build":{"version":"0.9.0"}}"#);
    let Verdict::Mismatch(m) = decide_reuse(&look(read, silent(), true), &SHELL) else {
        panic!("not a mismatch");
    };
    let error = serde_json::to_value(m.into_error()).unwrap();
    assert_eq!(
        error["message"],
        "The daemon running (pid 4242, version 0.9.0) wrote a discovery file this app \
         cannot read: another build of OpenAlpaca. Nothing was stopped or started. Update \
         the app, or stop that daemon with `openalpaca daemon stop` and press Reconnect in \
         Settings → Connection to start this app's own."
    );
    assert_eq!(error["daemon_protocol"], serde_json::Value::Null);
    assert_eq!(error["shell_protocol"], 3);
}

/// With one protocol supported, the sentence names one.
#[test]
fn the_build_s_own_range_is_one_protocol_today() {
    assert_eq!(ShellBuild::THIS.protocol, ShellBuild::THIS.oldest_supported);
    let mut d = discovery_of("0.1.0", ShellBuild::THIS.protocol + 1);
    d.build.version = "9.9.9".into();
    let Verdict::Mismatch(m) = decide_reuse(
        &look(
            parsed(&d),
            answered(INSTANCE, Some(ShellBuild::THIS.protocol + 1), "9.9.9"),
            true,
        ),
        &ShellBuild::THIS,
    ) else {
        panic!("not a mismatch");
    };
    let message = m.into_error().to_string();
    assert!(
        message.contains(&format!(
            "the app speaks protocol {}. Nothing",
            ShellBuild::THIS.protocol
        )),
        "{message}"
    );
}

// ============================================================================
// Health's answer, read from the bytes
// ============================================================================

fn http(status: &str, headers: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\n{headers}\r\n{body}").into_bytes()
}

#[test]
fn a_daemon_s_health_body_is_its_answer() {
    let body = r#"{"instance_id":"abc","pid":7,"protocol":1,"status":"ok","version":"0.1.0"}"#;
    let health = parse_health_response(&http(
        "200 OK",
        &format!(
            "content-type: application/json\r\ncontent-length: {}\r\n",
            body.len()
        ),
        body,
    ));
    assert_eq!(
        health,
        Health::Answered {
            instance_id: "abc".into(),
            protocol: Some(1),
            version: Some("0.1.0".into()),
        }
    );
}

#[test]
fn a_chunked_health_body_is_read_whole() {
    let health = parse_health_response(&http(
        "200 OK",
        "transfer-encoding: chunked\r\n",
        "f\r\n{\"status\":\"ok\",\r\n14\r\n\"instance_id\":\"abc\"}\r\n0\r\n\r\n",
    ));
    assert_eq!(
        health,
        Health::Answered {
            instance_id: "abc".into(),
            protocol: None,
            version: None,
        }
    );
}

#[test]
fn what_is_not_a_daemon_s_health_is_no_answer() {
    let daemon_body = r#"{"instance_id":"abc","protocol":1,"status":"ok"}"#;
    for bytes in [
        http(
            "404 Not Found",
            "content-type: text/html\r\n",
            "<html></html>",
        ),
        // Another status is no answer, whatever the body says.
        http("404 Not Found", "", daemon_body),
        http("500 Internal Server Error", "", daemon_body),
        http("200 OK", "", "<html></html>"),
        http("200 OK", "", r#"{"status":"down","instance_id":"abc"}"#),
        http("200 OK", "", r#"{"status":"ok"}"#),
        http(
            "200 OK",
            "",
            r#"{"status":"ok","instance_id":"abc","protocol":-1}"#,
        ),
        b"SSH-2.0-OpenSSH_9.9\r\n".to_vec(),
        Vec::new(),
    ] {
        let health = parse_health_response(&bytes);
        assert!(
            matches!(health, Health::Unanswered(_)),
            "{:?} read as {health:?}",
            String::from_utf8_lossy(&bytes)
        );
    }
}

/// The read stops once the answer is whole, not when the daemon closes: a
/// `Content-Length` met, or a chunked body's last chunk; a head with neither
/// waits for the close.
#[test]
fn an_answer_is_whole_at_its_length_or_its_last_chunk() {
    assert!(!answer_is_whole(
        b"HTTP/1.1 200 OK\r\ncontent-length: 4\r\n"
    ));
    assert!(!answer_is_whole(
        b"HTTP/1.1 200 OK\r\ncontent-length: 4\r\n\r\n{}"
    ));
    assert!(answer_is_whole(
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"
    ));
    assert!(!answer_is_whole(
        b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n2\r\n{}\r\n"
    ));
    assert!(answer_is_whole(
        b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n"
    ));
    assert!(!answer_is_whole(b"HTTP/1.1 200 OK\r\n\r\n{}"));
}

/// A body longer than its `Content-Length` is read to the length only.
#[test]
fn a_health_body_is_read_to_its_length() {
    let body = r#"{"status":"ok","instance_id":"abc"}"#;
    let health = parse_health_response(
        format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}trailing bytes",
            body.len()
        )
        .as_bytes(),
    );
    assert!(matches!(health, Health::Answered { .. }), "{health:?}");
}

// ============================================================================
// The start, over a world the test writes
// ============================================================================

/// A world whose looks are a script over time and spawns: `script(at,
/// spawns)` is what a look sees `at` after the world's start, once the shell
/// has spawned `spawns` daemons. Its clock moves only when the shell sleeps
/// (or a test passes time between invokes), and a sleep past
/// [`FakeWorld::CEILING`] fails the test: a clock that stopped counting
/// would otherwise spin the wait after a spawn for ever.
struct FakeWorld {
    start: Instant,
    at: Duration,
    spawns: usize,
    /// The pid a spawn reports: the daemon this ask started.
    spawn_pid: u32,
    looks: usize,
    script: Box<dyn FnMut(Duration, usize) -> Look + Send>,
    log_reads: VecDeque<Option<u64>>,
    /// Why the log cannot be read, when it cannot.
    log_unreadable: Option<String>,
}

impl FakeWorld {
    fn new(script: impl FnMut(Duration, usize) -> Look + Send + 'static) -> Self {
        Self {
            start: Instant::now(),
            at: Duration::ZERO,
            spawns: 0,
            spawn_pid: 9090,
            looks: 0,
            script: Box::new(script),
            log_reads: VecDeque::new(),
            log_unreadable: None,
        }
    }

    /// Twice the budget: no ask, however its wait goes, sleeps that long.
    const CEILING: Duration = STARTING_BUDGET.saturating_mul(2);

    /// Move the clock without a look, as the time between two invokes.
    fn pass(&mut self, by: Duration) {
        self.at += by;
    }
}

impl World for FakeWorld {
    async fn look(&mut self) -> Look {
        self.looks += 1;
        (self.script)(self.at, self.spawns)
    }

    async fn spawn(&mut self) -> Result<SpawnedDaemon, ShellError> {
        self.spawns += 1;
        Ok(SpawnedDaemon {
            pid: self.spawn_pid,
            log_start: 7,
        })
    }

    fn now(&self) -> Instant {
        self.start + self.at
    }

    async fn sleep(&mut self, by: Duration) {
        self.at += by;
        assert!(
            self.at <= Self::CEILING,
            "the wait slept past {:?} after {} looks: its clock no longer counts",
            Self::CEILING,
            self.looks
        );
    }

    async fn log_tail(&mut self, from: Option<u64>) -> Result<(PathBuf, String), String> {
        self.log_reads.push_back(from);
        match &self.log_unreadable {
            Some(why) => Err(why.clone()),
            None => Ok((PathBuf::from("/s/state/logs/daemon.log"), "a line".into())),
        }
    }
}

fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

fn this_build_discovery(pid: u32) -> Discovery {
    let mut d = discovery_of(ShellBuild::THIS.version, ShellBuild::THIS.protocol);
    d.pid = pid;
    d
}

fn healthy(d: &Discovery) -> Look {
    look(
        parsed(d),
        answered(
            &d.instance_id,
            Some(ShellBuild::THIS.protocol),
            ShellBuild::THIS.version,
        ),
        true,
    )
}

fn booting(d: &Discovery) -> Look {
    look(parsed(d), silent(), true)
}

fn kind(result: &Result<Reused, ShellError>) -> String {
    match result {
        Ok(_) => "reused".into(),
        Err(e) => serde_json::to_value(e).unwrap()["kind"]
            .as_str()
            .unwrap()
            .into(),
    }
}

/// The CLI's daemon is still booting (its pid live, its health silent): the
/// app parks in `starting`, invoke after invoke, and reuses it once it
/// answers — one daemon, never a second spawn (the owner's rule: the CLI and
/// the app connect to the same daemon).
#[test]
fn a_daemon_silent_then_answering_is_reused_and_never_spawned_over() {
    let d = this_build_discovery(4242);
    let answers_at = Duration::from_secs(20);
    let script_d = d.clone();
    let mut world = FakeWorld::new(move |at, _| {
        if at < answers_at {
            booting(&script_d)
        } else {
            healthy(&script_d)
        }
    });
    let watch = Mutex::new(StartingWatch::default());
    for _ in 0..19 {
        let result = run(ensure_with(&mut world, &watch, &ShellBuild::THIS));
        assert_eq!(kind(&result), "starting");
        world.pass(Duration::from_secs(1));
    }
    world.pass(Duration::from_secs(1));
    let result = run(ensure_with(&mut world, &watch, &ShellBuild::THIS));
    assert_eq!(kind(&result), "reused");
    assert_eq!(world.spawns, 0);
}

/// Past the budget, counted from the first silent look at that pid, the
/// shell answers the timeout report with the log's last lines, at every
/// invoke, and still spawns nothing.
#[test]
fn a_daemon_silent_past_the_budget_gets_the_timeout_report_and_no_spawn() {
    let d = this_build_discovery(4242);
    let script_d = d.clone();
    let mut world = FakeWorld::new(move |_, _| booting(&script_d));
    let watch = Mutex::new(StartingWatch::default());
    let result = run(ensure_with(&mut world, &watch, &ShellBuild::THIS));
    assert_eq!(kind(&result), "starting");
    world.pass(STARTING_BUDGET - Duration::from_millis(1));
    assert_eq!(
        kind(&run(ensure_with(&mut world, &watch, &ShellBuild::THIS))),
        "starting"
    );
    world.pass(Duration::from_millis(1));
    for _ in 0..3 {
        let result = run(ensure_with(&mut world, &watch, &ShellBuild::THIS));
        assert_eq!(kind(&result), "still_starting");
        let error = serde_json::to_value(result.err().unwrap()).unwrap();
        assert_eq!(
            error["message"],
            "The daemon (pid 4242) is still starting after 45 seconds: it is running but \
             does not answer. Nothing was started, stopped or restarted. The last lines of \
             its log (/s/state/logs/daemon.log):\n\na line"
        );
        assert_eq!(error["log_tail"], "a line");
        world.pass(Duration::from_secs(10));
    }
    assert_eq!(world.spawns, 0);
    // A daemon this run did not spawn has no start in the log to read from.
    assert!(world.log_reads.iter().all(Option::is_none));
}

/// A log that cannot be read past the budget is still the daemon's report,
/// never a failed spawn: the kind stays `still_starting`, the reason said.
#[test]
fn a_log_that_cannot_be_read_past_the_budget_is_still_starting() {
    let d = this_build_discovery(4242);
    let script_d = d.clone();
    let mut world = FakeWorld::new(move |_, _| booting(&script_d));
    world.log_unreadable = Some("Could not read the daemon log: task 3 panicked".into());
    let watch = Mutex::new(StartingWatch::default());
    let _ = run(ensure_with(&mut world, &watch, &ShellBuild::THIS));
    world.pass(STARTING_BUDGET);
    let error = serde_json::to_value(
        run(ensure_with(&mut world, &watch, &ShellBuild::THIS))
            .err()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        error,
        serde_json::json!({
            "kind": "still_starting",
            "message": "The daemon (pid 4242) is still starting after 45 seconds: it is \
                        running but does not answer. Nothing was started, stopped or \
                        restarted, and its log could not be read: Could not read the daemon \
                        log: task 3 panicked",
            "log_tail": null,
        })
    );
    assert_eq!(world.spawns, 0);
}

/// The first silent look is per daemon: another pid starts its own count.
#[test]
fn the_budget_counts_from_the_first_silent_look_at_that_pid() {
    let first = this_build_discovery(4242);
    let second = this_build_discovery(4343);
    let switch_at = STARTING_BUDGET;
    let (a, b) = (first.clone(), second.clone());
    let mut world = FakeWorld::new(move |at, _| {
        if at < switch_at {
            booting(&a)
        } else {
            booting(&b)
        }
    });
    let watch = Mutex::new(StartingWatch::default());
    assert_eq!(
        kind(&run(ensure_with(&mut world, &watch, &ShellBuild::THIS))),
        "starting"
    );
    world.pass(STARTING_BUDGET);
    assert_eq!(
        kind(&run(ensure_with(&mut world, &watch, &ShellBuild::THIS))),
        "starting",
        "a new pid's first look"
    );
}

/// The race the owner's rule judges: the app saw no daemon and spawned, but
/// the CLI's daemon took the store's lock first, so the app's exits 1. The
/// shell follows the daemon `discovery.json` names, the CLI's — booting,
/// then answering — and ends reusing it, never with the spawn-failed report.
#[test]
fn a_spawn_that_lost_the_lock_to_the_cli_s_daemon_ends_reusing_it() {
    let cli = this_build_discovery(5151);
    let script_d = cli.clone();
    let mut world = FakeWorld::new(move |at, spawns| {
        if spawns == 0 {
            look(DiscoveryRead::Missing, silent(), false)
        } else if at < Duration::from_secs(12) {
            booting(&script_d)
        } else {
            healthy(&script_d)
        }
    });
    let watch = Mutex::new(StartingWatch::default());
    let result = run(ensure_with(&mut world, &watch, &ShellBuild::THIS));
    assert_eq!(kind(&result), "reused");
    assert_eq!(result.unwrap().discovery.pid, 5151);
    assert_eq!(world.spawns, 1, "one spawn, never a second");
}

/// The same race with the CLI's daemon past the budget: the timeout report,
/// naming the daemon this ask started apart from the one it waits for, and
/// quoting what the log holds since this ask's spawn, named so; no second
/// spawn.
#[test]
fn a_spawn_that_lost_the_lock_to_a_daemon_still_booting_ends_still_starting() {
    let cli = this_build_discovery(5151);
    let script_d = cli.clone();
    let mut world = FakeWorld::new(move |_, spawns| {
        if spawns == 0 {
            look(DiscoveryRead::Missing, silent(), false)
        } else {
            booting(&script_d)
        }
    });
    let watch = Mutex::new(StartingWatch::default());
    let result = run(ensure_with(&mut world, &watch, &ShellBuild::THIS));
    assert_eq!(kind(&result), "still_starting");
    let error = serde_json::to_value(result.err().unwrap()).unwrap();
    assert_eq!(
        error["message"],
        "The daemon (pid 5151) is still starting after 45 seconds: it is running but does \
         not answer. This app started a daemon too (pid 9090), but the discovery file names \
         this one; nothing was stopped or restarted. What its log \
         (/s/state/logs/daemon.log) holds since this app's start:\n\na line"
    );
    assert_eq!(world.spawns, 1);
    assert_eq!(world.log_reads, [Some(7)]);
}

/// The daemon this ask spawned, booting past the budget: the timeout report
/// as its own, quoting what its log holds since the spawn.
#[test]
fn a_spawned_daemon_still_booting_past_the_budget_is_reported_as_this_ask_s() {
    let ours = this_build_discovery(6161);
    let script_d = ours.clone();
    let mut world = FakeWorld::new(move |_, spawns| {
        if spawns == 0 {
            look(DiscoveryRead::Missing, silent(), false)
        } else {
            booting(&script_d)
        }
    });
    world.spawn_pid = 6161;
    let watch = Mutex::new(StartingWatch::default());
    let result = run(ensure_with(&mut world, &watch, &ShellBuild::THIS));
    let error = serde_json::to_value(result.err().unwrap()).unwrap();
    assert_eq!(
        error["message"],
        "The daemon (pid 6161) is still starting after 45 seconds: it is running but does \
         not answer. Nothing was stopped or restarted. Its log (/s/state/logs/daemon.log) \
         ends with:\n\na line"
    );
    assert_eq!(error["kind"], "still_starting");
    assert_eq!(world.spawns, 1);
    assert_eq!(world.log_reads, [Some(7)]);
}

/// A daemon this run spawned that boots slowly (its pid live, health silent
/// past 5 s) is waited for under the budget, not failed at 5 s.
#[test]
fn a_spawned_daemon_booting_past_five_seconds_is_waited_for() {
    let d = this_build_discovery(6161);
    let script_d = d.clone();
    let mut world = FakeWorld::new(move |at, spawns| {
        if spawns == 0 {
            look(DiscoveryRead::Missing, silent(), false)
        } else if at < Duration::from_secs(30) {
            booting(&script_d)
        } else {
            healthy(&script_d)
        }
    });
    let watch = Mutex::new(StartingWatch::default());
    let result = run(ensure_with(&mut world, &watch, &ShellBuild::THIS));
    assert_eq!(kind(&result), "reused");
    assert_eq!(world.spawns, 1);
}

/// A spawned daemon that never writes a discovery file, or whose pid is
/// gone, fails at 5 s with the two pinned sentences' report.
#[test]
fn a_spawned_daemon_that_never_comes_up_fails_at_five_seconds() {
    let gone = this_build_discovery(7171);
    for never in [
        look(DiscoveryRead::Missing, silent(), false),
        look(parsed(&gone), silent(), false),
    ] {
        let mut world = FakeWorld::new(move |_, _| Look {
            discovery: match &never.discovery {
                DiscoveryRead::Missing => DiscoveryRead::Missing,
                DiscoveryRead::Parsed(d) => DiscoveryRead::Parsed(d.clone()),
                _ => unreachable!(),
            },
            health: silent(),
            pid_live: false,
        });
        let watch = Mutex::new(StartingWatch::default());
        let result = run(ensure_with(&mut world, &watch, &ShellBuild::THIS));
        assert_eq!(kind(&result), "spawn_failed");
        let error = serde_json::to_value(result.err().unwrap()).unwrap();
        assert_eq!(
            error["message"],
            "The daemon did not start within 5 seconds. Its log (/s/state/logs/daemon.log) \
             ends with:\n\na line"
        );
        assert_eq!(world.spawns, 1);
        assert!(world.at >= SPAWN_WAIT && world.at < SPAWN_WAIT + Duration::from_secs(1));
        // Only what this ask's spawn wrote, from where the log stood before it.
        assert_eq!(world.log_reads, [Some(7)]);
    }
}

/// A mismatch found after the spawn (the daemon that won the lock speaks
/// another protocol) is the mismatch, not a second spawn.
#[test]
fn a_mismatch_found_after_the_spawn_is_reported() {
    let newer = discovery_of("9.9.9", ShellBuild::THIS.protocol + 1);
    let mut world = FakeWorld::new(move |_, spawns| {
        if spawns == 0 {
            look(DiscoveryRead::Missing, silent(), false)
        } else {
            look(
                parsed(&newer),
                answered(INSTANCE, Some(ShellBuild::THIS.protocol + 1), "9.9.9"),
                true,
            )
        }
    });
    let watch = Mutex::new(StartingWatch::default());
    let result = run(ensure_with(&mut world, &watch, &ShellBuild::THIS));
    assert_eq!(kind(&result), "mismatch");
    assert_eq!(world.spawns, 1);
}

/// A mismatch at the first look spawns nothing, kills nothing.
#[test]
fn a_mismatch_at_the_first_look_spawns_nothing() {
    let newer = discovery_of("9.9.9", ShellBuild::THIS.protocol + 1);
    let mut world = FakeWorld::new(move |_, _| {
        look(
            parsed(&newer),
            answered(INSTANCE, Some(ShellBuild::THIS.protocol + 1), "9.9.9"),
            true,
        )
    });
    let watch = Mutex::new(StartingWatch::default());
    let result = run(ensure_with(&mut world, &watch, &ShellBuild::THIS));
    assert_eq!(kind(&result), "mismatch");
    assert_eq!(world.spawns, 0);
    assert_eq!(world.looks, 1);
}

/// The starting answer's sentence, which the window shows while it waits.
#[test]
fn the_starting_answer_names_the_pid_and_the_budget() {
    let d = this_build_discovery(4242);
    let script_d = d.clone();
    let mut world = FakeWorld::new(move |_, _| booting(&script_d));
    let watch = Mutex::new(StartingWatch::default());
    let error = serde_json::to_value(
        run(ensure_with(&mut world, &watch, &ShellBuild::THIS))
            .err()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        error,
        serde_json::json!({
            "kind": "starting",
            "message": "The daemon (pid 4242) is starting: it has written its discovery \
                        file but does not answer yet. Waiting for it, up to 45 seconds.",
        })
    );
}
