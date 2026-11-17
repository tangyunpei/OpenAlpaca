//! `decide_reuse`'s verdicts as this shell words them (the table itself is
//! `openalpaca_app_support::protocol`'s tests since Q05; Q04, plan-rev3 §7.2
//! G3-b, DU-29 (a), C-09, A-18, A-20) and the start it drives, over a world the test writes: no file, no
//! process table, no socket and no clock of the machine's. Nothing here
//! spawns, reads the store or opens a port, so no test holds the home-store
//! guard.

use super::*;
use openalpaca_app_support::discovery::{self, Discovery};
use std::collections::VecDeque;

// ============================================================================
// The table
// ============================================================================

/// A shell that speaks protocol 3 and reads back to 2, so the rows that need
/// a protocol older than this shell's but still supported, and one too old,
/// have values to run on (the build's own constants are both 1 today).
const SHELL: ClientBuild = ClientBuild {
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
        Verdict::Reuse(reused) => Row::Reuse(reuse_note(&SHELL, &reused)),
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
    assert_eq!(THIS_SHELL.protocol, THIS_SHELL.oldest_supported);
    let mut d = discovery_of("0.1.0", THIS_SHELL.protocol + 1);
    d.build.version = "9.9.9".into();
    let Verdict::Mismatch(m) = decide_reuse(
        &look(
            parsed(&d),
            answered(INSTANCE, Some(THIS_SHELL.protocol + 1), "9.9.9"),
            true,
        ),
        &THIS_SHELL,
    ) else {
        panic!("not a mismatch");
    };
    let message = m.into_error().to_string();
    assert!(
        message.contains(&format!(
            "the app speaks protocol {}. Nothing",
            THIS_SHELL.protocol
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
    /// A spawn refused: `true` for the sidecar missing, `false` for another
    /// failure; `None` spawns.
    refuse_spawn: Option<bool>,
    /// The lines the shell logged, in order.
    lines: Vec<String>,
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
            refuse_spawn: None,
            lines: Vec::new(),
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

    async fn spawn(&mut self) -> Result<SpawnedDaemon, SpawnRefusal> {
        match self.refuse_spawn {
            Some(true) => {
                return Err(SpawnRefusal::SidecarMissing(crate::spawn_failed(
                    "Failed to spawn daemon: Daemon binary not found at /a/App/openalpacad. \
                     The application may be incorrectly installed."
                        .into(),
                )));
            }
            Some(false) => {
                return Err(SpawnRefusal::Failed(crate::spawn_failed(
                    "Failed to spawn daemon: Could not resolve the app's path /a/App: gone".into(),
                )));
            }
            None => {}
        }
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

    fn log(&mut self, line: String) {
        self.lines.push(line);
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
    let mut d = discovery_of(THIS_SHELL.version, THIS_SHELL.protocol);
    d.pid = pid;
    d
}

fn healthy(d: &Discovery) -> Look {
    look(
        parsed(d),
        answered(
            &d.instance_id,
            Some(THIS_SHELL.protocol),
            THIS_SHELL.version,
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

/// The sentence every `still_starting` report ends with (*Rev 106* (9), the
/// advisor's pick): the way out, worded for a boot that may still finish.
const WAY_OUT: &str = "If it does not finish starting, stop it with `openalpaca daemon stop` \
                       and press Reconnect in Settings → Connection to start a new one.";

/// One ask of the window's: `ensure_with` over `world`, from [`WINDOW`].
fn ask(world: &mut FakeWorld, watch: &Mutex<StartingWatch>) -> Result<Reused, ShellError> {
    run(ensure_with(world, watch, &THIS_SHELL, WINDOW))
}

/// The window's asks, one a second as `STARTING_RETRY_MS` paces them, until
/// one answers other than `starting`: every answer's kind, in order, and the
/// last answer.
fn ask_until_settled(
    world: &mut FakeWorld,
    watch: &Mutex<StartingWatch>,
) -> (Vec<String>, Result<Reused, ShellError>) {
    let mut kinds = Vec::new();
    loop {
        let result = ask(world, watch);
        kinds.push(kind(&result));
        if kind(&result) != "starting" {
            return (kinds, result);
        }
        assert!(kinds.len() < 120, "the asks never settled: {kinds:?}");
        world.pass(Duration::from_secs(1));
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
        let result = ask(&mut world, &watch);
        assert_eq!(kind(&result), "starting");
        world.pass(Duration::from_secs(1));
    }
    world.pass(Duration::from_secs(1));
    let result = ask(&mut world, &watch);
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
    let result = ask(&mut world, &watch);
    assert_eq!(kind(&result), "starting");
    world.pass(STARTING_BUDGET - Duration::from_millis(1));
    assert_eq!(kind(&ask(&mut world, &watch)), "starting");
    world.pass(Duration::from_millis(1));
    for _ in 0..3 {
        let result = ask(&mut world, &watch);
        assert_eq!(kind(&result), "still_starting");
        let error = serde_json::to_value(result.err().unwrap()).unwrap();
        assert_eq!(
            error["message"],
            format!(
                "The daemon (pid 4242) is still starting after 45 seconds: it is running but \
                 does not answer. Nothing was started, stopped or restarted. The last lines of \
                 its log (/s/state/logs/daemon.log):\n\na line\n\n{WAY_OUT}"
            )
        );
        assert_eq!(error["log_tail"], "a line");
        world.pass(Duration::from_secs(10));
    }
    assert_eq!(world.spawns, 0);
    // A daemon this run did not spawn has no start in the log to read from.
    assert!(world.log_reads.iter().all(Option::is_none));
}

/// A log that cannot be read past the budget is still the daemon's report,
/// never a failed spawn: the kind stays `still_starting`, the reason said,
/// and the way out after it.
#[test]
fn a_log_that_cannot_be_read_past_the_budget_is_still_starting() {
    let d = this_build_discovery(4242);
    let script_d = d.clone();
    let mut world = FakeWorld::new(move |_, _| booting(&script_d));
    world.log_unreadable = Some("Could not read the daemon log: task 3 panicked".into());
    let watch = Mutex::new(StartingWatch::default());
    let _ = ask(&mut world, &watch);
    world.pass(STARTING_BUDGET);
    let error = serde_json::to_value(ask(&mut world, &watch).err().unwrap()).unwrap();
    assert_eq!(
        error,
        serde_json::json!({
            "kind": "still_starting",
            "message": format!(
                "The daemon (pid 4242) is still starting after 45 seconds: it is running but \
                 does not answer. Nothing was started, stopped or restarted, and its log could \
                 not be read: Could not read the daemon log: task 3 panicked. {WAY_OUT}"
            ),
            "log_tail": null,
        })
    );
    assert_eq!(world.spawns, 0);
}

/// Every shape of the timeout report ends with the way out (*Rev 106* (9)):
/// a log with lines, a log that holds nothing, a log that cannot be read;
/// a daemon found, the one this app spawned, and another that won the lock.
#[test]
fn every_still_starting_report_ends_with_the_way_out() {
    let path = PathBuf::from("/s/state/logs/daemon.log");
    for waited in [Waited::Found, Waited::Spawned, Waited::Other { ours: 9090 }] {
        for read in [
            Ok((path.clone(), "a line".to_owned())),
            Ok((path.clone(), String::new())),
            Err("Could not read the daemon log: task 3 panicked".to_owned()),
            Err("the log's path could not be had.".to_owned()),
        ] {
            let error = serde_json::to_value(still_starting(4242, read.clone(), waited)).unwrap();
            let message = error["message"].as_str().unwrap();
            assert!(
                message.ends_with(&format!(". {WAY_OUT}"))
                    || message.ends_with(&format!("\n\n{WAY_OUT}")),
                "{waited:?} {read:?}: {message}"
            );
            assert_eq!(message.matches(WAY_OUT).count(), 1, "{message}");
            assert!(!message.contains(".."), "{message}");
        }
    }
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
    assert_eq!(kind(&ask(&mut world, &watch)), "starting");
    world.pass(STARTING_BUDGET);
    assert_eq!(
        kind(&ask(&mut world, &watch)),
        "starting",
        "a new pid's first look"
    );
}

/// *Rev 106* (8), the advisor's pick: after this app's own spawn, once
/// `discovery.json` names a live `openalpacad` whose health is silent, the
/// ask answers `starting` at once — the window reads `Daemon is starting`,
/// not `connecting` — and never waits in the ask.
#[test]
fn after_a_spawn_a_live_silent_daemon_answers_starting_at_once() {
    let ours = this_build_discovery(9090);
    let script_d = ours.clone();
    let mut world = FakeWorld::new(move |at, spawns| {
        if spawns == 0 || at < Duration::from_millis(400) {
            look(DiscoveryRead::Missing, silent(), false)
        } else {
            booting(&script_d)
        }
    });
    let watch = Mutex::new(StartingWatch::default());
    let error = serde_json::to_value(ask(&mut world, &watch).err().unwrap()).unwrap();
    assert_eq!(
        error,
        serde_json::json!({
            "kind": "starting",
            "message": "The daemon (pid 9090) is starting: it has written its discovery \
                        file but does not answer yet. Waiting for it, up to 45 seconds.",
        })
    );
    assert_eq!(world.spawns, 1);
    // The spawn's polls until the file named it, and no longer.
    assert_eq!(world.at, Duration::from_millis(400));
}

/// The next asks after a spawn keep the budget's count from the first
/// silent look, made inside the spawn's ask: no ask restarts it, none
/// spawns again, and past it the report is about this app's own daemon,
/// quoting what its log holds since the spawn.
#[test]
fn the_asks_after_a_spawn_keep_the_first_silent_look_s_count() {
    let ours = this_build_discovery(9090);
    let script_d = ours.clone();
    let mut world = FakeWorld::new(move |_, spawns| {
        if spawns == 0 {
            look(DiscoveryRead::Missing, silent(), false)
        } else {
            booting(&script_d)
        }
    });
    let watch = Mutex::new(StartingWatch::default());
    assert_eq!(kind(&ask(&mut world, &watch)), "starting");
    let first_silent = world.at;
    world.pass(STARTING_BUDGET - Duration::from_millis(1));
    assert_eq!(kind(&ask(&mut world, &watch)), "starting");
    world.pass(Duration::from_millis(1));
    assert_eq!(world.at - first_silent, STARTING_BUDGET);
    let error = serde_json::to_value(ask(&mut world, &watch).err().unwrap()).unwrap();
    assert_eq!(error["kind"], "still_starting");
    assert_eq!(
        error["message"],
        format!(
            "The daemon (pid 9090) is still starting after 45 seconds: it is running but does \
             not answer. Nothing was stopped or restarted. Its log (/s/state/logs/daemon.log) \
             ends with:\n\na line\n\n{WAY_OUT}"
        )
    );
    assert_eq!(world.spawns, 1);
    assert_eq!(world.log_reads, [Some(7)]);
}

/// The race the owner's rule judges: the app saw no daemon and spawned, but
/// the CLI's daemon took the store's lock first, so the app's exits 1. The
/// shell follows the daemon `discovery.json` names, the CLI's — booting:
/// `starting` at every ask, then answering — and ends reusing it, never with
/// the spawn-failed report and never with a second spawn.
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
    let (kinds, result) = ask_until_settled(&mut world, &watch);
    assert_eq!(kinds.len(), 13, "{kinds:?}");
    assert!(kinds[..12].iter().all(|k| k == "starting"), "{kinds:?}");
    assert_eq!(kind(&result), "reused");
    assert_eq!(result.unwrap().discovery.pid, 5151);
    assert_eq!(world.spawns, 1, "one spawn, never a second");
}

/// The same race with the CLI's daemon past the budget: `starting` at every
/// ask until the budget has passed, then the timeout report, naming the
/// daemon this app started apart from the one it waits for, and quoting
/// what the log holds since this app's spawn, named so; no second spawn.
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
    let (kinds, result) = ask_until_settled(&mut world, &watch);
    assert_eq!(kinds.len(), 46, "{kinds:?}");
    assert_eq!(kind(&result), "still_starting");
    let error = serde_json::to_value(result.err().unwrap()).unwrap();
    assert_eq!(
        error["message"],
        format!(
            "The daemon (pid 5151) is still starting after 45 seconds: it is running but does \
             not answer. This app started a daemon too (pid 9090), but the discovery file \
             names this one; nothing was stopped or restarted. What its log \
             (/s/state/logs/daemon.log) holds since this app's start:\n\na line\n\n{WAY_OUT}"
        )
    );
    assert_eq!(world.spawns, 1);
    assert_eq!(world.log_reads, [Some(7)]);
}

/// The daemon this app spawned, booting past the budget: `starting` at
/// every ask, then the timeout report as its own, quoting what its log
/// holds since the spawn.
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
    let (kinds, result) = ask_until_settled(&mut world, &watch);
    assert_eq!(kinds.len(), 46, "{kinds:?}");
    let error = serde_json::to_value(result.err().unwrap()).unwrap();
    assert_eq!(
        error["message"],
        format!(
            "The daemon (pid 6161) is still starting after 45 seconds: it is running but does \
             not answer. Nothing was stopped or restarted. Its log (/s/state/logs/daemon.log) \
             ends with:\n\na line\n\n{WAY_OUT}"
        )
    );
    assert_eq!(error["kind"], "still_starting");
    assert_eq!(world.spawns, 1);
    assert_eq!(world.log_reads, [Some(7)]);
}

/// A daemon this run spawned that boots slowly (its pid live, health silent
/// past 5 s) is waited for under the budget, not failed at 5 s: `starting`
/// at every ask until it answers, then reused, with one spawn.
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
    let (kinds, result) = ask_until_settled(&mut world, &watch);
    assert!(kinds.len() > 1, "{kinds:?}");
    assert_eq!(kind(&result), "reused");
    assert_eq!(world.spawns, 1);
}

/// A spawned daemon that writes its file and dies after the ask answered
/// `starting` is the failed start, as when it died inside the one ask: the
/// next ask finds its pid gone and reports this spawn's log lines, never
/// spawning again — a daemon that dies at every boot is not restarted
/// every second, its reason unseen.
#[test]
fn a_spawned_daemon_that_dies_after_answering_starting_is_reported_not_spawned_again() {
    let ours = this_build_discovery(9090);
    let script_d = ours.clone();
    let mut world = FakeWorld::new(move |at, spawns| match spawns {
        0 => look(DiscoveryRead::Missing, silent(), false),
        1 if at < Duration::from_millis(500) => booting(&script_d),
        1 => look(parsed(&script_d), silent(), false),
        _ => healthy(&script_d),
    });
    let watch = Mutex::new(StartingWatch::default());
    assert_eq!(kind(&ask(&mut world, &watch)), "starting");
    world.pass(Duration::from_secs(1));
    let result = ask(&mut world, &watch);
    assert_eq!(kind(&result), "spawn_failed");
    let error = serde_json::to_value(result.err().unwrap()).unwrap();
    assert_eq!(
        error["message"],
        "The daemon did not start within 5 seconds. Its log (/s/state/logs/daemon.log) ends \
         with:\n\na line"
    );
    assert_eq!(world.spawns, 1, "never a second spawn");
    assert_eq!(world.log_reads, [Some(7)]);
    // The wait ended where the one ask's would have: 5 s after the spawn.
    assert!(world.at >= SPAWN_WAIT && world.at < SPAWN_WAIT + Duration::from_secs(1));
    // The failed start is over: the next ask, a Reconnect, spawns afresh.
    let result = ask(&mut world, &watch);
    assert_eq!(kind(&result), "reused");
    assert_eq!(world.spawns, 2);
}

/// Once the timeout report has been answered after a spawn, the spawn's
/// wait is over, as the one ask's was: a Reconnect after the daemon was
/// stopped (the report's way out) spawns afresh. The Reconnect comes 10 s
/// after the report, well inside the budget since the last ask, so it is
/// the report, not the spawn's staleness, that ends the wait (green on the
/// parent, where the one ask ended it; a guard).
#[test]
fn after_the_report_a_stopped_daemon_s_reconnect_spawns_afresh() {
    let ours = this_build_discovery(9090);
    let script_d = ours.clone();
    let stopped_at = STARTING_BUDGET + Duration::from_secs(5);
    let mut world = FakeWorld::new(move |at, spawns| match spawns {
        0 => look(DiscoveryRead::Missing, silent(), false),
        1 if at < stopped_at => booting(&script_d),
        1 => look(DiscoveryRead::Missing, silent(), false),
        _ => healthy(&script_d),
    });
    let watch = Mutex::new(StartingWatch::default());
    let (_, result) = ask_until_settled(&mut world, &watch);
    assert_eq!(kind(&result), "still_starting");
    world.pass(Duration::from_secs(10));
    let result = ask(&mut world, &watch);
    assert_eq!(kind(&result), "reused");
    assert_eq!(world.spawns, 2);
}

/// A spawn whose `starting` answer no ask followed for the budget (the
/// window closed or disconnected) is over by then, as the one ask's wait
/// would have been: a much later ask that finds no daemon spawns afresh,
/// never a failed start quoting that old spawn's lines.
#[test]
fn a_spawn_no_ask_followed_for_the_budget_is_forgotten() {
    let ours = this_build_discovery(9090);
    let script_d = ours.clone();
    let mut world = FakeWorld::new(move |at, spawns| match spawns {
        0 => look(DiscoveryRead::Missing, silent(), false),
        1 if at < Duration::from_secs(1) => booting(&script_d),
        1 => look(DiscoveryRead::Missing, silent(), false),
        _ => healthy(&script_d),
    });
    let watch = Mutex::new(StartingWatch::default());
    assert_eq!(kind(&ask(&mut world, &watch)), "starting");
    world.pass(STARTING_BUDGET);
    let result = ask(&mut world, &watch);
    assert_eq!(kind(&result), "reused");
    assert_eq!(world.spawns, 2);
    assert!(world.log_reads.is_empty(), "{:?}", world.log_reads);
}

/// The spawn is forgotten only once the budget has passed since the last
/// ask, never since the spawn: a spawned daemon that dies just before its
/// own budget (counted from the first silent look, which comes after the
/// spawn) ends, while the window asks every second, is the failed start
/// with this spawn's lines, never a silent second spawn.
#[test]
fn a_spawned_daemon_dying_just_before_its_budget_is_reported_not_respawned() {
    let ours = this_build_discovery(9090);
    let script_d = ours.clone();
    let dies_at = Duration::from_millis(44_500);
    let mut world = FakeWorld::new(move |at, spawns| match spawns {
        0 => look(DiscoveryRead::Missing, silent(), false),
        1 if at < dies_at => booting(&script_d),
        1 => look(parsed(&script_d), silent(), false),
        _ => healthy(&script_d),
    });
    let watch = Mutex::new(StartingWatch::default());
    let (_, result) = ask_until_settled(&mut world, &watch);
    assert_eq!(kind(&result), "spawn_failed");
    assert_eq!(world.spawns, 1, "never a second spawn");
    assert_eq!(world.log_reads, [Some(7)]);
}

/// A spawn no ask followed for the budget is forgotten by every reader: a
/// daemon found starting much later (the CLI's) is reported as a found
/// one — the log's last lines, no word of this app's old spawn — and its
/// reuse is one already running.
#[test]
fn a_forgotten_spawn_names_no_later_daemon() {
    let ours = this_build_discovery(9090);
    let cli = this_build_discovery(5151);
    let (a, b) = (ours.clone(), cli.clone());
    let mut world = FakeWorld::new(move |at, spawns| match spawns {
        0 => look(DiscoveryRead::Missing, silent(), false),
        _ if at < Duration::from_secs(1) => booting(&a),
        _ if at < Duration::from_secs(200) => booting(&b),
        _ => healthy(&b),
    });
    let watch = Mutex::new(StartingWatch::default());
    assert_eq!(kind(&ask(&mut world, &watch)), "starting");
    world.pass(Duration::from_secs(100));
    assert_eq!(kind(&ask(&mut world, &watch)), "starting");
    world.pass(STARTING_BUDGET);
    let result = ask(&mut world, &watch);
    assert_eq!(kind(&result), "still_starting");
    let message = serde_json::to_value(result.err().unwrap()).unwrap()["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        message.contains("Nothing was started, stopped or restarted. The last lines of its log"),
        "{message}"
    );
    assert!(!message.contains("9090"), "{message}");
    assert_eq!(world.log_reads, [None]);
    world.pass(Duration::from_secs(100));
    assert_eq!(kind(&ask(&mut world, &watch)), "reused");
    assert_eq!(world.spawns, 1);
    assert!(
        world
            .lines
            .last()
            .is_some_and(|l| l.ends_with("already running: this run of the app did not start it.")),
        "{:?}",
        world.lines
    );
}

/// The timeout report after a spawn ends the spawn's wait: an ask after it
/// reports the daemon as a found one (the log's last lines), whether it is
/// this app's own or the one that took the lock from it, and the latter's
/// later reuse is one already running.
#[test]
fn after_the_report_the_spawn_s_wait_is_over_for_every_reader() {
    // This app's own, still booting.
    let ours = this_build_discovery(9090);
    let script_d = ours.clone();
    let mut world = FakeWorld::new(move |_, spawns| match spawns {
        0 => look(DiscoveryRead::Missing, silent(), false),
        _ => booting(&script_d),
    });
    let watch = Mutex::new(StartingWatch::default());
    let (_, result) = ask_until_settled(&mut world, &watch);
    assert_eq!(kind(&result), "still_starting");
    world.pass(Duration::from_secs(1));
    let result = ask(&mut world, &watch);
    assert_eq!(kind(&result), "still_starting");
    let message = serde_json::to_value(result.err().unwrap()).unwrap()["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        message.contains("Nothing was started, stopped or restarted. The last lines of its log"),
        "{message}"
    );
    assert_eq!(world.log_reads, [Some(7), None]);
    // The CLI's, which took the lock from this app's own, answering later.
    let cli = this_build_discovery(5151);
    let script_d = cli.clone();
    let mut world = FakeWorld::new(move |at, spawns| match spawns {
        0 => look(DiscoveryRead::Missing, silent(), false),
        _ if at < Duration::from_secs(60) => booting(&script_d),
        _ => healthy(&script_d),
    });
    let watch = Mutex::new(StartingWatch::default());
    let (_, result) = ask_until_settled(&mut world, &watch);
    assert_eq!(kind(&result), "still_starting");
    world.pass(Duration::from_secs(1));
    let result = ask(&mut world, &watch);
    assert_eq!(kind(&result), "still_starting");
    let message = serde_json::to_value(result.err().unwrap()).unwrap()["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!message.contains("9090"), "{message}");
    assert_eq!(world.log_reads, [Some(7), None]);
    world.pass(Duration::from_secs(20));
    assert_eq!(kind(&ask(&mut world, &watch)), "reused");
    assert_eq!(world.spawns, 1);
    assert!(
        world
            .lines
            .last()
            .is_some_and(|l| l.ends_with("already running: this run of the app did not start it.")),
        "{:?}",
        world.lines
    );
}

/// After the timeout report has ended the spawn's wait, this app's own
/// daemon, answering at last, is reused as the daemon this app started:
/// only the watch's list of this run's spawns still knows it (the pending
/// spawn is gone with the report), so `gui.log` never calls it one already
/// running.
#[test]
fn this_app_s_own_daemon_reused_after_the_report_is_named_as_its_own() {
    let ours = this_build_discovery(9090);
    let script_d = ours.clone();
    let answers_at = STARTING_BUDGET + Duration::from_secs(20);
    let mut world = FakeWorld::new(move |at, spawns| match spawns {
        0 => look(DiscoveryRead::Missing, silent(), false),
        _ if at < answers_at => booting(&script_d),
        _ => healthy(&script_d),
    });
    let watch = Mutex::new(StartingWatch::default());
    let (_, result) = ask_until_settled(&mut world, &watch);
    assert_eq!(kind(&result), "still_starting");
    world.pass(Duration::from_secs(30));
    assert_eq!(kind(&ask(&mut world, &watch)), "reused");
    assert_eq!(world.spawns, 1);
    assert!(
        world
            .lines
            .last()
            .is_some_and(|l| l.ends_with("the daemon this app started.")),
        "{:?}",
        world.lines
    );
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
        let result = ask(&mut world, &watch);
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
    let newer = discovery_of("9.9.9", THIS_SHELL.protocol + 1);
    let mut world = FakeWorld::new(move |_, spawns| {
        if spawns == 0 {
            look(DiscoveryRead::Missing, silent(), false)
        } else {
            look(
                parsed(&newer),
                answered(INSTANCE, Some(THIS_SHELL.protocol + 1), "9.9.9"),
                true,
            )
        }
    });
    let watch = Mutex::new(StartingWatch::default());
    let result = ask(&mut world, &watch);
    assert_eq!(kind(&result), "mismatch");
    assert_eq!(world.spawns, 1);
}

/// A mismatch at the first look spawns nothing, kills nothing.
#[test]
fn a_mismatch_at_the_first_look_spawns_nothing() {
    let newer = discovery_of("9.9.9", THIS_SHELL.protocol + 1);
    let mut world = FakeWorld::new(move |_, _| {
        look(
            parsed(&newer),
            answered(INSTANCE, Some(THIS_SHELL.protocol + 1), "9.9.9"),
            true,
        )
    });
    let watch = Mutex::new(StartingWatch::default());
    let result = ask(&mut world, &watch);
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
    let error = serde_json::to_value(ask(&mut world, &watch).err().unwrap()).unwrap();
    assert_eq!(
        error,
        serde_json::json!({
            "kind": "starting",
            "message": "The daemon (pid 4242) is starting: it has written its discovery \
                        file but does not answer yet. Waiting for it, up to 45 seconds.",
        })
    );
}

// ============================================================================
// The verdicts' lines in gui.log
// ============================================================================

/// The window every ask above comes from.
const WINDOW: &str = "main";

/// The lines `world` logged, with the prefix every verdict line carries for
/// `window` taken off; a line without it fails the test.
fn verdict_lines(world: &FakeWorld, window: &str) -> Vec<String> {
    let prefix = format!("Daemon verdict for window `{window}`: ");
    world
        .lines
        .iter()
        .filter(|line| line.contains(&format!("window `{window}`")))
        .map(|line| {
            line.strip_prefix(&prefix)
                .unwrap_or_else(|| panic!("not a verdict line: {line}"))
                .to_owned()
        })
        .collect()
}

/// Q04-LEG-B found the shell logs nothing for these verdicts: one line per
/// change, not per ask — the window asks every second while a daemon
/// starts. `starting` asked three times is one line; `still_starting` asked
/// twice, one more; the reuse asked twice, one more.
#[test]
fn a_verdict_asked_again_logs_once_and_a_change_logs_again() {
    let d = this_build_discovery(4242);
    let script_d = d.clone();
    let answers_at = STARTING_BUDGET + Duration::from_secs(15);
    let mut world = FakeWorld::new(move |at, _| {
        if at < answers_at {
            booting(&script_d)
        } else {
            healthy(&script_d)
        }
    });
    let watch = Mutex::new(StartingWatch::default());
    for _ in 0..3 {
        assert_eq!(kind(&ask(&mut world, &watch)), "starting");
        world.pass(Duration::from_secs(1));
    }
    world.pass(STARTING_BUDGET);
    for _ in 0..2 {
        assert_eq!(kind(&ask(&mut world, &watch)), "still_starting");
        world.pass(Duration::from_secs(1));
    }
    world.pass(Duration::from_secs(15));
    for _ in 0..2 {
        assert_eq!(kind(&ask(&mut world, &watch)), "reused");
    }
    assert_eq!(
        verdict_lines(&world, WINDOW),
        [
            "starting — pid 4242 has written its discovery file but its health does not answer \
             yet; waiting up to 45 s, starting nothing beside it."
                .to_owned(),
            "still starting — pid 4242 has not answered 45 s after the first silent look; \
             reported, nothing spawned over it."
                .to_owned(),
            format!(
                "reuse — pid 4242, version {}, protocol {}, already running: this run of the \
                 app did not start it.",
                THIS_SHELL.version, THIS_SHELL.protocol
            ),
        ]
    );
    assert_eq!(world.lines.len(), 3);
}

/// A mismatch asked again is one line, naming both sides' protocols.
#[test]
fn a_mismatch_asked_again_logs_once() {
    let newer = discovery_of("9.9.9", THIS_SHELL.protocol + 1);
    let mut world = FakeWorld::new(move |_, _| {
        look(
            parsed(&newer),
            answered(INSTANCE, Some(THIS_SHELL.protocol + 1), "9.9.9"),
            true,
        )
    });
    let watch = Mutex::new(StartingWatch::default());
    for _ in 0..3 {
        assert_eq!(kind(&ask(&mut world, &watch)), "mismatch");
    }
    assert_eq!(
        verdict_lines(&world, WINDOW),
        [format!(
            "refusal (mismatch) — pid 4242, version 9.9.9, protocol {}; this app speaks \
             protocol {}; nothing was stopped or started.",
            THIS_SHELL.protocol + 1,
            THIS_SHELL.protocol
        )]
    );
}

/// A sidecar missing asked again is one line, naming no path; a spawn that
/// fails otherwise is no line of its own but a change, so the sidecar
/// missing after it is logged again.
#[test]
fn a_missing_sidecar_asked_again_logs_once() {
    let mut world = FakeWorld::new(|_, _| look(DiscoveryRead::Missing, silent(), false));
    world.refuse_spawn = Some(true);
    let watch = Mutex::new(StartingWatch::default());
    for _ in 0..3 {
        let result = ask(&mut world, &watch);
        assert_eq!(kind(&result), "spawn_failed");
    }
    world.refuse_spawn = Some(false);
    assert_eq!(kind(&ask(&mut world, &watch)), "spawn_failed");
    world.refuse_spawn = Some(true);
    assert_eq!(kind(&ask(&mut world, &watch)), "spawn_failed");
    let missing = format!(
        "refusal (sidecar missing) — no `{}` beside the app; nothing was started.",
        crate::SIDECAR_NAME
    );
    assert_eq!(verdict_lines(&world, WINDOW), [missing.clone(), missing]);
    assert_eq!(world.spawns, 0, "a refused spawn started nothing");
}

/// Each window keeps its own last verdict: the same `starting` asked from
/// two windows is a line for each, and asked again, none.
#[test]
fn each_window_logs_its_own_changes() {
    let d = this_build_discovery(4242);
    let script_d = d.clone();
    let mut world = FakeWorld::new(move |_, _| booting(&script_d));
    let watch = Mutex::new(StartingWatch::default());
    for _ in 0..2 {
        for window in ["main", "second"] {
            let result = run(ensure_with(&mut world, &watch, &THIS_SHELL, window));
            assert_eq!(kind(&result), "starting");
        }
    }
    assert_eq!(verdict_lines(&world, "main").len(), 1);
    assert_eq!(verdict_lines(&world, "second").len(), 1);
    assert_eq!(world.lines.len(), 2);
}

/// The reuse line says whose daemon it is, as far as the shell knows: the
/// one this app spawned, another than the one this app spawned while that
/// spawn's wait is not over (no cause claimed: the shell cannot know it),
/// or one already running.
#[test]
fn the_reuse_line_says_whose_daemon_it_is() {
    let version = THIS_SHELL.version;
    let protocol = THIS_SHELL.protocol;
    // This app's own.
    let ours = this_build_discovery(9090);
    let script_d = ours.clone();
    let mut world = FakeWorld::new(move |_, spawns| {
        if spawns == 0 {
            look(DiscoveryRead::Missing, silent(), false)
        } else {
            healthy(&script_d)
        }
    });
    let watch = Mutex::new(StartingWatch::default());
    assert_eq!(kind(&ask(&mut world, &watch)), "reused");
    assert_eq!(
        verdict_lines(&world, WINDOW),
        [format!(
            "reuse — pid 9090, version {version}, protocol {protocol}, the daemon this app \
             started."
        )]
    );
    // Another's, named while this app's spawn waits: `starting` while it
    // boots, then the reuse, naming this app's own pid.
    let cli = this_build_discovery(5151);
    let script_d = cli.clone();
    let mut world = FakeWorld::new(move |at, spawns| {
        if spawns == 0 {
            look(DiscoveryRead::Missing, silent(), false)
        } else if at < Duration::from_secs(3) {
            booting(&script_d)
        } else {
            healthy(&script_d)
        }
    });
    let watch = Mutex::new(StartingWatch::default());
    let (_, result) = ask_until_settled(&mut world, &watch);
    assert_eq!(kind(&result), "reused");
    let lines = verdict_lines(&world, WINDOW);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(
        lines[1],
        format!(
            "reuse — pid 5151, version {version}, protocol {protocol}, not the one this app \
             started (pid 9090)."
        )
    );
}

/// No line carries the daemon's token, its log's lines or a path: the
/// discovery file's token, the tail the timeout report quotes and the log's
/// path stay out of `gui.log`.
#[test]
fn no_verdict_line_carries_the_token_the_log_or_a_path() {
    let d = this_build_discovery(4242);
    let token = d.auth.token.clone();
    assert!(!token.is_empty());
    let script_d = d.clone();
    let mut world = FakeWorld::new(move |at, _| {
        if at < STARTING_BUDGET + Duration::from_secs(5) {
            booting(&script_d)
        } else {
            healthy(&script_d)
        }
    });
    let watch = Mutex::new(StartingWatch::default());
    let _ = ask(&mut world, &watch);
    world.pass(STARTING_BUDGET);
    assert_eq!(kind(&ask(&mut world, &watch)), "still_starting");
    world.pass(Duration::from_secs(5));
    assert_eq!(kind(&ask(&mut world, &watch)), "reused");
    let mut missing = FakeWorld::new(|_, _| look(DiscoveryRead::Missing, silent(), false));
    missing.refuse_spawn = Some(true);
    let _ = ask(&mut missing, &Mutex::new(StartingWatch::default()));
    let lines: Vec<&String> = world.lines.iter().chain(&missing.lines).collect();
    assert_eq!(lines.len(), 4, "{lines:?}");
    for line in lines {
        assert!(!line.contains(&token), "{line}");
        assert!(!line.contains("a line"), "{line}");
        assert!(!line.contains('/'), "{line}");
    }
}

// ============================================================================
// The reconnect ladder's mismatch as a verdict (LADDER-LINE)
// ============================================================================

/// `get_connection_info`'s refusal of a live daemon whose file states
/// another protocol, as the window labelled `window` asked it.
fn ladder(world: &mut FakeWorld, watch: &Mutex<StartingWatch>, window: &str, d: &Discovery) {
    let mismatch = protocol::file_mismatch(d, &THIS_SHELL).expect("a file of another protocol");
    ladder_mismatch(watch, window, &mismatch, |line| world.log(line));
}

/// The mismatch line `ensure_with` writes, for a daemon of protocol
/// `protocol` at version 9.9.9, pid 4242.
fn mismatch_line(protocol: u32) -> String {
    format!(
        "refusal (mismatch) — pid 4242, version 9.9.9, protocol {protocol}; this app speaks \
         protocol {}; nothing was stopped or started.",
        THIS_SHELL.protocol
    )
}

/// *Rev 114* (3) (b): the ladder's refusal is `ensure_with`'s mismatch by
/// another path, so it writes the same line, naming the pid, version and
/// protocol, once per change of the window's verdict, never per ask; each
/// window keeps its own.
#[test]
fn the_ladder_s_mismatch_logs_one_verdict_line_and_asked_again_none() {
    let newer = discovery_of("9.9.9", THIS_SHELL.protocol + 1);
    let mut world = FakeWorld::new(|_, _| look(DiscoveryRead::Missing, silent(), false));
    let watch = Mutex::new(StartingWatch::default());
    for _ in 0..3 {
        ladder(&mut world, &watch, WINDOW, &newer);
    }
    assert_eq!(
        verdict_lines(&world, WINDOW),
        [mismatch_line(THIS_SHELL.protocol + 1)]
    );
    ladder(&mut world, &watch, "second", &newer);
    ladder(&mut world, &watch, "second", &newer);
    assert_eq!(verdict_lines(&world, "second").len(), 1);
    assert_eq!(world.lines.len(), 2, "{:?}", world.lines);
    assert_eq!(world.looks, 0, "the ladder's line asks the world nothing");
}

/// A changed verdict is a line: the ladder's mismatch, then a reuse
/// `ensure_with` answered, then the ladder's mismatch again is three.
#[test]
fn a_changed_verdict_around_the_ladder_s_mismatch_logs_again() {
    let newer = discovery_of("9.9.9", THIS_SHELL.protocol + 1);
    let ours = this_build_discovery(5151);
    let script_d = ours.clone();
    let mut world = FakeWorld::new(move |_, _| healthy(&script_d));
    let watch = Mutex::new(StartingWatch::default());
    ladder(&mut world, &watch, WINDOW, &newer);
    assert_eq!(kind(&ask(&mut world, &watch)), "reused");
    ladder(&mut world, &watch, WINDOW, &newer);
    ladder(&mut world, &watch, WINDOW, &newer);
    let lines = verdict_lines(&world, WINDOW);
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[0], mismatch_line(THIS_SHELL.protocol + 1));
    assert!(lines[1].starts_with("reuse — pid 5151, "), "{}", lines[1]);
    assert_eq!(lines[2], lines[0]);
}

/// The ladder and `ensure_with` keep one record per window: a mismatch
/// `ensure_with` logged is not logged again by the ladder, and the ladder's
/// is not logged again by `ensure_with`.
#[test]
fn the_ladder_and_ensure_share_the_window_s_record() {
    let protocol = THIS_SHELL.protocol + 1;
    let newer = discovery_of("9.9.9", protocol);
    let script_d = newer.clone();
    let mut world = FakeWorld::new(move |_, _| {
        look(
            parsed(&script_d),
            answered(INSTANCE, Some(protocol), "9.9.9"),
            true,
        )
    });
    let watch = Mutex::new(StartingWatch::default());
    assert_eq!(kind(&ask(&mut world, &watch)), "mismatch");
    ladder(&mut world, &watch, WINDOW, &newer);
    assert_eq!(verdict_lines(&world, WINDOW), [mismatch_line(protocol)]);

    let mut world = FakeWorld::new(move |_, _| {
        look(
            parsed(&newer),
            answered(INSTANCE, Some(protocol), "9.9.9"),
            true,
        )
    });
    let watch = Mutex::new(StartingWatch::default());
    let newer = discovery_of("9.9.9", protocol);
    ladder(&mut world, &watch, WINDOW, &newer);
    assert_eq!(kind(&ask(&mut world, &watch)), "mismatch");
    assert_eq!(verdict_lines(&world, WINDOW), [mismatch_line(protocol)]);
}

/// The ladder's line leaves the spawn's wait and the starting clock as they
/// were: only `ensure_with`'s answers end or extend them.
#[test]
fn the_ladder_s_line_leaves_the_spawn_s_wait_alone() {
    let spawned = this_build_discovery(9090);
    let script_d = spawned.clone();
    let mut world = FakeWorld::new(move |_, spawns| {
        if spawns == 0 {
            look(DiscoveryRead::Missing, silent(), false)
        } else {
            booting(&script_d)
        }
    });
    let watch = Mutex::new(StartingWatch::default());
    assert_eq!(kind(&ask(&mut world, &watch)), "starting");
    let (spawn, seen) = {
        let w = watch.lock().unwrap();
        (w.spawn.map(|p| (p.pid, p.at, p.asked)), w.seen.clone())
    };
    assert!(spawn.is_some() && seen.is_some());
    world.pass(Duration::from_secs(3));
    ladder(
        &mut world,
        &watch,
        WINDOW,
        &discovery_of("9.9.9", THIS_SHELL.protocol + 1),
    );
    let w = watch.lock().unwrap();
    assert_eq!(w.spawn.map(|p| (p.pid, p.at, p.asked)), spawn);
    assert_eq!(w.seen, seen);
}

/// The ladder's line carries no token, no log line and no path.
#[test]
fn the_ladder_s_line_carries_no_token_or_path() {
    let newer = discovery_of("9.9.9", THIS_SHELL.protocol + 1);
    let token = newer.auth.token.clone();
    assert!(!token.is_empty());
    let mut world = FakeWorld::new(|_, _| look(DiscoveryRead::Missing, silent(), false));
    ladder(
        &mut world,
        &Mutex::new(StartingWatch::default()),
        WINDOW,
        &newer,
    );
    assert_eq!(world.lines.len(), 1);
    assert!(!world.lines[0].contains(&token), "{}", world.lines[0]);
    assert!(!world.lines[0].contains('/'), "{}", world.lines[0]);
}
