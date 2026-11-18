//! Whether the daemon `discovery.json` names is one this CLI uses, and the
//! wait for one that is starting (CLI-PROTOCOL: the app shell's
//! compatibility check, Q04, given to the CLI under the owner's rule that
//! the CLI and the app always use one daemon).
//!
//! **The look.** The file is read ([`read_discovery_bytes`], so a file
//! another build wrote is told apart from no file), the address it names is
//! asked `GET /v1/health` under [`HEALTH_PROBE_TIMEOUT`] and the answer read
//! by [`protocol::read_health_body`] — the reader the app shell's probe uses
//! too — and the file's pid is checked only when the decision turns on it
//! ([`needs_pid`]).
//!
//! **[`decide_reuse`]** (`openalpaca_app_support::protocol`, the function
//! the shell's `ensure_with` calls) is the decision; this module words each
//! verdict for a terminal: a daemon used, with a note on stderr when it
//! speaks an older protocol or is another version of this one; a mismatch
//! refused with the sentence naming both protocols and the way out; a
//! daemon starting waited for, up to [`STARTING_BUDGET`] from the first
//! silent look, then reported still starting. The CLI never spawns a daemon
//! here — a spawn verdict is "not running" — and never stops one.
//!
//! **[`ensure_with`]** and [`wait_after_start`] run over a [`World`] — the
//! look, the clock, the sleep, the lines on stderr — so a test drives each
//! path against a loopback stub with a clock it owns; [`RealWorld`] is the
//! machine's.

use anyhow::{Result, bail};
use openalpaca_app_support::daemon_lifecycle;
use openalpaca_app_support::discovery::{self, Discovery};
use openalpaca_app_support::paths;
use openalpaca_app_support::protocol::{
    self, ClientBuild, Compat, DiscoveryRead, Health, Look, Mismatch, Reused, STARTING_BUDGET,
    Verdict, decide_reuse, needs_pid, read_discovery_bytes,
};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// This CLI's build: the protocol's constants, never the debug override
/// (`protocol::reported`, a daemon's to report), and the CLI's version.
pub(crate) const THIS_CLI: ClientBuild = ClientBuild::this_build(env!("CARGO_PKG_VERSION"));

/// How long the address has to answer the whole `GET /v1/health`. Longer
/// than the app shell's 300 ms: a command run while the daemon is busy is
/// not a window that asks again every second, and an answer missed here
/// reads as a daemon starting, which costs a wait.
pub(crate) const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// How often a wait looks again.
const POLL: Duration = Duration::from_millis(250);

// ============================================================================
// The world
// ============================================================================

/// What [`ensure_with`] and [`wait_after_start`] read and do, injected.
pub(crate) trait World {
    /// One look: the file, health at its address, the pid when the
    /// decision turns on it.
    fn look(&mut self) -> impl Future<Output = Look> + Send;
    /// The monotonic clock.
    fn now(&self) -> Instant;
    /// Wait.
    fn sleep(&mut self, by: Duration) -> impl Future<Output = ()> + Send;
    /// A line for the person, on stderr: never stdout, which a command's
    /// output (its JSON among them) owns.
    fn say(&mut self, line: String);
    /// The note beside a daemon used: [`Self::say`], once per process in
    /// the real world (a command that connects for each request, as
    /// `confirmations watch` does, says it once).
    fn note(&mut self, line: String);
}

/// The store's `discovery.json`, read now.
fn read_discovery_file() -> DiscoveryRead {
    let Ok(path) = paths::discovery_path() else {
        return DiscoveryRead::Unreadable;
    };
    match std::fs::read(&path) {
        Ok(bytes) => read_discovery_bytes(&bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DiscoveryRead::Missing,
        Err(_) => DiscoveryRead::Unreadable,
    }
}

/// `GET /v1/health` at `host:port`, read as [`protocol::read_health_body`]
/// reads a `200`'s body; any other status, a transport failure or a
/// timeout is no answer, with why.
pub(crate) async fn probe_health(host: &str, port: u16) -> Health {
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let client = match reqwest::Client::builder()
        .timeout(HEALTH_PROBE_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(e) => return Health::Unanswered(format!("no HTTP client: {e}")),
    };
    let response = match client
        .get(format!("http://{authority}/v1/health"))
        .send()
        .await
    {
        Ok(response) => response,
        Err(e) if e.is_timeout() => {
            return Health::Unanswered(format!(
                "no answer within {} ms",
                HEALTH_PROBE_TIMEOUT.as_millis()
            ));
        }
        Err(e) => return Health::Unanswered(format!("{e}")),
    };
    let status = response.status();
    if status != reqwest::StatusCode::OK {
        return Health::Unanswered(format!("HTTP {}", status.as_u16()));
    }
    match response.bytes().await {
        Ok(body) => protocol::read_health_body(&body),
        Err(e) => Health::Unanswered(format!("a body cut short: {e}")),
    }
}

/// One look at the machine, with `pid_is_daemon` the process table's
/// answer (the real world's is `daemon_lifecycle::pid_is_daemon`).
pub(crate) async fn look_with(pid_is_daemon: impl Fn(u32) -> bool) -> Look {
    let discovery = read_discovery_file();
    let health = match &discovery {
        DiscoveryRead::Parsed(d) => probe_health(&d.listen.host, d.listen.port).await,
        _ => Health::Unanswered("no address to ask".into()),
    };
    let pid_live = needs_pid(&discovery, &health).is_some_and(pid_is_daemon);
    Look {
        discovery,
        health,
        pid_live,
    }
}

/// Whether this process has said its note.
static NOTED: AtomicBool = AtomicBool::new(false);

/// The machine's file, process table, address and clock; stderr.
pub(crate) struct RealWorld;

impl World for RealWorld {
    async fn look(&mut self) -> Look {
        look_with(daemon_lifecycle::pid_is_daemon).await
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    async fn sleep(&mut self, by: Duration) {
        tokio::time::sleep(by).await;
    }

    fn say(&mut self, line: String) {
        eprintln!("{line}");
    }

    fn note(&mut self, line: String) {
        if !NOTED.swap(true, Ordering::Relaxed) {
            eprintln!("{line}");
        }
    }
}

// ============================================================================
// The verdicts, as a terminal says them
// ============================================================================

/// The way to this CLI's own daemon from one it uses.
const OWN_DAEMON: &str = "`openalpaca daemon restart` stops it and starts this CLI's own.";

/// The note beside a daemon used, `None` for one of this CLI's protocol and
/// version.
pub(crate) fn reuse_note(cli: &ClientBuild, reused: &Reused) -> Option<String> {
    reused.compat.map(|why| match why {
        Compat::OlderProtocol => format!(
            "Note: the daemon running speaks protocol {}, older than this CLI's {}; the CLI \
             works with it. {OWN_DAEMON}",
            reused.protocol, cli.protocol
        ),
        Compat::OtherVersion => format!(
            "Note: the daemon running is version {} and this CLI is {}; they speak the same \
             protocol ({}), so the CLI uses it. {OWN_DAEMON}",
            reused.version, cli.version, reused.protocol
        ),
    })
}

/// The daemon refused: its pid, and the version and protocol it states.
fn who(m: &Mismatch) -> String {
    let mut who = format!("pid {}", m.pid);
    if let Some(v) = &m.daemon_version {
        who.push_str(&format!(", version {v}"));
    }
    if let Some(p) = m.daemon_protocol {
        who.push_str(&format!(", protocol {p}"));
    }
    who
}

/// The way out of a mismatch: updating the CLI is offered only for a
/// protocol newer than the CLI's (or none stated), else the stop alone.
fn remedy(m: &Mismatch, stop: String) -> String {
    match m.daemon_protocol {
        Some(p) if p > m.client.protocol => {
            format!("Update the CLI to one that speaks protocol {p}, or {stop}")
        }
        Some(_) => {
            let mut s = stop;
            s[..1].make_ascii_uppercase();
            s
        }
        None => format!("Update the CLI, or {stop}"),
    }
}

/// A mismatch as this CLI refuses it: both protocols, that nothing was
/// stopped or started, and the way out. A file this build cannot read is
/// another build's, which this CLI's `daemon stop` cannot read either, so
/// its way out is that build's stop or the pid's signal.
pub(crate) fn mismatch_sentence(m: &Mismatch) -> String {
    let who = who(m);
    let start_own = "start this CLI's own with `openalpaca daemon start`";
    if !m.readable {
        let stop = format!(
            "stop that daemon with the `openalpaca` of the build that started it (or `kill \
             {}`), then {start_own}.",
            m.pid
        );
        return format!(
            "The daemon running ({who}) wrote a discovery file this CLI cannot read: another \
             build of OpenAlpaca. Nothing was stopped or started. {}",
            remedy(m, stop)
        );
    }
    let range = if m.client.oldest_supported < m.client.protocol {
        format!(", and reads back to protocol {}", m.client.oldest_supported)
    } else {
        String::new()
    };
    let stop = format!("stop that daemon with `openalpaca daemon stop` and {start_own}.");
    format!(
        "The daemon running ({who}) is not one this CLI can use: the CLI speaks protocol \
         {}{range}. Nothing was stopped or started. {}",
        m.client.protocol,
        remedy(m, stop)
    )
}

/// The line said once a daemon is first seen starting.
fn starting_line(pid: u32) -> String {
    format!(
        "The daemon (pid {pid}) is starting: it has written its discovery file but does not \
         answer yet. Waiting for it, up to {} seconds.",
        STARTING_BUDGET.as_secs()
    )
}

/// The report of a daemon still silent [`STARTING_BUDGET`] after the first
/// silent look, with its log named when there is one.
fn still_starting(pid: u32) -> String {
    let budget = STARTING_BUDGET.as_secs();
    let log = paths::daemon_log_path()
        .ok()
        .filter(|path| path.is_file())
        .map(|path| format!(" Its log: {}.", path.display()))
        .unwrap_or_default();
    format!(
        "The daemon (pid {pid}) is still starting after {budget} seconds: it is running but \
         does not answer. Nothing was started, stopped or restarted.{log} If it does not finish \
         starting, stop it with `openalpaca daemon stop` and run `openalpaca daemon start`."
    )
}

/// Why a spawn verdict is "not running", from what the look saw: for a
/// file this build reads, its pid is not a live `openalpacad` and its
/// address either does not answer as a daemon (silent, refused, another
/// status) or answers as another instance than the file's.
pub(crate) fn not_running(look: &Look) -> String {
    match &look.discovery {
        DiscoveryRead::Missing => "Daemon is not running (no discovery file)".to_owned(),
        DiscoveryRead::Unreadable => {
            "Daemon is not running (its discovery file names no daemon)".to_owned()
        }
        DiscoveryRead::Foreign { pid, .. } => format!(
            "Daemon is not running (the discovery file names pid {pid}, which is not a running \
             openalpacad)"
        ),
        DiscoveryRead::Parsed(d) => {
            let address = match &look.health {
                Health::Answered { instance_id, .. } => format!(
                    "; {}:{} answers as another daemon, instance {instance_id}",
                    d.listen.host, d.listen.port
                ),
                Health::Unanswered(_) => format!(
                    ", and nothing at {}:{} answers as a daemon",
                    d.listen.host, d.listen.port
                ),
            };
            format!(
                "Daemon is not running (the discovery file names pid {}, which is not a running \
                 openalpacad{address})",
                d.pid
            )
        }
    }
}

// ============================================================================
// The check, and the wait after a start
// ============================================================================

/// The daemon waited for and its clock: the pid, its instance and when it
/// was first seen silent.
#[derive(Default)]
struct Silent {
    seen: Option<(u32, String, Instant)>,
}

impl Silent {
    /// Whether `pid` (`instance`) has been silent for [`STARTING_BUDGET`],
    /// saying the starting line at the first silent look at it.
    fn overdue<W: World>(&mut self, world: &mut W, pid: u32, instance: &str) -> bool {
        let now = world.now();
        let since = match &self.seen {
            Some((seen, seen_instance, at)) if *seen == pid && seen_instance == instance => *at,
            _ => {
                world.say(starting_line(pid));
                self.seen = Some((pid, instance.to_owned(), now));
                now
            }
        };
        now.saturating_duration_since(since) >= STARTING_BUDGET
    }
}

/// The daemon this CLI uses, over `world`, judged by `cli`: the file of a
/// daemon [`decide_reuse`] reuses (its note said), or the refusal — not
/// running, a mismatch, still starting. A daemon starting is looked at
/// again every [`POLL`] until it answers or [`STARTING_BUDGET`] has passed
/// since the first silent look; nothing is ever spawned or stopped.
pub(crate) async fn ensure_with<W: World>(world: &mut W, cli: &ClientBuild) -> Result<Discovery> {
    let mut silent = Silent::default();
    loop {
        let look = world.look().await;
        match decide_reuse(&look, cli) {
            Verdict::Reuse(reused) => {
                if let Some(note) = reuse_note(cli, &reused) {
                    world.note(note);
                }
                return Ok(reused.discovery);
            }
            Verdict::Mismatch(m) => bail!("{}", mismatch_sentence(&m)),
            Verdict::Spawn => bail!("{}", not_running(&look)),
            Verdict::Starting { pid, instance_id } => {
                if silent.overdue(world, pid, &instance_id) {
                    bail!("{}", still_starting(pid));
                }
                world.sleep(POLL).await;
            }
        }
    }
}

/// The address and token of the daemon this CLI uses: [`ensure_with`], then
/// the file's token judged as before (`discovery::ensure_not_expired`).
/// What `DaemonClient::connect_with` connects through, and the two readers
/// of the events socket — `openalpaca daemon tail` and `tasks confirmations
/// watch` — which read `discovery.json` themselves before (123-3 (b)): a
/// daemon of another protocol is refused with [`mismatch_sentence`] before
/// any socket is opened.
pub(crate) async fn connection_with<W: World>(
    world: &mut W,
    cli: &ClientBuild,
) -> Result<discovery::ConnectionInfo> {
    let disc = ensure_with(world, cli).await?;
    discovery::ensure_not_expired(&disc)?;
    Ok(discovery::ConnectionInfo::from(&disc))
}

/// What `openalpaca daemon start`'s first look decided ([`judge_before_start`]).
pub(crate) enum BeforeStart<S> {
    /// A live daemon this CLI cannot use: `start` was not called, and this is
    /// the [`mismatch_sentence`] that says so.
    Refused(String),
    /// `start` ran; the process it spawned, `None` when it found one running.
    Started(Option<S>),
}

/// `openalpaca daemon start`'s first look (123-8 (b)): a live daemon this
/// CLI cannot use — another build's file, or a protocol it does not speak —
/// is refused with [`mismatch_sentence`] before `start` runs, so its log is
/// not rotated and no daemon is spawned beside it (one that would lose the
/// store's lock): "Nothing was stopped or started" is then true. Any other
/// verdict hands over to `start` (`manager::start_daemon`, which spawns only
/// when no daemon is running), whose spawned process it returns.
pub(crate) async fn judge_before_start<W: World, S>(
    world: &mut W,
    cli: &ClientBuild,
    start: impl FnOnce() -> Result<Option<S>>,
) -> Result<BeforeStart<S>> {
    let look = world.look().await;
    if let Verdict::Mismatch(m) = decide_reuse(&look, cli) {
        return Ok(BeforeStart::Refused(mismatch_sentence(&m)));
    }
    start().map(BeforeStart::Started)
}

/// The process `openalpaca daemon start` spawned, as the wait after it
/// reads it: a daemon that exits at boot (a pre-squash database refused,
/// SQ-1, among the reasons) ends the wait at once with what it wrote to
/// its log, where a file it left naming its dead pid would read only as
/// "not running" until [`STARTING_BUDGET`] had passed.
pub(crate) trait Spawned {
    /// Its pid.
    fn pid(&self) -> u32;
    /// Who it is, for the sentence: the daemon or, in a development
    /// start, the `cargo run` that builds and runs it, with its pid.
    fn who(&self) -> String;
    /// How it exited (`ExitStatus`'s words), once it has; `None` while it
    /// runs.
    fn exited(&mut self) -> Option<String>;
    /// The daemon log's length just before the spawn, where this run's
    /// lines begin ([`paths::read_log_tail`]'s `from`).
    fn log_start(&self) -> u64;
}

/// How many of its log's last lines a daemon that exited at boot is shown
/// with: the app's figure for a daemon that did not start
/// (`TIMEOUT_TAIL_LINES` in the shell).
const EXIT_TAIL_LINES: usize = 40;

/// The report of a spawned daemon that exited before it answered: the lines
/// this run wrote to its log said first, verbatim, then the sentence naming
/// the log, which the caller follows with what it does next.
fn exited_at_boot<W: World>(world: &mut W, child: &dyn Spawned, status: &str) -> String {
    let who = child.who();
    let Ok(log) = paths::daemon_log_path() else {
        return format!("{who} exited ({status}) before it answered.");
    };
    let tail = paths::read_log_tail(&log, child.log_start(), EXIT_TAIL_LINES).unwrap_or_default();
    if tail.is_empty() {
        return format!(
            "{who} exited ({status}) before it answered, and wrote nothing to its log ({}).",
            log.display()
        );
    }
    world.say(tail);
    format!(
        "{who} exited ({status}) before it answered; the lines above are what it wrote to its \
         log ({}).",
        log.display()
    )
}

/// What the wait after `openalpaca daemon start` found.
#[derive(Debug)]
pub(crate) enum StartWait {
    /// A daemon this CLI uses answers.
    Ready,
    /// It does not, and why, worded for the terminal.
    NotReady(String),
}

/// The wait after `openalpaca daemon start` spawned a daemon (or found one
/// running): until a daemon this CLI uses answers, judged as
/// [`ensure_with`] judges it — a mismatch and a daemon still starting
/// [`STARTING_BUDGET`] after its first silent look end it — and a file
/// naming a live daemon waited for, up to [`STARTING_BUDGET`] from the
/// wait's start (a development start through `cargo run` builds first),
/// unless the process this start spawned has exited, which ends it at
/// once with what that process wrote to its log.
/// Before the app it replaces a fixed two-second sleep, so the app started
/// next finds the daemon serving and uses it (the owner's rule: one
/// daemon); `daemon start --daemon-only` and `daemon restart` wait the same
/// way and fail on what it reports (123-2 (b)). `spawned` is the process
/// this start spawned, `None` when it found one running.
pub(crate) async fn wait_after_start<W: World>(
    world: &mut W,
    cli: &ClientBuild,
    mut spawned: Option<&mut dyn Spawned>,
) -> StartWait {
    let started = world.now();
    let mut silent = Silent::default();
    loop {
        let look = world.look().await;
        match decide_reuse(&look, cli) {
            Verdict::Reuse(reused) => {
                if let Some(note) = reuse_note(cli, &reused) {
                    world.note(note);
                }
                return StartWait::Ready;
            }
            Verdict::Mismatch(m) => return StartWait::NotReady(mismatch_sentence(&m)),
            Verdict::Starting { pid, instance_id } => {
                // The child itself, exited but not yet reaped, should the
                // process table still show it: this start's daemon is gone.
                if let Some(child) = spawned.as_deref_mut()
                    && child.pid() == pid
                    && let Some(status) = child.exited()
                {
                    return StartWait::NotReady(exited_at_boot(world, child, &status));
                }
                if silent.overdue(world, pid, &instance_id) {
                    return StartWait::NotReady(still_starting(pid));
                }
            }
            Verdict::Spawn => {
                // The look's own verdicts come first: a daemon used or
                // refused, or another one starting, is the answer whatever
                // became of the child.
                if let Some(child) = spawned.as_deref_mut()
                    && let Some(status) = child.exited()
                {
                    return StartWait::NotReady(exited_at_boot(world, child, &status));
                }
                if world.now().saturating_duration_since(started) >= STARTING_BUDGET {
                    return StartWait::NotReady(format!(
                        "{} {} seconds after the start.",
                        not_running(&look),
                        STARTING_BUDGET.as_secs()
                    ));
                }
            }
        }
        world.sleep(POLL).await;
    }
}

#[cfg(test)]
mod tests;
