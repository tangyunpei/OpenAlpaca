//! Whether the daemon `discovery.json` names is reused, and the start that
//! decision drives (Q04: the daemon compatibility contract, DU-29 (a)).
//!
//! **The two-step probe.** The file is read, then the address it names is
//! asked `GET /v1/health` — a request written by hand over tokio's
//! `TcpStream`, each address under [`HEALTH_PROBE_TIMEOUT`] (Q12's form; the
//! shell has no HTTP client and needs none for one GET) — and the answer's
//! `instance_id` and `protocol` read. A bare TCP connect, what the shell
//! asked before, is answered by a booting daemon's bound socket and by any
//! other program on a recycled port.
//!
//! **[`decide_reuse`]** (`openalpaca_app_support::protocol` since Q05, for
//! the CLI's same check) is the decision, pure, over what one look saw
//! ([`Look`]): reuse, reuse with a note, a mismatch named and refused, a
//! daemon still starting, or spawn. A discovery pid that is a live
//! `openalpacad` beats every spawn verdict: a second daemon would only lose
//! the singleton lock to it and exit, after rotating `daemon.log` under it.
//! The shell words each verdict: the note, the refusal and its `gui.log`
//! line are its own. Nothing here ever kills a process or probes the lock
//! (`daemon_lifecycle::singleton_lock_is_free` takes it, and can make a
//! booting daemon exit).
//!
//! **[`ensure_with`]** is the start over a [`World`] — the file, the process
//! table, the probe, the spawn, the clock, the log — so a test drives every
//! path with a world it writes; [`RealWorld`] is the machine's. The owner's
//! rule judges it: whether the CLI or the app started the daemon, both use
//! the same one (2026-11-16). Each window's verdict, when it changes, is one
//! line of `gui.log` (Q04-VERDICT), the reconnect ladder's mismatch among
//! them ([`ladder_mismatch`], LADDER-LINE).

use crate::{ShellError, did_not_start, record_spawned, spawn_daemon};
use openalpaca_app_support::daemon_lifecycle;
use openalpaca_app_support::paths;
use openalpaca_app_support::protocol::{
    self, ClientBuild, Compat, DiscoveryRead, Health, Look, Mismatch, Reused, STARTING_BUDGET,
    Verdict, decide_reuse, needs_pid, read_discovery_bytes,
};
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ============================================================================
// The shell's own build, and what one look reads
// ============================================================================

/// This shell's build: the protocol's constants, never the debug override
/// (`protocol::reported`, a daemon's to report), and the app's version.
pub(crate) const THIS_SHELL: ClientBuild = ClientBuild::this_build(env!("CARGO_PKG_VERSION"));

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

// ============================================================================
// The verdicts, as the window and the log say them
// ============================================================================

/// The note the window shows beside a daemon it uses, `None` for one of this
/// shell's protocol and version — or one it does not use at all.
pub(crate) fn compat_note(shell: &ClientBuild, protocol: u32, version: &str) -> Option<String> {
    protocol::compat(shell, protocol, version)
        .map(|why| note_sentence(shell, why, protocol, version))
}

/// The note for a daemon [`decide_reuse`] reused, worded from its reason.
pub(crate) fn reuse_note(shell: &ClientBuild, reused: &Reused) -> Option<String> {
    reused
        .compat
        .map(|why| note_sentence(shell, why, reused.protocol, &reused.version))
}

/// The sentence for `why`: an older protocol the app works with, or its own
/// protocol at another version, each with the way to this app's own daemon.
fn note_sentence(shell: &ClientBuild, why: Compat, protocol: u32, version: &str) -> String {
    match why {
        Compat::OlderProtocol => format!(
            "This daemon speaks protocol {protocol}, older than this app's {}; the app works \
             with it. To run this app's own daemon (version {}), use Stop daemon…, then Start \
             daemon.",
            shell.protocol, shell.version
        ),
        Compat::OtherVersion => format!(
            "This daemon is version {version} and this app is {}; they speak the same \
             protocol, so the app uses it. To run this app's own version, use Stop daemon…, \
             then Start daemon.",
            shell.version
        ),
    }
}

/// The way out of a mismatch that runs this app's own daemon: the other
/// stopped from a terminal (the window cannot stop a daemon it is not
/// connected to), then the panel's `Reconnect`, which starts this app's.
const STOP_AND_RECONNECT: &str = "stop that daemon with `openalpaca daemon stop` and press \
                                  Reconnect in Settings → Connection to start this app's own.";

/// [`STOP_AND_RECONNECT`] as a sentence of its own.
const STOP_AND_RECONNECT_SENTENCE: &str = "Stop that daemon with `openalpaca daemon stop` and \
                                           press Reconnect in Settings → Connection to start \
                                           this app's own.";

/// A mismatch as this shell says it — the `who`, the `gui.log` line and the
/// refusal the webview receives — over `support::protocol`'s
/// [`Mismatch`], which carries the facts and no words.
pub(crate) trait MismatchWords {
    /// The daemon refused: its pid, and the version and protocol it states.
    fn who(&self) -> String;
    /// The refusal's line in `gui.log`: whom, and this shell's protocol.
    fn log_line(&self) -> String;
    /// The mismatch as the webview receives it: the sentence, both sides'
    /// protocols and the remedy.
    fn into_error(self) -> ShellError;
}

impl MismatchWords for Mismatch {
    fn who(&self) -> String {
        let mut who = format!("pid {}", self.pid);
        if let Some(v) = &self.daemon_version {
            who.push_str(&format!(", version {v}"));
        }
        if let Some(p) = self.daemon_protocol {
            who.push_str(&format!(", protocol {p}"));
        }
        who
    }

    fn log_line(&self) -> String {
        let unreadable = if self.readable {
            ""
        } else {
            ", in a discovery file this app cannot read"
        };
        format!(
            "refusal (mismatch) — {}{unreadable}; this app speaks protocol {}; nothing was \
             stopped or started.",
            self.who(),
            self.client.protocol
        )
    }

    fn into_error(self) -> ShellError {
        let newer = self
            .daemon_protocol
            .is_none_or(|p| p > self.client.protocol);
        let remedy = match (newer, self.daemon_protocol) {
            (true, Some(p)) => {
                format!("Update the app to one that speaks protocol {p}, or {STOP_AND_RECONNECT}")
            }
            (true, None) => format!("Update the app, or {STOP_AND_RECONNECT}"),
            (false, _) => STOP_AND_RECONNECT_SENTENCE.to_owned(),
        };
        let who = self.who();
        let message = if self.readable {
            let range = if self.client.oldest_supported < self.client.protocol {
                format!(
                    ", and reads back to protocol {}",
                    self.client.oldest_supported
                )
            } else {
                String::new()
            };
            format!(
                "The daemon running ({who}) is not one this app can use: the app speaks \
                 protocol {}{range}. Nothing was stopped or started. {remedy}",
                self.client.protocol
            )
        } else {
            format!(
                "The daemon running ({who}) wrote a discovery file this app cannot read: \
                 another build of OpenAlpaca. Nothing was stopped or started. {remedy}"
            )
        };
        ShellError::Mismatch {
            message,
            daemon_version: self.daemon_version,
            daemon_protocol: self.daemon_protocol,
            shell_protocol: self.client.protocol,
            remedy,
        }
    }
}

// ============================================================================
// The starting state's clock, the spawn waited for, the verdicts logged
// ============================================================================

/// A daemon this app spawned whose wait is not over: its pid, where its
/// lines in the log begin, when it was spawned, and when an ask last
/// answered (the spawn is forgotten once [`STARTING_BUDGET`] has passed
/// since then with no ask: the window is gone).
#[derive(Debug, Clone, Copy)]
struct PendingSpawn {
    pid: u32,
    log_start: u64,
    at: Instant,
    asked: Instant,
}

/// What an ask answered, as `gui.log` tells one verdict from the next: its
/// kind and the daemon it is about. A failed spawn is one too, though no
/// line of its own: the same verdict after it is a change.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Said {
    Starting(u32),
    StillStarting(u32),
    Reuse(u32),
    Mismatch(u32),
    SidecarMissing,
    SpawnFailed,
}

/// The app's memory between asks: when the shell first saw the daemon it is
/// waiting for silent (its pid, its instance and the instant — the budget
/// counts from there, across invokes, so a window that asks again every
/// second never restarts it); the spawn whose wait is not over (*Rev 106*
/// (8): an ask after this app's spawn answers `starting` at once, so the
/// wait goes on across asks); every pid this run of the app spawned (only
/// it knows a reuse of this app's own daemon as such once the spawn's wait
/// is over, after the timeout report; `SPAWNED` in `lib.rs` is the real
/// spawn's `PATH` report, kept only where one was captured, and this watch
/// is the state the tests inject; a recycled pid can mislabel only a
/// `gui.log` line); and each window's last verdict, by its label.
#[derive(Debug, Default)]
pub(crate) struct StartingWatch {
    seen: Option<(u32, String, Instant)>,
    spawn: Option<PendingSpawn>,
    spawned: Vec<u32>,
    said: Vec<(String, Said)>,
}

impl StartingWatch {
    /// An empty watch.
    pub(crate) const fn new() -> Self {
        Self {
            seen: None,
            spawn: None,
            spawned: Vec::new(),
            said: Vec::new(),
        }
    }

    /// When `pid` (`instance_id`) was first seen silent; `now` when this is
    /// the first look at it, which starts its count.
    pub(crate) fn since(&mut self, pid: u32, instance_id: &str, now: Instant) -> Instant {
        match &self.seen {
            Some((seen, instance, at)) if *seen == pid && instance == instance_id => *at,
            _ => {
                self.seen = Some((pid, instance_id.to_owned(), now));
                now
            }
        }
    }

    /// Forget the spawn at an ask made `now` once [`STARTING_BUDGET`] has
    /// passed since the last ask answered (the window closed or
    /// disconnected meanwhile), as the one ask's wait would have been over
    /// by then. Called once per ask, before any verdict is judged, so every
    /// reader sees the same spawn.
    fn forget_stale_spawn(&mut self, now: Instant) {
        if self
            .spawn
            .is_some_and(|p| now.saturating_duration_since(p.asked) >= STARTING_BUDGET)
        {
            self.spawn = None;
        }
    }

    /// An ask answered `now`: a spawn whose wait goes on counts its
    /// staleness from here.
    fn answered(&mut self, now: Instant) {
        if let Some(p) = &mut self.spawn {
            p.asked = now;
        }
    }

    /// Forget the daemon waited for and the spawn: it answered, or another
    /// verdict came.
    fn settle(&mut self) {
        self.seen = None;
        self.spawn = None;
    }

    /// Record `said` as `window`'s verdict: whether it is a change.
    fn say(&mut self, window: &str, said: Said) -> bool {
        match self.said.iter_mut().find(|(label, _)| label == window) {
            Some((_, last)) if *last == said => false,
            Some((_, last)) => {
                *last = said;
                true
            }
            None => {
                self.said.push((window.to_owned(), said));
                true
            }
        }
    }

    /// Forget everything, as at the app's start: no test inherits another's.
    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        *self = Self::new();
    }
}

/// The app's one watch.
pub(crate) static STARTING: Mutex<StartingWatch> = Mutex::new(StartingWatch::new());

fn watch(watch: &Mutex<StartingWatch>) -> std::sync::MutexGuard<'_, StartingWatch> {
    watch.lock().unwrap_or_else(|p| {
        tracing::warn!("The starting watch's lock was poisoned; recovering.");
        p.into_inner()
    })
}

/// The answer while a daemon starts: the window parks and asks again.
fn starting(pid: u32) -> ShellError {
    ShellError::Starting {
        message: format!(
            "The daemon (pid {pid}) is starting: it has written its discovery file but does \
             not answer yet. Waiting for it, up to {} seconds.",
            STARTING_BUDGET.as_secs()
        ),
    }
}

/// The sentence every timeout report ends with (*Rev 106* (9)): the way
/// out the mismatch names, worded for a boot that may yet finish, since a
/// person may read it while a slow one still goes on. `openalpaca daemon
/// stop` signals the pid, a hung boot included; `Reconnect` then starts a
/// daemon afresh.
const STILL_STARTING_WAY_OUT: &str = "If it does not finish starting, stop it with `openalpaca \
                                      daemon stop` and press Reconnect in Settings → Connection \
                                      to start a new one.";

/// Whose daemon the timeout report is about, which sets its words and
/// which of the log's lines it quotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Waited {
    /// One found already there, or waited for past a spawn's report: the
    /// log's last lines.
    Found,
    /// The one this app spawned, its wait not over: what the log holds
    /// since that spawn.
    Spawned,
    /// Another than the one this app spawned (`ours`) — the CLI's, which
    /// took the store's lock first: what the log holds since this app's
    /// spawn, named so.
    Other { ours: u32 },
}

/// The timeout report: a daemon still not answering [`STARTING_BUDGET`]
/// after the first silent look, with the log's tail as [`Waited`] says —
/// or why the log could not be read, still this kind: the report is about
/// the daemon, never a failed spawn.
fn still_starting(pid: u32, read: Result<(PathBuf, String), String>, waited: Waited) -> ShellError {
    let head = format!(
        "The daemon (pid {pid}) is still starting after {} seconds: it is running but does \
         not answer.",
        STARTING_BUDGET.as_secs()
    );
    let nothing = match waited {
        Waited::Found => "Nothing was started, stopped or restarted".to_owned(),
        Waited::Spawned => "Nothing was stopped or restarted".to_owned(),
        Waited::Other { ours } => format!(
            "This app started a daemon too (pid {ours}), but the discovery file names this \
             one; nothing was stopped or restarted"
        ),
    };
    let (log_path, tail) = match read {
        Ok(read) => read,
        Err(why) => {
            let stop = if why.ends_with('.') { "" } else { "." };
            return ShellError::StillStarting {
                message: format!(
                    "{head} {nothing}, and its log could not be read: {why}{stop} \
                     {STILL_STARTING_WAY_OUT}"
                ),
                log_tail: None,
            };
        }
    };
    if tail.is_empty() {
        return ShellError::StillStarting {
            message: format!(
                "{head} {nothing}, and its log ({}) holds nothing yet. {STILL_STARTING_WAY_OUT}",
                log_path.display()
            ),
            log_tail: None,
        };
    }
    let quoted = match waited {
        Waited::Found => format!("The last lines of its log ({}):", log_path.display()),
        Waited::Spawned => format!("Its log ({}) ends with:", log_path.display()),
        Waited::Other { .. } => format!(
            "What its log ({}) holds since this app's start:",
            log_path.display()
        ),
    };
    ShellError::StillStarting {
        message: format!("{head} {nothing}. {quoted}\n\n{tail}\n\n{STILL_STARTING_WAY_OUT}"),
        log_tail: Some(tail),
    }
}

// ============================================================================
// The start
// ============================================================================

/// How long a daemon this run spawned has to write a discovery file naming
/// a live daemon before the start is reported failed.
pub(crate) const SPAWN_WAIT: Duration = Duration::from_secs(5);

/// How often the wait after a spawn looks again.
const SPAWN_POLL: Duration = Duration::from_millis(200);

/// A daemon [`World::spawn`] started: its pid, and the log's length before
/// it, where this run's lines begin.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SpawnedDaemon {
    pub(crate) pid: u32,
    pub(crate) log_start: u64,
}

/// A spawn refused, with the refusal the window reads: the sidecar is not
/// beside the app, or the spawn failed otherwise. `gui.log` logs the first
/// as a verdict of its own.
#[derive(Debug)]
pub(crate) enum SpawnRefusal {
    SidecarMissing(ShellError),
    Failed(ShellError),
}

/// `spawn_daemon`'s failure as a [`SpawnRefusal`]: its sentence, as before
/// Q04-VERDICT, `Failed to spawn daemon: ` and the error, the sidecar
/// missing told apart by its type ([`crate::SidecarMissing`]).
pub(crate) fn spawn_refusal(e: &anyhow::Error) -> SpawnRefusal {
    let refused = crate::spawn_failed(format!("Failed to spawn daemon: {e:#}"));
    if e.downcast_ref::<crate::SidecarMissing>().is_some() {
        SpawnRefusal::SidecarMissing(refused)
    } else {
        SpawnRefusal::Failed(refused)
    }
}

/// What [`ensure_with`] reads and does, injected.
pub(crate) trait World {
    /// One look: the file, health at its address, and the pid when the
    /// decision turns on it ([`needs_pid`]).
    fn look(&mut self) -> impl Future<Output = Look> + Send;
    /// Spawn a daemon: its pid and where this run's lines in the log begin.
    fn spawn(&mut self) -> impl Future<Output = Result<SpawnedDaemon, SpawnRefusal>> + Send;
    /// The monotonic clock.
    fn now(&self) -> Instant;
    /// Wait.
    fn sleep(&mut self, by: Duration) -> impl Future<Output = ()> + Send;
    /// One `INFO` line of the shell's log.
    fn log(&mut self, line: String);
    /// The daemon log's path and its last lines: from `from` (this run's
    /// spawn) to the end, or the last lines when `None`; why not, when the
    /// log's path cannot be had or the read cannot be run.
    fn log_tail(
        &mut self,
        from: Option<u64>,
    ) -> impl Future<Output = Result<(PathBuf, String), String>> + Send;
}

/// What one ask answers, what it said for `gui.log`'s count of changes, and
/// its line there, `None` for a verdict that has none.
struct Answer {
    result: Result<Reused, ShellError>,
    said: Said,
    line: Option<String>,
}

/// Ensure a daemon this shell can use, over `world`, for the window
/// labelled `window`:
///
/// - the first look's verdict: a reuse or a mismatch is the answer; a
///   daemon starting answers [`ShellError::Starting`] at once — the window
///   parks and asks again — until [`STARTING_BUDGET`] has passed since the
///   first silent look at that pid, then [`ShellError::StillStarting`], at
///   every ask, and never a spawn;
/// - only a spawn verdict spawns, once; the wait after it judges the daemon
///   the file then names, which need not be the one spawned (the CLI's may
///   have taken the lock first): a reuse or a mismatch ends it, and a daemon
///   starting answers `starting` at once (*Rev 106* (8)), the budget counted
///   from that first silent look, as the first look's does. The spawn is
///   remembered until its wait is over (or [`STARTING_BUDGET`] has passed
///   since the last ask answered), so a later ask that finds no live
///   daemon named goes on with it rather than spawning again; with none
///   named [`SPAWN_WAIT`] after the spawn it ends with the two 5 s
///   sentences. The timeout report ends the wait too, as the one ask's did.
///
/// Each answer that differs from `window`'s last is one line of the
/// shell's log: starting, still starting, a reuse (whose daemon it is, as
/// far as this run knows), a refusal (a mismatch, the sidecar missing). A
/// reuse or a mismatch line names the daemon's pid, version and protocol,
/// a starting or still-starting line its pid, the sidecar line the file;
/// none its token, its log or a path.
pub(crate) async fn ensure_with<W: World>(
    world: &mut W,
    starting_watch: &Mutex<StartingWatch>,
    shell: &ClientBuild,
    window: &str,
) -> Result<Reused, ShellError> {
    let Answer { result, said, line } = answer(world, starting_watch, shell).await;
    let now = world.now();
    let changed = {
        let mut w = watch(starting_watch);
        w.answered(now);
        w.say(window, said)
    };
    if changed && let Some(line) = line {
        world.log(verdict_line(window, &line));
    }
    result
}

/// `line` as `gui.log` holds a verdict of the window labelled `window`.
fn verdict_line(window: &str, line: &str) -> String {
    format!("Daemon verdict for window `{window}`: {line}")
}

/// The reconnect ladder's mismatch — `get_connection_info`'s refusal from
/// the file over a live pid ([`file_mismatch`]), which parks the window as
/// [`ensure_with`]'s does — as the verdict of the window labelled `window`
/// (*Rev 114* (3) (b), LADDER-LINE): recorded in the same per-window record
/// and, when it changes that window's verdict, the same line handed to
/// `log`, so a run of refusals is one line and a mismatch either path
/// logged first is not logged again by the other. It is the same refusal
/// by another path, and the common one when a daemon is replaced while the
/// window is open. Nothing else of the watch is touched: the starting
/// clock and a spawn's wait are [`ensure_with`]'s.
pub(crate) fn ladder_mismatch(
    starting_watch: &Mutex<StartingWatch>,
    window: &str,
    mismatch: &Mismatch,
    log: impl FnOnce(String),
) {
    if watch(starting_watch).say(window, Said::Mismatch(mismatch.pid)) {
        log(verdict_line(window, &mismatch.log_line()));
    }
}

/// [`ensure_with`]'s answer, before its line is logged.
async fn answer<W: World>(
    world: &mut W,
    starting_watch: &Mutex<StartingWatch>,
    shell: &ClientBuild,
) -> Answer {
    // A spawn no ask followed for the budget (the window gone meanwhile)
    // is over, as the one ask's wait would have been by then: forgotten
    // here, once, before any reader.
    let now = world.now();
    watch(starting_watch).forget_stale_spawn(now);
    let verdict = decide_reuse(&world.look().await, shell);
    if let Some(answer) = judge(world, starting_watch, verdict).await {
        return answer;
    }
    // A spawn whose wait is not over (an ask after it answered `starting`)
    // goes on, never a second spawn: its daemon is gone or not yet named,
    // as a look inside the one ask would have found it.
    let pending = watch(starting_watch).spawn;
    let pending = match pending {
        Some(pending) => pending,
        None => {
            watch(starting_watch).seen = None;
            let SpawnedDaemon { pid, log_start } = match world.spawn().await {
                Ok(spawned) => spawned,
                Err(SpawnRefusal::SidecarMissing(refused)) => {
                    return Answer {
                        result: Err(refused),
                        said: Said::SidecarMissing,
                        line: Some(format!(
                            "refusal (sidecar missing) — no `{}` beside the app; nothing was \
                             started.",
                            crate::SIDECAR_NAME
                        )),
                    };
                }
                Err(SpawnRefusal::Failed(refused)) => {
                    return Answer {
                        result: Err(refused),
                        said: Said::SpawnFailed,
                        line: None,
                    };
                }
            };
            let at = world.now();
            let pending = PendingSpawn {
                pid,
                log_start,
                at,
                asked: at,
            };
            let mut w = watch(starting_watch);
            w.spawn = Some(pending);
            w.spawned.push(pid);
            pending
        }
    };
    loop {
        if world.now().saturating_duration_since(pending.at) >= SPAWN_WAIT {
            watch(starting_watch).settle();
            // As before Q04: a log that cannot be read is the failed
            // start's own sentence.
            let result = match world.log_tail(Some(pending.log_start)).await {
                Ok((path, tail)) => Err(did_not_start(&path, tail)),
                Err(why) => Err(crate::spawn_failed(why)),
            };
            return Answer {
                result,
                said: Said::SpawnFailed,
                line: None,
            };
        }
        world.sleep(SPAWN_POLL).await;
        let verdict = decide_reuse(&world.look().await, shell);
        if let Some(answer) = judge(world, starting_watch, verdict).await {
            return answer;
        }
    }
}

/// The answer to a verdict, the same at the first look and after a spawn;
/// `None` for a spawn verdict, which the spawn and its wait answer.
async fn judge<W: World>(
    world: &mut W,
    starting_watch: &Mutex<StartingWatch>,
    verdict: Verdict,
) -> Option<Answer> {
    let pending = watch(starting_watch).spawn;
    let answer = match verdict {
        Verdict::Spawn => return None,
        Verdict::Reuse(reused) => {
            let pid = reused.discovery.pid;
            let spawned_here = watch(starting_watch).spawned.contains(&pid);
            let whose = match pending {
                _ if spawned_here => "the daemon this app started".to_owned(),
                Some(ours) => format!("not the one this app started (pid {})", ours.pid),
                None => "already running: this run of the app did not start it".to_owned(),
            };
            watch(starting_watch).settle();
            let line = format!(
                "reuse — pid {pid}, version {}, protocol {}, {whose}.",
                reused.version, reused.protocol
            );
            Answer {
                result: Ok(reused),
                said: Said::Reuse(pid),
                line: Some(line),
            }
        }
        Verdict::Mismatch(mismatch) => {
            watch(starting_watch).settle();
            Answer {
                said: Said::Mismatch(mismatch.pid),
                line: Some(mismatch.log_line()),
                result: Err(mismatch.into_error()),
            }
        }
        Verdict::Starting { pid, instance_id } => {
            let now = world.now();
            let since = watch(starting_watch).since(pid, &instance_id, now);
            let budget = STARTING_BUDGET.as_secs();
            if now.saturating_duration_since(since) < STARTING_BUDGET {
                return Some(Answer {
                    result: Err(starting(pid)),
                    said: Said::Starting(pid),
                    line: Some(format!(
                        "starting — pid {pid} has written its discovery file but its health \
                         does not answer yet; waiting up to {budget} s, starting nothing beside \
                         it."
                    )),
                });
            }
            let (from, waited) = match pending {
                None => (None, Waited::Found),
                Some(ours) if ours.pid == pid => (Some(ours.log_start), Waited::Spawned),
                Some(ours) => (Some(ours.log_start), Waited::Other { ours: ours.pid }),
            };
            // The report ends the spawn's wait, as it ended the one ask:
            // an ask after it that finds no live daemon spawns afresh.
            watch(starting_watch).spawn = None;
            Answer {
                result: Err(still_starting(pid, world.log_tail(from).await, waited)),
                said: Said::StillStarting(pid),
                line: Some(format!(
                    "still starting — pid {pid} has not answered {budget} s after the first \
                     silent look; reported, nothing spawned over it."
                )),
            }
        }
    };
    Some(answer)
}

// ============================================================================
// The machine's world
// ============================================================================

/// The real file, process table, address, spawn and clock. Every file read,
/// the process-table read and the spawn run on the blocking pool; the probe
/// is tokio's, under tokio's timeout.
pub(crate) struct RealWorld;

impl World for RealWorld {
    async fn look(&mut self) -> Look {
        let discovery = tauri::async_runtime::spawn_blocking(read_discovery_file)
            .await
            .unwrap_or(DiscoveryRead::Unreadable);
        let health = match &discovery {
            DiscoveryRead::Parsed(d) => probe_health(&d.listen.host, d.listen.port).await,
            _ => Health::Unanswered("no address to ask".into()),
        };
        let pid_live = match needs_pid(&discovery, &health) {
            Some(pid) => {
                tauri::async_runtime::spawn_blocking(move || daemon_lifecycle::pid_is_daemon(pid))
                    .await
                    .unwrap_or(false)
            }
            None => false,
        };
        Look {
            discovery,
            health,
            pid_live,
        }
    }

    async fn spawn(&mut self) -> Result<SpawnedDaemon, SpawnRefusal> {
        let spawned = tauri::async_runtime::spawn_blocking(spawn_daemon)
            .await
            .map_err(|e| {
                SpawnRefusal::Failed(crate::spawn_failed(format!("Failed to spawn daemon: {e}")))
            })?
            .map_err(|e| spawn_refusal(&e))?;
        record_spawned(spawned.pid, spawned.path);
        Ok(SpawnedDaemon {
            pid: spawned.pid,
            log_start: spawned.log_start,
        })
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    async fn sleep(&mut self, by: Duration) {
        tokio::time::sleep(by).await;
    }

    fn log(&mut self, line: String) {
        tracing::info!("{line}");
    }

    async fn log_tail(&mut self, from: Option<u64>) -> Result<(PathBuf, String), String> {
        // Only what this run's spawn wrote, when it spawned, so an earlier
        // run's last words are never presented as this one's; else the
        // log's last lines, which the sentence names as such.
        tauri::async_runtime::spawn_blocking(move || {
            let log_path = paths::daemon_log_path().map_err(|e| format!("{e:#}"))?;
            let tail =
                paths::read_log_tail(&log_path, from.unwrap_or(0), crate::TIMEOUT_TAIL_LINES)
                    .unwrap_or_default();
            Ok::<_, String>((log_path, tail))
        })
        .await
        .map_err(|e| format!("Could not read the daemon log: {e}"))
        .and_then(|read| read)
    }
}

// ============================================================================
// The health probe (the hand-written GET, (a-i))
// ============================================================================

/// How long one address has to answer the whole GET: connect, request and
/// response (plan-rev3 §7.2 G3-b's 300 ms).
pub(crate) const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// The most of an answer read: a health body is under 200 bytes.
const HEALTH_MAX_BYTES: u64 = 16 * 1024;

/// `GET /v1/health` at `host:port`: the first address whose GET completes
/// within [`HEALTH_PROBE_TIMEOUT`], read as [`parse_health_response`] reads
/// it.
pub(crate) async fn probe_health(host: &str, port: u16) -> Health {
    let Ok(addrs) = tokio::net::lookup_host((host, port)).await else {
        return Health::Unanswered(format!("{host} did not resolve"));
    };
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let request = format!(
        "GET /v1/health HTTP/1.1\r\nHost: {authority}\r\nAccept: application/json\r\n\
         Connection: close\r\n\r\n"
    );
    match first_answer(addrs, |addr| health_get(addr, request.as_bytes())).await {
        Some(bytes) => parse_health_response(&bytes),
        None => Health::Unanswered(format!(
            "no address answered within {} ms",
            HEALTH_PROBE_TIMEOUT.as_millis()
        )),
    }
}

/// The first of `addrs`, tried in order, whose `attempt` completes within
/// [`HEALTH_PROBE_TIMEOUT`] — each address given that and no more. The
/// attempt is a parameter so a test can give the probe an address that
/// never answers.
pub(crate) async fn first_answer<T, F>(
    addrs: impl IntoIterator<Item = SocketAddr>,
    attempt: impl Fn(SocketAddr) -> F,
) -> Option<T>
where
    F: Future<Output = std::io::Result<T>>,
{
    for addr in addrs {
        if let Ok(Ok(answer)) = tokio::time::timeout(HEALTH_PROBE_TIMEOUT, attempt(addr)).await {
            return Some(answer);
        }
    }
    None
}

/// One GET to `addr`: the request written, the answer read until it is
/// whole ([`answer_is_whole`]: its `Content-Length`, or a chunked body's
/// last chunk) or the connection closes, at most [`HEALTH_MAX_BYTES`]. The
/// request asks the daemon to close, but the read never waits on that.
async fn health_get(addr: SocketAddr, request: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut stream = tokio::net::TcpStream::connect(addr).await?;
    stream.write_all(request).await?;
    let mut answer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        answer.extend_from_slice(&chunk[..read]);
        if answer_is_whole(&answer) || answer.len() as u64 >= HEALTH_MAX_BYTES {
            break;
        }
    }
    Ok(answer)
}

/// Whether `answer` holds a whole HTTP/1.x response: its head, and a body
/// as long as its `Content-Length` or ending in a chunked body's last chunk.
/// A head with neither is whole only when the connection closes.
pub(crate) fn answer_is_whole(answer: &[u8]) -> bool {
    let Some(split) = answer.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&answer[..split]).to_ascii_lowercase();
    let body = &answer[split + 4..];
    for line in head.split("\r\n").skip(1) {
        if let Some(length) = line.strip_prefix("content-length:") {
            return length
                .trim()
                .parse::<usize>()
                .is_ok_and(|length| body.len() >= length);
        }
        if line.starts_with("transfer-encoding:") && line.contains("chunked") {
            return dechunk(body).is_some();
        }
    }
    false
}

/// An HTTP/1.x answer as a daemon's health, or why it is not one: status
/// `200`, a body (whole or chunked) that is a JSON object whose `status` is
/// `ok` and whose `instance_id` is a string; a `protocol` present must be a
/// number a `u32` holds.
pub(crate) fn parse_health_response(bytes: &[u8]) -> Health {
    let unanswered = |why: String| Health::Unanswered(why);
    let Some(split) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
        return unanswered("not an HTTP answer".into());
    };
    let head = String::from_utf8_lossy(&bytes[..split]);
    let mut lines = head.split("\r\n");
    let mut status = lines.next().unwrap_or_default().split_whitespace();
    let (Some(version), Some(code)) = (status.next(), status.next()) else {
        return unanswered("not an HTTP answer".into());
    };
    if !version.starts_with("HTTP/1.") {
        return unanswered("not an HTTP/1 answer".into());
    }
    if code != "200" {
        return unanswered(format!("HTTP {code}"));
    }
    let chunked = lines.any(|line| {
        let line = line.to_ascii_lowercase();
        line.starts_with("transfer-encoding:") && line.contains("chunked")
    });
    let rest = &bytes[split + 4..];
    let body = if chunked {
        match dechunk(rest) {
            Some(body) => body,
            None => return unanswered("a chunked body cut short".into()),
        }
    } else {
        rest.to_vec()
    };
    // `lines` is spent on the scan above; the length is read again here.
    let length = head.split("\r\n").skip(1).find_map(|line| {
        line.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|length| length.trim().parse::<usize>().ok())
    });
    let body = match length {
        Some(length) if !chunked => body.get(..length).map(<[u8]>::to_vec).unwrap_or(body),
        _ => body,
    };
    let Ok(serde_json::Value::Object(body)) = serde_json::from_slice(&body) else {
        return unanswered("a body that is not a JSON object".into());
    };
    if body.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
        return unanswered("a body whose status is not ok".into());
    }
    let Some(instance_id) = body.get("instance_id").and_then(serde_json::Value::as_str) else {
        return unanswered("a body with no instance_id".into());
    };
    let protocol = match body.get("protocol") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => match value.as_u64().and_then(|n| u32::try_from(n).ok()) {
            Some(protocol) => Some(protocol),
            None => return unanswered("a protocol that is not a number".into()),
        },
    };
    Health::Answered {
        instance_id: instance_id.to_owned(),
        protocol,
        version: body
            .get("version")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    }
}

/// A chunked body's bytes, `None` when it is cut short or malformed.
fn dechunk(mut rest: &[u8]) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let end = rest.windows(2).position(|w| w == b"\r\n")?;
        let size = std::str::from_utf8(&rest[..end]).ok()?;
        let size = usize::from_str_radix(size.split(';').next()?.trim(), 16).ok()?;
        rest = &rest[end + 2..];
        if size == 0 {
            return Some(body);
        }
        body.extend_from_slice(rest.get(..size)?);
        rest = rest.get(size..)?.strip_prefix(b"\r\n")?;
    }
}

#[cfg(test)]
mod tests;
