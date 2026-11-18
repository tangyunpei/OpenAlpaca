//! The CLI's daemon check (CLI-PROTOCOL) against an in-test loopback stub
//! answering `GET /v1/health` — each of `decide_reuse`'s outcomes as the
//! CLI says it — and the wait after `openalpaca daemon start`, under the
//! CLI's `EnvSandbox`: the discovery file is the sandbox's, the stub is
//! this test's, the process table is injected (a live `openalpacad` cannot
//! be one of a test's own), and the clock is the test's, so no test waits
//! the budget out. No real daemon runs.

use super::*;
use crate::client::DaemonClient;
use crate::test_util::{
    EnvSandbox, bearer_of, discovery_of_a_daemon_up_for, json_answer, ok_json, serve_until_idle,
};
use std::collections::VecDeque;

/// How long a stub waits for a connection it was given an answer for.
const IDLE: Duration = Duration::from_secs(3);

/// The instance `discovery_of_a_daemon_up_for` writes.
const INSTANCE: &str = "instance-t21";

/// A world over the sandboxed file and the stub, with the process table's
/// answer, the clock and the lines the test's own. A canned stub's looks
/// are the only ones it gives, in order: one look more than given panics,
/// never falls through to the file.
struct Stub {
    pid_live: bool,
    canned: Option<VecDeque<Look>>,
    at: Instant,
    started: Instant,
    said: Vec<String>,
    notes: Vec<String>,
    looks: usize,
}

impl Stub {
    fn new(pid_live: bool) -> Self {
        let at = Instant::now();
        Self {
            pid_live,
            canned: None,
            at,
            started: at,
            said: Vec::new(),
            notes: Vec::new(),
            looks: 0,
        }
    }

    fn canned(looks: impl IntoIterator<Item = Look>) -> Self {
        let mut stub = Self::new(false);
        stub.canned = Some(looks.into_iter().collect());
        stub
    }

    fn waited(&self) -> Duration {
        self.at - self.started
    }
}

impl World for Stub {
    async fn look(&mut self) -> Look {
        self.looks += 1;
        if let Some(canned) = &mut self.canned {
            return canned
                .pop_front()
                .unwrap_or_else(|| panic!("look {} is one more than canned", self.looks));
        }
        let live = self.pid_live;
        look_with(move |_| live).await
    }

    fn now(&self) -> Instant {
        self.at
    }

    async fn sleep(&mut self, by: Duration) {
        self.at += by;
    }

    fn say(&mut self, line: String) {
        self.said.push(line);
    }

    fn note(&mut self, line: String) {
        self.notes.push(line);
    }
}

/// `GET /v1/health`'s answer from a daemon of `protocol` at `version`.
fn health(protocol: u32, version: &str) -> String {
    ok_json(
        &serde_json::json!({
            "status": "ok",
            "version": version,
            "pid": 51823,
            "instance_id": INSTANCE,
            "protocol": protocol,
        })
        .to_string(),
    )
}

/// What a booting daemon's socket answers before it serves.
fn not_yet() -> String {
    json_answer("404 Not Found", "x", "{}")
}

/// A client of protocol `protocol` that reads back to `oldest`, at this
/// CLI's version.
fn cli(protocol: u32, oldest: u32) -> ClientBuild {
    ClientBuild {
        protocol,
        oldest_supported: oldest,
        version: THIS_CLI.version,
    }
}

/// The plain reuse: a daemon of this CLI's protocol and version is used,
/// with no note, carrying the file's token. (Passes on the parent too,
/// which used every file it could parse.)
#[tokio::test]
async fn a_daemon_of_this_protocol_and_version_is_used_with_no_note() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let (addr, server) = serve_until_idle(vec![health(1, THIS_CLI.version)], IDLE);
    let d = discovery_of_a_daemon_up_for(addr.port(), 0);
    let mut world = Stub::new(false);

    let client = DaemonClient::connect_with(&mut world, &cli(1, 1))
        .await
        .expect("used");

    assert_eq!(client.token(), d.auth.token);
    assert!(world.notes.is_empty() && world.said.is_empty());
    drop(server);
}

/// The same protocol at another version: used, with the note naming both
/// versions, and the requests that follow carry the file's token.
#[tokio::test]
async fn another_version_of_this_protocol_is_used_with_a_note_naming_both() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let (addr, server) = serve_until_idle(vec![health(1, "9.9.9"), ok_json("{}")], IDLE);
    let d = discovery_of_a_daemon_up_for(addr.port(), 0);
    let mut world = Stub::new(false);

    let client = DaemonClient::connect_with(&mut world, &cli(1, 1))
        .await
        .expect("used");
    let _: serde_json::Value = client.get("/v1/status").await.expect("answered");

    assert_eq!(world.notes.len(), 1, "{:?}", world.notes);
    let note = &world.notes[0];
    assert!(
        note.contains("version 9.9.9")
            && note.contains(&format!("this CLI is {}", THIS_CLI.version))
            && note.contains("the same protocol (1)")
            && note.contains("`openalpaca daemon restart`"),
        "{note}"
    );
    let requests = server.join().expect("the stub finishes");
    assert!(requests[0].starts_with("GET /v1/health "), "{requests:?}");
    assert!(requests[1].starts_with("GET /v1/status "), "{requests:?}");
    assert_eq!(
        bearer_of(&requests[1]).as_deref(),
        Some(d.auth.token.as_str())
    );
    assert_eq!(
        bearer_of(&requests[0]),
        None,
        "health is asked without the token"
    );
}

/// A protocol older than the CLI's that it still reads: used, with the
/// note naming both protocols.
#[tokio::test]
async fn an_older_protocol_this_cli_still_reads_is_used_with_a_note() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let (addr, server) = serve_until_idle(vec![health(1, THIS_CLI.version)], IDLE);
    discovery_of_a_daemon_up_for(addr.port(), 0);
    let mut world = Stub::new(false);

    DaemonClient::connect_with(&mut world, &cli(2, 1))
        .await
        .expect("used");

    assert_eq!(world.notes.len(), 1, "{:?}", world.notes);
    assert!(
        world.notes[0]
            .contains("speaks protocol 1, older than this CLI's 2; the CLI works with it"),
        "{}",
        world.notes[0]
    );
    drop(server);
}

/// `PROTOCOL + 1`: refused with the sentence naming both protocols and
/// the way out; nothing is stopped or started.
#[tokio::test]
async fn a_newer_protocol_is_refused_naming_both_and_the_way_out() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let (addr, server) = serve_until_idle(vec![health(2, "0.2.0")], IDLE);
    let d = discovery_of_a_daemon_up_for(addr.port(), 0);
    let mut world = Stub::new(true);

    let refused = DaemonClient::connect_with(&mut world, &cli(1, 1))
        .await
        .err()
        .expect("refused")
        .to_string();

    assert_eq!(
        refused,
        format!(
            "The daemon running (pid {}, version 0.2.0, protocol 2) is not one this CLI can use: \
             the CLI speaks protocol 1. Nothing was stopped or started. Update the CLI to one \
             that speaks protocol 2, or stop that daemon with `openalpaca daemon stop` and start \
             this CLI's own with `openalpaca daemon start`.",
            d.pid
        )
    );
    assert_eq!(world.looks, 1, "one look; no wait");
    drop(server);
}

/// A daemon older than the oldest protocol the CLI reads: refused, the
/// CLI's range named, the way out the stop and this CLI's start.
#[tokio::test]
async fn a_daemon_older_than_the_cli_reads_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let (addr, server) = serve_until_idle(vec![health(1, "0.0.9")], IDLE);
    discovery_of_a_daemon_up_for(addr.port(), 0);
    let mut world = Stub::new(true);

    let refused = DaemonClient::connect_with(&mut world, &cli(3, 2))
        .await
        .err()
        .expect("refused")
        .to_string();

    assert!(
        refused.contains("the CLI speaks protocol 3, and reads back to protocol 2.")
            && refused.ends_with(
                "Nothing was stopped or started. Stop that daemon with `openalpaca daemon stop` \
                 and start this CLI's own with `openalpaca daemon start`."
            ),
        "{refused}"
    );
    drop(server);
}

/// A daemon starting — its file written, its pid a live `openalpacad`, its
/// health not answering yet — is waited for, said once, then used.
#[tokio::test]
async fn a_starting_daemon_is_waited_for_then_used() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let (addr, server) = serve_until_idle(
        vec![
            not_yet(),
            not_yet(),
            health(1, THIS_CLI.version),
            ok_json(r#"{"answered":true}"#),
        ],
        IDLE,
    );
    let d = discovery_of_a_daemon_up_for(addr.port(), 0);
    let mut world = Stub::new(true);

    let client = DaemonClient::connect_with(&mut world, &cli(1, 1))
        .await
        .expect("used once it answers");
    let body: serde_json::Value = client.get("/v1/status").await.expect("answered");

    assert_eq!(body["answered"], true);
    assert_eq!(world.looks, 3);
    assert_eq!(world.said.len(), 1, "{:?}", world.said);
    assert_eq!(
        world.said[0],
        format!(
            "The daemon (pid {}) is starting: it has written its discovery file but does not \
             answer yet. Waiting for it, up to 45 seconds.",
            d.pid
        )
    );
    assert!(world.waited() < STARTING_BUDGET);
    server.join().expect("the stub finishes");
}

/// A starting daemon's look: its file, no answer, its pid live.
fn starting_look(d: &Discovery) -> Look {
    Look {
        discovery: DiscoveryRead::Parsed(d.clone()),
        health: Health::Unanswered("HTTP 404".into()),
        pid_live: true,
    }
}

/// Silent for the whole budget from the first silent look: reported still
/// starting, with the way out, after exactly the budget; nothing spawned.
#[tokio::test]
async fn a_daemon_silent_past_the_budget_is_reported_still_starting() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let d = discovery_of_a_daemon_up_for(1, 0);
    let looks = (STARTING_BUDGET.as_millis() / POLL.as_millis()) as usize + 1;
    let mut world = Stub::canned(std::iter::repeat_n(starting_look(&d), looks));

    let refused = DaemonClient::connect_with(&mut world, &cli(1, 1))
        .await
        .err()
        .expect("reported")
        .to_string();

    assert_eq!(
        refused,
        format!(
            "The daemon (pid {}) is still starting after 45 seconds: it is running but does not \
             answer. Nothing was started, stopped or restarted. If it does not finish starting, \
             stop it with `openalpaca daemon stop` and run `openalpaca daemon start`.",
            d.pid
        )
    );
    assert_eq!(world.waited(), STARTING_BUDGET);
    assert_eq!(
        world.said.len(),
        1,
        "the starting line once: {:?}",
        world.said
    );
}

/// A discovery file another build wrote, over a live daemon: refused with
/// the sentence a file this CLI cannot read gets, whose way out is not
/// this CLI's `daemon stop` (which cannot read the file either).
#[tokio::test]
async fn a_file_another_build_wrote_over_a_live_daemon_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let path = paths::discovery_path().unwrap();
    assert!(path.starts_with(tmp.path()), "{}", path.display());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        br#"{"pid": 777, "schema": 9, "build": {"version": "5.0.0", "protocol": 7}}"#,
    )
    .unwrap();
    let mut world = Stub::new(true);

    let refused = DaemonClient::connect_with(&mut world, &cli(1, 1))
        .await
        .err()
        .expect("refused")
        .to_string();

    assert_eq!(
        refused,
        "The daemon running (pid 777, version 5.0.0, protocol 7) wrote a discovery file this \
         CLI cannot read: another build of OpenAlpaca. Nothing was stopped or started. Update \
         the CLI to one that speaks protocol 7, or stop that daemon with the `openalpaca` of \
         the build that started it (or `kill 777`), then start this CLI's own with \
         `openalpaca daemon start`."
    );
}

/// Another build's file stating this CLI's own protocol (its schema the
/// difference): the way out is that build's stop, never an update of the
/// CLI to the protocol it already speaks.
#[tokio::test]
async fn another_builds_file_at_this_protocol_offers_no_update() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let path = paths::discovery_path().unwrap();
    assert!(path.starts_with(tmp.path()), "{}", path.display());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        br#"{"pid": 777, "schema": 9, "build": {"version": "0.1.1", "protocol": 1}}"#,
    )
    .unwrap();
    let mut world = Stub::new(true);

    let refused = DaemonClient::connect_with(&mut world, &cli(1, 1))
        .await
        .err()
        .expect("refused")
        .to_string();

    assert_eq!(
        refused,
        "The daemon running (pid 777, version 0.1.1, protocol 1) wrote a discovery file this \
         CLI cannot read: another build of OpenAlpaca. Nothing was stopped or started. Stop \
         that daemon with the `openalpaca` of the build that started it (or `kill 777`), then \
         start this CLI's own with `openalpaca daemon start`."
    );
}

/// A file whose pid is no daemon and whose address answers nothing: not
/// running, said so, never a request sent to the address.
#[tokio::test]
async fn a_stale_file_is_not_running() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let d = discovery_of_a_daemon_up_for(1, 0);
    let mut world = Stub::new(false);

    let refused = DaemonClient::connect_with(&mut world, &cli(1, 1))
        .await
        .err()
        .expect("refused")
        .to_string();

    assert_eq!(
        refused,
        format!(
            "Daemon is not running (the discovery file names pid {}, which is not a running \
             openalpacad, and nothing at 127.0.0.1:1 answers as a daemon)",
            d.pid
        )
    );
}

/// A file whose pid is no daemon while its address answers as another
/// instance: not running, and the address is said to answer as another
/// daemon, never that nothing answers there.
#[tokio::test]
async fn a_stale_file_whose_address_another_daemon_answers_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let d = openalpaca_app_support::discovery::make_discovery("127.0.0.1", 1, "i".into(), "0.1.0");
    let mut world = Stub::canned([Look {
        discovery: DiscoveryRead::Parsed(d.clone()),
        health: Health::Answered {
            instance_id: "another".into(),
            protocol: Some(1),
            version: Some(THIS_CLI.version.to_owned()),
        },
        pid_live: false,
    }]);

    let refused = DaemonClient::connect_with(&mut world, &cli(1, 1))
        .await
        .err()
        .expect("refused")
        .to_string();

    assert_eq!(
        refused,
        format!(
            "Daemon is not running (the discovery file names pid {}, which is not a running \
             openalpacad; 127.0.0.1:1 answers as another daemon, instance another)",
            d.pid
        )
    );
}

// ============================================================================
// The wait after `openalpaca daemon start`
// ============================================================================

fn missing() -> Look {
    Look {
        discovery: DiscoveryRead::Missing,
        health: Health::Unanswered("no address to ask".into()),
        pid_live: false,
    }
}

fn answering(d: &Discovery, protocol: u32) -> Look {
    Look {
        discovery: DiscoveryRead::Parsed(d.clone()),
        health: Health::Answered {
            instance_id: d.instance_id.clone(),
            protocol: Some(protocol),
            version: Some(THIS_CLI.version.to_owned()),
        },
        pid_live: false,
    }
}

/// The wait replaces the two-second sleep: no file yet, then a daemon
/// starting, then serving — the app is started only after it answers.
#[tokio::test]
async fn the_wait_after_a_start_waits_for_the_file_then_the_answer() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let d = openalpaca_app_support::discovery::make_discovery("127.0.0.1", 1, "i".into(), "0.1.0");
    let mut world = Stub::canned([missing(), missing(), starting_look(&d), answering(&d, 1)]);

    let waited = wait_after_start(&mut world, &cli(1, 1), None).await;

    assert!(matches!(waited, StartWait::Ready), "{waited:?}");
    assert_eq!(world.looks, 4);
    assert_eq!(world.waited(), POLL * 3);
    assert_eq!(
        world.said.len(),
        1,
        "the starting line once: {:?}",
        world.said
    );
}

/// A daemon this CLI cannot use ends the wait at once, with its sentence.
#[tokio::test]
async fn the_wait_after_a_start_ends_on_a_mismatch_with_its_sentence() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let d = openalpaca_app_support::discovery::make_discovery("127.0.0.1", 1, "i".into(), "0.1.0");
    let mut world = Stub::canned([answering(&d, 2)]);

    let waited = wait_after_start(&mut world, &cli(1, 1), None).await;

    let StartWait::NotReady(why) = waited else {
        panic!("not refused: {waited:?}");
    };
    assert!(
        why.starts_with("The daemon running (pid ") && why.contains("protocol 2) is not one"),
        "{why}"
    );
    assert_eq!(world.looks, 1);
}

/// No daemon ever writes its file: the wait gives up after the budget from
/// its start, saying so.
#[tokio::test]
async fn the_wait_after_a_start_gives_up_on_no_daemon_after_the_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let looks = (STARTING_BUDGET.as_millis() / POLL.as_millis()) as usize + 1;
    let mut world = Stub::canned(std::iter::repeat_n(missing(), looks));

    let waited = wait_after_start(&mut world, &cli(1, 1), None).await;

    let StartWait::NotReady(why) = waited else {
        panic!("not reported: {waited:?}");
    };
    assert_eq!(
        why,
        "Daemon is not running (no discovery file) 45 seconds after the start."
    );
    assert_eq!(world.waited(), STARTING_BUDGET);
}

/// `openalpaca daemon start` opens the app only once the daemon answers:
/// the wait looks until it does, and the app is started after it, with
/// nothing said (the parent slept two seconds and looked at nothing).
#[tokio::test]
async fn daemon_start_opens_the_app_once_the_daemon_answers() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let d = openalpaca_app_support::discovery::make_discovery("127.0.0.1", 1, "i".into(), "0.1.0");
    let mut world = Stub::canned([missing(), starting_look(&d), answering(&d, 1)]);
    let mut started = false;

    crate::commands::daemon::start_gui_after_the_daemon(&mut world, &cli(1, 1), None, || {
        started = true;
        Ok(())
    })
    .await
    .expect("started");

    assert!(started);
    assert_eq!(world.looks, 3, "the app after the answering look");
    assert_eq!(world.waited(), POLL * 2);
    assert_eq!(
        world.said.len(),
        1,
        "the starting line only: {:?}",
        world.said
    );
}

/// A wait that ends on a daemon this CLI cannot use says why on stderr and
/// starts the app all the same, whose own check shows the verdict.
#[tokio::test]
async fn daemon_start_says_a_failed_wait_and_opens_the_app_all_the_same() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let d = openalpaca_app_support::discovery::make_discovery("127.0.0.1", 1, "i".into(), "0.1.0");
    let mut world = Stub::canned([answering(&d, 2)]);
    let mut started = false;

    crate::commands::daemon::start_gui_after_the_daemon(&mut world, &cli(1, 1), None, || {
        started = true;
        Ok(())
    })
    .await
    .expect("started");

    assert!(started);
    assert_eq!(world.said.len(), 1, "{:?}", world.said);
    let said = &world.said[0];
    assert!(
        said.starts_with("⚠️  The daemon running (pid ")
            && said.contains("protocol 2) is not one this CLI can use")
            && said.ends_with(" Starting the app all the same."),
        "{said}"
    );
}

// ============================================================================
// The process the start spawned
// ============================================================================

/// A spawned process the test owns: who it is, and its exit once given.
struct FakeSpawn {
    pid: u32,
    exit: Option<&'static str>,
    log_start: u64,
}

impl Spawned for FakeSpawn {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn who(&self) -> String {
        format!("The daemon this start spawned (pid {})", self.pid)
    }

    fn exited(&mut self) -> Option<String> {
        self.exit.map(str::to_owned)
    }

    fn log_start(&self) -> u64 {
        self.log_start
    }
}

/// What a daemon that died at boot leaves: its file, naming its pid, which
/// is no live `openalpacad`, at an address nothing answers.
fn left_by_a_dead_daemon(d: &Discovery) -> Look {
    Look {
        discovery: DiscoveryRead::Parsed(d.clone()),
        health: Health::Unanswered("connection refused".into()),
        pid_live: false,
    }
}

/// The sandbox's daemon log, holding an earlier run's line, and its length
/// (the spawn's offset).
fn a_log_with_an_earlier_run() -> (std::path::PathBuf, u64) {
    let logs = paths::logs_dir().expect("the scratch logs dir");
    let log = logs.join("daemon.log");
    std::fs::write(&log, "an earlier run's last line\n").unwrap();
    let start = std::fs::metadata(&log).unwrap().len();
    (log, start)
}

/// A daemon that wrote its file and died at boot (a pre-squash database
/// refused, SQ-1) ends the wait at its first look, with the lines this run
/// wrote to its log said above the sentence, never an earlier run's. (On
/// the parent the wait looked on for 45 s and said nothing.)
#[tokio::test]
async fn a_spawned_daemon_that_exits_at_boot_ends_the_wait_with_its_log() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let (log, start) = a_log_with_an_earlier_run();
    std::fs::write(
        &log,
        "an earlier run's last line\nError: this database was written by another build\n",
    )
    .unwrap();
    let d = openalpaca_app_support::discovery::make_discovery("127.0.0.1", 1, "i".into(), "0.1.0");
    let mut world = Stub::canned([left_by_a_dead_daemon(&d)]);
    let mut spawned = FakeSpawn {
        pid: 4242,
        exit: Some("exit status: 1"),
        log_start: start,
    };

    let waited = wait_after_start(&mut world, &cli(1, 1), Some(&mut spawned)).await;

    let StartWait::NotReady(why) = waited else {
        panic!("not reported: {waited:?}");
    };
    assert_eq!(
        why,
        format!(
            "The daemon this start spawned (pid 4242) exited (exit status: 1) before it \
             answered; the lines above are what it wrote to its log ({}).",
            log.display()
        )
    );
    assert_eq!(
        world.said,
        vec!["Error: this database was written by another build".to_owned()],
        "this run's lines only"
    );
    assert_eq!(world.looks, 1);
    assert_eq!(world.waited(), Duration::ZERO);
}

/// A daemon that exits having written nothing says so, naming its log.
#[tokio::test]
async fn a_spawned_daemon_that_exits_writing_nothing_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let (log, start) = a_log_with_an_earlier_run();
    let mut world = Stub::canned([missing()]);
    let mut spawned = FakeSpawn {
        pid: 4242,
        exit: Some("signal: 9 (SIGKILL)"),
        log_start: start,
    };

    let waited = wait_after_start(&mut world, &cli(1, 1), Some(&mut spawned)).await;

    let StartWait::NotReady(why) = waited else {
        panic!("not reported: {waited:?}");
    };
    assert_eq!(
        why,
        format!(
            "The daemon this start spawned (pid 4242) exited (signal: 9 (SIGKILL)) before it \
             answered, and wrote nothing to its log ({}).",
            log.display()
        )
    );
    assert!(world.said.is_empty(), "{:?}", world.said);
}

/// A spawned daemon still running is waited for as before: no file, then
/// serving.
#[tokio::test]
async fn a_spawned_daemon_still_running_is_waited_for() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let d = openalpaca_app_support::discovery::make_discovery("127.0.0.1", 1, "i".into(), "0.1.0");
    let mut world = Stub::canned([missing(), missing(), answering(&d, 1)]);
    let mut spawned = FakeSpawn {
        pid: 4242,
        exit: None,
        log_start: 0,
    };

    let waited = wait_after_start(&mut world, &cli(1, 1), Some(&mut spawned)).await;

    assert!(matches!(waited, StartWait::Ready), "{waited:?}");
    assert_eq!(world.looks, 3);
}

/// The start's own child, read through `manager::SpawnedDaemon`: a process
/// that writes to the sandbox's log and exits is seen exited, with its
/// status, and only what it wrote after the offset is said.
#[tokio::test]
async fn the_started_child_is_read_for_its_exit_and_its_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let (log, start) = a_log_with_an_earlier_run();
    let out = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    let child = std::process::Command::new("/bin/sh")
        .args(["-c", "echo refused at boot >&2; exit 3"])
        .stdin(std::process::Stdio::null())
        .stdout(out.try_clone().unwrap())
        .stderr(out)
        .spawn()
        .expect("sh runs");
    let pid = child.id();
    let mut spawned = crate::manager::SpawnedDaemon::new(child, false, start);
    let deadline = Instant::now() + Duration::from_secs(10);
    while spawned.exited().is_none() {
        assert!(Instant::now() < deadline, "sh did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut world = Stub::canned([missing()]);

    let waited = wait_after_start(&mut world, &cli(1, 1), Some(&mut spawned)).await;

    let StartWait::NotReady(why) = waited else {
        panic!("not reported: {waited:?}");
    };
    assert_eq!(
        why,
        format!(
            "The daemon this start spawned (pid {pid}) exited (exit status: 3) before it \
             answered; the lines above are what it wrote to its log ({}).",
            log.display()
        )
    );
    assert_eq!(world.said, vec!["refused at boot".to_owned()]);
}

/// The child exited but still in the process table as the file's live pid
/// (unreaped): its exit ends the wait all the same; a daemon starting
/// under another pid is still waited for.
#[tokio::test]
async fn an_exited_child_still_shown_as_starting_ends_the_wait() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let (log, start) = a_log_with_an_earlier_run();
    let d = openalpaca_app_support::discovery::make_discovery("127.0.0.1", 1, "i".into(), "0.1.0");
    let mut world = Stub::canned([starting_look(&d), starting_look(&d)]);
    let mut other = FakeSpawn {
        pid: d.pid + 1,
        exit: Some("exit status: 1"),
        log_start: start,
    };
    let mut world_other = Stub::canned([starting_look(&d), answering(&d, 1)]);

    let waited = wait_after_start(&mut world_other, &cli(1, 1), Some(&mut other)).await;
    assert!(matches!(waited, StartWait::Ready), "{waited:?}");

    let mut spawned = FakeSpawn {
        pid: d.pid,
        exit: Some("exit status: 1"),
        log_start: start,
    };
    let waited = wait_after_start(&mut world, &cli(1, 1), Some(&mut spawned)).await;

    let StartWait::NotReady(why) = waited else {
        panic!("not reported: {waited:?}");
    };
    assert_eq!(
        why,
        format!(
            "The daemon this start spawned (pid {}) exited (exit status: 1) before it \
             answered, and wrote nothing to its log ({}).",
            d.pid,
            log.display()
        )
    );
    assert_eq!(world.looks, 1);
}

// ============================================================================
// `daemon start` judges before it spawns (123-8), `--daemon-only` and
// `restart` wait (123-2)
// ============================================================================

/// Another build's discovery file over its live daemon (pid 777, protocol
/// 7), written into the sandbox, and the refusal every command says of it.
fn another_builds_file(sandbox: &std::path::Path) -> &'static str {
    let path = paths::discovery_path().unwrap();
    assert!(path.starts_with(sandbox), "{}", path.display());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        br#"{"pid": 777, "schema": 9, "build": {"version": "5.0.0", "protocol": 7}}"#,
    )
    .unwrap();
    "The daemon running (pid 777, version 5.0.0, protocol 7) wrote a discovery file this CLI \
     cannot read: another build of OpenAlpaca. Nothing was stopped or started. Update the CLI \
     to one that speaks protocol 7, or stop that daemon with the `openalpaca` of the build that \
     started it (or `kill 777`), then start this CLI's own with `openalpaca daemon start`."
}

/// What a start closure did, for the test to read after.
#[derive(Default)]
struct Calls {
    start: bool,
    app: bool,
}

/// `openalpaca daemon start` over another build's live daemon refuses at
/// its first look, with the sentence — now true — that nothing was stopped
/// or started: the start (which rotates the daemon's log, then spawns) is
/// never called and no log is written. With `--daemon-only` the refusal is
/// the command's error; with the app it is said on stderr and the app is
/// started all the same, as a wait that ends on such a daemon has it (Rev
/// 123's first item). (On the parent the start rotated that daemon's log
/// and spawned beside it.)
#[tokio::test]
async fn daemon_start_refuses_another_builds_daemon_before_it_rotates_or_spawns() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let sentence = another_builds_file(tmp.path());
    let log = paths::daemon_log_path().unwrap();
    assert!(log.starts_with(tmp.path()), "{}", log.display());
    for daemon_only in [false, true] {
        let mut world = Stub::new(true);
        let mut calls = Calls::default();

        let outcome = crate::commands::daemon::start_with(
            &mut world,
            &cli(1, 1),
            || -> Result<Option<FakeSpawn>> {
                calls.start = true;
                Ok(None)
            },
            (!daemon_only).then_some(|| {
                calls.app = true;
                Ok(())
            }),
        )
        .await;

        if daemon_only {
            assert_eq!(outcome.expect_err("refused").to_string(), sentence);
            assert!(!calls.app, "no app");
            assert!(world.said.is_empty(), "{:?}", world.said);
        } else {
            outcome.expect("the app is started all the same");
            assert!(calls.app, "the app is started all the same");
            assert_eq!(
                world.said,
                vec![format!("⚠️  {sentence} Starting the app all the same.")]
            );
        }
        assert!(
            !calls.start,
            "nothing rotated or spawned, daemon_only {daemon_only}"
        );
        assert_eq!(world.looks, 1, "one look; no wait");
    }
    assert!(!log.exists(), "no daemon log was rotated or opened");
}

/// The same over a daemon whose file this build reads and whose health
/// states a protocol this CLI does not speak.
#[tokio::test]
async fn daemon_start_refuses_a_daemon_of_another_protocol_before_it_spawns() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let d = openalpaca_app_support::discovery::make_discovery("127.0.0.1", 1, "i".into(), "0.1.0");
    let mut world = Stub::canned([answering(&d, 2)]);
    let mut calls = Calls::default();

    let refused = crate::commands::daemon::start_with(
        &mut world,
        &cli(1, 1),
        || -> Result<Option<FakeSpawn>> {
            calls.start = true;
            Ok(None)
        },
        None::<fn() -> Result<()>>,
    )
    .await
    .expect_err("refused")
    .to_string();

    assert!(
        refused.starts_with(&format!("The daemon running (pid {}, ", d.pid))
            && refused.contains("protocol 2) is not one this CLI can use")
            && refused.contains("Nothing was stopped or started."),
        "{refused}"
    );
    assert!(!calls.start);
}

/// No daemon: the start is called, and `--daemon-only` waits for the
/// daemon it spawned to answer, then succeeds.
#[tokio::test]
async fn daemon_start_daemon_only_spawns_and_waits_for_the_answer() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let d = openalpaca_app_support::discovery::make_discovery("127.0.0.1", 1, "i".into(), "0.1.0");
    let mut world = Stub::canned([missing(), missing(), starting_look(&d), answering(&d, 1)]);
    let mut calls = Calls::default();

    crate::commands::daemon::start_with(
        &mut world,
        &cli(1, 1),
        || {
            calls.start = true;
            Ok(Some(FakeSpawn {
                pid: d.pid,
                exit: None,
                log_start: 0,
            }))
        },
        None::<fn() -> Result<()>>,
    )
    .await
    .expect("the daemon answers");

    assert!(calls.start);
    assert_eq!(
        world.looks, 4,
        "the first look, then the wait to the answer"
    );
}

/// `daemon start --daemon-only` over a daemon that dies at boot (a
/// pre-squash database refused, SQ-1): the lines this run wrote to its log
/// said, and the command fails with the wait's sentence. (On the parent it
/// returned once the process was spawned, reporting it started.)
#[tokio::test]
async fn daemon_start_daemon_only_fails_on_a_daemon_that_dies_at_boot() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let (log, start) = a_log_with_an_earlier_run();
    std::fs::write(
        &log,
        "an earlier run's last line\nError: this database was written by another build\n",
    )
    .unwrap();
    let d = openalpaca_app_support::discovery::make_discovery("127.0.0.1", 1, "i".into(), "0.1.0");
    let mut world = Stub::canned([missing(), left_by_a_dead_daemon(&d)]);

    let failed = crate::commands::daemon::start_with(
        &mut world,
        &cli(1, 1),
        || {
            Ok(Some(FakeSpawn {
                pid: 4242,
                exit: Some("exit status: 1"),
                log_start: start,
            }))
        },
        None::<fn() -> Result<()>>,
    )
    .await
    .expect_err("a daemon that died at boot fails the command")
    .to_string();

    assert_eq!(
        failed,
        format!(
            "The daemon this start spawned (pid 4242) exited (exit status: 1) before it \
             answered; the lines above are what it wrote to its log ({}).",
            log.display()
        )
    );
    assert_eq!(
        world.said,
        vec!["Error: this database was written by another build".to_owned()]
    );
}

/// `--daemon-only` with no daemon ever answering: the command fails after
/// the shared budget (`STARTING_BUDGET`, 45 s) from the wait's start.
#[tokio::test]
async fn daemon_start_daemon_only_fails_when_no_daemon_answers_within_the_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let looks = (STARTING_BUDGET.as_millis() / POLL.as_millis()) as usize + 2;
    let mut world = Stub::canned(std::iter::repeat_n(missing(), looks));

    let failed = crate::commands::daemon::start_with(
        &mut world,
        &cli(1, 1),
        || {
            Ok(Some(FakeSpawn {
                pid: 4242,
                exit: None,
                log_start: 0,
            }))
        },
        None::<fn() -> Result<()>>,
    )
    .await
    .expect_err("reported")
    .to_string();

    assert_eq!(
        failed,
        "Daemon is not running (no discovery file) 45 seconds after the start."
    );
    assert_eq!(world.waited(), STARTING_BUDGET);
}

/// `daemon restart`'s start waits the same way (`restart_start_with`, what
/// the restart arm calls once the old daemon is gone): the daemon it
/// spawned dying at boot fails the command; one that answers succeeds.
#[tokio::test]
async fn the_restarts_start_fails_on_a_daemon_that_dies_and_passes_one_that_answers() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let (log, start) = a_log_with_an_earlier_run();
    let d = openalpaca_app_support::discovery::make_discovery("127.0.0.1", 1, "i".into(), "0.1.0");
    let mut world = Stub::canned([left_by_a_dead_daemon(&d)]);

    let failed = crate::commands::daemon::restart_start_with(&mut world, &cli(1, 1), || {
        Ok(Some(FakeSpawn {
            pid: 4242,
            exit: Some("exit status: 1"),
            log_start: start,
        }))
    })
    .await
    .expect_err("failed")
    .to_string();
    assert_eq!(
        failed,
        format!(
            "The daemon this start spawned (pid 4242) exited (exit status: 1) before it \
             answered, and wrote nothing to its log ({}).",
            log.display()
        )
    );

    let mut world = Stub::canned([missing(), answering(&d, 1)]);
    crate::commands::daemon::restart_start_with(&mut world, &cli(1, 1), || {
        Ok(Some(FakeSpawn {
            pid: d.pid,
            exit: None,
            log_start: 0,
        }))
    })
    .await
    .expect("answered");
}

// ============================================================================
// `daemon tail`, `confirmations watch` and `status` run the same check
// (123-3)
// ============================================================================

/// What a stub that is no WebSocket server answers the upgrade with.
fn no_upgrade() -> String {
    json_answer("404 Not Found", "x", "{}")
}

/// `daemon tail` and `confirmations watch` over another build's live
/// daemon: refused with the shared sentence before any socket is opened.
/// (On the parent each failed to parse the file.)
#[tokio::test]
async fn the_socket_readers_refuse_another_builds_daemon() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let sentence = another_builds_file(tmp.path());

    let mut world = Stub::new(true);
    let tail = crate::commands::tail::run_with(&mut world, &cli(1, 1), 1)
        .await
        .expect_err("tail refused")
        .to_string();
    assert_eq!(tail, sentence);

    let mut world = Stub::new(true);
    let watch = crate::commands::confirmations::watch_with(&mut world, &cli(1, 1))
        .await
        .expect_err("watch refused")
        .to_string();
    assert_eq!(watch, sentence);
}

/// The same over a daemon whose file this build reads and whose health
/// states a newer protocol: refused with the sentence, the health asked and
/// the events socket never. (On the parent each opened the socket.)
#[tokio::test]
async fn the_socket_readers_refuse_a_daemon_of_another_protocol() {
    for reader in ["tail", "watch"] {
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvSandbox::enter(tmp.path());
        let (addr, server) = serve_until_idle(vec![health(2, "0.2.0")], IDLE);
        let d = discovery_of_a_daemon_up_for(addr.port(), 0);
        let mut world = Stub::new(true);

        let refused = match reader {
            "tail" => crate::commands::tail::run_with(&mut world, &cli(1, 1), 1).await,
            _ => crate::commands::confirmations::watch_with(&mut world, &cli(1, 1)).await,
        }
        .expect_err("refused")
        .to_string();

        assert!(
            refused.starts_with(&format!(
                "The daemon running (pid {}, version 0.2.0, protocol 2) is not one this CLI can \
                 use",
                d.pid
            )),
            "{reader}: {refused}"
        );
        let requests = server.join().expect("the stub finishes");
        assert_eq!(requests.len(), 1, "{reader}: {requests:?}");
        assert!(requests[0].starts_with("GET /v1/health "), "{requests:?}");
    }
}

/// A daemon of this CLI's protocol: each reader goes on, as before, to open
/// the events socket with the file's token (the stub, no WebSocket server,
/// then refuses the upgrade).
#[tokio::test]
async fn the_socket_readers_open_the_socket_of_a_daemon_this_cli_uses() {
    for reader in ["tail", "watch"] {
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvSandbox::enter(tmp.path());
        let (addr, server) =
            serve_until_idle(vec![health(1, THIS_CLI.version), no_upgrade()], IDLE);
        let d = discovery_of_a_daemon_up_for(addr.port(), 0);
        let mut world = Stub::new(false);

        let failed = match reader {
            "tail" => crate::commands::tail::run_with(&mut world, &cli(1, 1), 1).await,
            _ => crate::commands::confirmations::watch_with(&mut world, &cli(1, 1)).await,
        };

        assert!(failed.is_err(), "{reader}: the stub refuses the upgrade");
        let requests = server.join().expect("the stub finishes");
        assert_eq!(requests.len(), 2, "{reader}: {requests:?}");
        assert!(requests[0].starts_with("GET /v1/health "), "{requests:?}");
        let token = urlencoding::encode(&d.auth.token).into_owned();
        assert!(
            requests[1].starts_with(&format!("GET /v1/events?token={token} ")),
            "{reader}: {requests:?}"
        );
        assert!(world.notes.is_empty() && world.said.is_empty());
    }
}

/// `status` over another build's live daemon and over one of another
/// protocol prints the shared sentence where it printed health; over
/// another build's file whose process is gone, not running, saying why.
#[tokio::test]
async fn status_reports_a_daemon_this_cli_cannot_use_with_the_sentence() {
    use crate::commands::status::{Found, look_with};
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let sentence = another_builds_file(tmp.path());

    match look_with(&mut Stub::new(true), &cli(1, 1)).await.unwrap() {
        Found::Mismatch(said) => assert_eq!(said, sentence),
        other => panic!("not refused: {other:?}"),
    }
    match look_with(&mut Stub::new(false), &cli(1, 1)).await.unwrap() {
        Found::NotRunning(why) => assert_eq!(
            why,
            "Daemon is not running (the discovery file names pid 777, which is not a running \
             openalpacad)"
        ),
        other => panic!("not reported as not running: {other:?}"),
    }

    let (addr, server) = serve_until_idle(vec![health(2, "0.2.0")], IDLE);
    let d = discovery_of_a_daemon_up_for(addr.port(), 0);
    match look_with(&mut Stub::new(true), &cli(1, 1)).await.unwrap() {
        Found::Mismatch(said) => assert!(
            said.starts_with(&format!(
                "The daemon running (pid {}, version 0.2.0, protocol 2) is not one this CLI \
                 can use",
                d.pid
            )),
            "{said}"
        ),
        other => panic!("not refused: {other:?}"),
    }
    assert_eq!(server.join().unwrap().len(), 1, "health asked once");
}

/// T21's second case: `openalpaca status` against a daemon up for more
/// than a day — its `discovery.json` rotated on the daemon's schedule —
/// finds it running, where the file as the start wrote it was refused
/// with "Discovery token has expired" after 24 h. Its health is asked
/// twice: by the look (123-3), then for what `status` prints.
#[tokio::test]
async fn status_finds_a_daemon_up_for_more_than_a_day_running() {
    use crate::commands::status::{Found, look_with};
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let health = r#"{"status":"ok","version":"0.1.0","pid":51823,"instance_id":"instance-t21"}"#;
    let (addr, server) = serve_until_idle(vec![ok_json(health), ok_json(health)], IDLE);
    let d = discovery_of_a_daemon_up_for(addr.port(), 25);
    assert!(d.auth.issued_at > d.started_at, "the file was rotated");

    match look_with(&mut Stub::new(false), &cli(1, 1))
        .await
        .expect("status looks")
    {
        Found::Running { health, info } => {
            assert_eq!(health["status"], "ok");
            assert_eq!(info.token, d.auth.token);
        }
        other => panic!("not running: {other:?}"),
    }
    assert_eq!(server.join().expect("health was answered").len(), 2);
}

/// The client's check is kept: a file whose `expires_at` has passed —
/// what a daemon wrote before T21, a day after its start — is still
/// refused, before the health `status` prints is asked.
#[tokio::test]
async fn status_still_refuses_an_expired_file() {
    use crate::commands::status::{Found, look_with};
    use openalpaca_app_support::discovery;
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let started = chrono::Utc::now() - chrono::TimeDelta::hours(25);
    let d = discovery::make_discovery("127.0.0.1", 1, "instance-t21".into(), "0.1.0")
        .with_rotated_auth(started);
    discovery::write_discovery_atomic(&d).unwrap();

    match look_with(&mut Stub::new(false), &cli(1, 1))
        .await
        .expect("status looks")
    {
        Found::Expired(reason) => assert_eq!(reason, "Discovery token has expired"),
        other => panic!("not refused as expired: {other:?}"),
    }
}
