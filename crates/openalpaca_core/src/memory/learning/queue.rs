//! The learning queue (design 09 §1.5).
//!
//! In memory it keeps three things: per session, the user-authored turns not
//! yet learned, when the last turn landed, whether it is due and its retry
//! backoff; per lane, the current session; and the set of turns in flight.
//! The gateway feeds it through a [`TurnGuard`]: [`LearningQueue::turn_started`]
//! before the user row is persisted, [`TurnGuard::attach_session`] once
//! persistence resolves the session, and the guard's drop after the assistant
//! row is written — or on any earlier exit. Each is one `std::sync::Mutex`
//! update: no await, no database, no model.
//!
//! **The triggers** make a session due: 10 user-authored turns since it was
//! last learned (`turn_finished`), about two minutes of idle and a switch to
//! another conversation (the sweeper), the stop drain, and the boot catch-up.
//! A due session starts only when no turn has been in flight for 20 s, one
//! flush at a time on the whole daemon.
//!
//! A **flush** learns a session's window chunk by chunk (§1.6): read the
//! window, ask the model, then store the chunk's notes and advance the
//! durable cursor in one blocking task (§1.4). Delivery is at-least-once: a
//! crash between the writes and the advance repeats the chunk, and the
//! guards of §1.4 make the repeat harmless. Before every chunk an automatic
//! flush checks again that chat is quiet; on a local model a turn that starts
//! cancels the call (the GPU serves the reply alone), and on a keyed remote
//! model a chunk starts only when half of the key's rate budget would remain.

use super::extractor::{self, AGENT_LABEL, StoreCounts, StoreEnd, StoreJob};
use super::window::{self, NextChunk};
use super::{
    BACKOFF_BASE, BACKOFF_CAP, Budget, MAX_ATTEMPTS, MAX_CHUNKS_PER_FLUSH, RESUME_QUIET,
    WINDOW_READ_ROWS,
};
use crate::daemon_config::DaemonConfig;
use crate::memory::service::{
    Attribution, Capabilities, MemoryError, MemoryService, Surface, WorkspaceHint,
};
use crate::orchestrator::BackgroundJobs;
use arc_swap::ArcSwap;
use openalpaca_llm::{LlmError, LlmRouter, LlmRouterError};
use openalpaca_storage::repository::LlmUsageRepository;
use openalpaca_storage::{
    Conversation, ConversationRepository, Database, LearningCursor, LearningCursorRepository,
    LearningRow,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// How the queue decides when to flush.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueMode {
    /// The daemon's: the sweeper and the triggers start flushes.
    Automatic,
    /// The evaluation harness's (design §7.2): nothing starts a flush on its
    /// own; only an explicit flush does.
    Manual,
}

/// What a flush needs from the daemon.
#[derive(Clone)]
pub struct LearningDeps {
    pub db: Database,
    pub router: Arc<LlmRouter>,
    pub daemon_config: Arc<ArcSwap<DaemonConfig>>,
    pub memory_service: Arc<MemoryService>,
}

/// Why a flush stopped before its session's window was learned. `None` on
/// a [`FlushReport`] means the window was learned to its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushStop {
    /// `[orchestrator.learning] enabled = false`.
    Disabled,
    /// This session's flush was already running (one flush per session).
    AlreadyRunning,
    /// The next row is a turn still in flight: its answer has not landed.
    InFlight,
    /// A chat turn is in flight, or one ended less than 20 s ago: an
    /// automatic flush stops at the chunk boundary. The session stays due.
    Busy,
    /// A local model's call was cancelled because a chat turn started. The
    /// session stays due; nothing was written or counted.
    Yielded,
    /// A keyed remote model's rate budget would drop under half: the chunk
    /// waits for the next sweep. Nothing was spent or counted.
    RateHeadroom,
    /// The queue's shutdown token fired before the model answered.
    Cancelled,
    /// No model could answer (§7b P10). The window holds, with backoff.
    Unavailable,
    /// A reachable model answered with something unusable. The attempt is
    /// counted; the window holds until the third (ruling LR20).
    BadOutput,
    /// A partition the chunk would write to is quarantined (design §2).
    Quarantined,
    /// A daily cap binds (§1.8). The window stays queued.
    Cap,
    /// A note write failed on I/O. The window holds, with backoff.
    StoreError,
    /// The session was deleted (LQ4): its unlearned turns went with it.
    SessionGone,
    /// The flush ran its chunk budget; the rest stays due.
    ChunkLimit,
}

impl FlushStop {
    /// A stop the session waits out with a retry backoff (§1.4).
    fn backs_off(self) -> bool {
        matches!(
            self,
            FlushStop::Unavailable
                | FlushStop::BadOutput
                | FlushStop::Quarantined
                | FlushStop::StoreError
        )
    }
}

/// What one flush did (design §2.4's outcomes, counted without bodies).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlushReport {
    /// Model calls made.
    pub calls: usize,
    /// Chunks whose window the cursor moved past, with or without a call.
    pub chunks: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Notes written: `active + candidates`.
    pub stored: usize,
    /// Active `extracted_user` notes — the user's own words.
    pub active: usize,
    /// `extracted_other` candidates.
    pub candidates: usize,
    /// Items an existing note already held byte for byte.
    pub deduplicated: usize,
    /// Rule 14: items past `extract_max_items_per_call`.
    pub dropped_over_cap: usize,
    /// Rule 6: items under `memory_confidence_threshold`.
    pub dropped_below_threshold: usize,
    /// Rules 2–4: no content, too long, an unknown kind, no row cited.
    pub dropped_invalid: usize,
    /// Rule 12: items left citing only forgotten rows.
    pub dropped_suppressed: usize,
    /// Items the service refused.
    pub failed: usize,
    /// Chunks with less of the user's own text than
    /// `extract_min_content_len`: advanced past with no call.
    pub skipped_short: usize,
    /// Chunks of a lane whose owner is not a `global_user` row: advanced
    /// past with no call.
    pub skipped_no_owner: usize,
    /// Chunks advanced past after `MAX_ATTEMPTS` unusable answers.
    pub skipped_unusable: usize,
    pub stopped: Option<FlushStop>,
}

impl FlushReport {
    fn add(&mut self, counts: &StoreCounts) {
        self.stored += counts.stored;
        self.active += counts.active;
        self.candidates += counts.candidates;
        self.deduplicated += counts.deduplicated;
        self.dropped_over_cap += counts.dropped_over_cap;
        self.dropped_suppressed += counts.dropped_suppressed;
        self.failed += counts.failed;
    }
}

/// Test seams inside the store-and-advance closure. Production uses
/// [`StoreHooks::default`]: a no-op and the advance.
#[derive(Clone)]
pub(crate) struct StoreHooks {
    /// Runs on the blocking thread after the store loop, before the advance.
    pub after_store_loop: Arc<dyn Fn() + Send + Sync>,
    /// Skips the advance: a crash between §1.4's steps 3 and 4.
    pub skip_advance: bool,
}

impl Default for StoreHooks {
    fn default() -> Self {
        Self {
            after_store_loop: Arc::new(|| {}),
            skip_advance: false,
        }
    }
}

/// The queue's clocks. Constants in production (`memory::learning`); a test
/// shortens them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timings {
    pub quiet: Duration,
    pub backoff_base: Duration,
    pub backoff_cap: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            quiet: RESUME_QUIET,
            backoff_base: BACKOFF_BASE,
            backoff_cap: BACKOFF_CAP,
        }
    }
}

/// How one flush runs.
#[derive(Debug, Clone, Copy)]
struct FlushPolicy {
    /// At most this many chunks; `None` runs to the window's end.
    max_chunks: Option<usize>,
    /// Whether the flush shares the model with live chat (§1.7): it checks
    /// for quiet before every chunk, yields a local model's call to a turn
    /// that starts, and leaves half of a keyed model's rate budget. An
    /// explicit flush (the harness) and the stop drain do not.
    shares_with_chat: bool,
}

impl FlushPolicy {
    const EXPLICIT: Self = Self {
        max_chunks: None,
        shares_with_chat: false,
    };
    const AUTOMATIC: Self = Self {
        max_chunks: Some(MAX_CHUNKS_PER_FLUSH),
        shares_with_chat: true,
    };
    const DRAIN: Self = Self {
        max_chunks: Some(MAX_CHUNKS_PER_FLUSH),
        shares_with_chat: false,
    };
}

#[derive(Debug, Default)]
struct SessionState {
    /// User-authored turns that finished since this session was last learned.
    pending_user_turns: u32,
    /// When the session's last turn finished (the idle trigger's clock).
    last_turn_at: Option<Instant>,
    /// A trigger fired: the session flushes at the next quiet start.
    due: bool,
    /// A flush of this session is running.
    running: bool,
    /// A held window waits until then (§1.4's backoff).
    retry_at: Option<Instant>,
    /// The delay the next hold waits; doubles to the cap.
    backoff: Option<Duration>,
}

#[derive(Debug)]
struct InFlight {
    lane_key: String,
    session_id: Option<String>,
    user_authored: bool,
}

#[derive(Debug)]
struct QueueState {
    sessions: HashMap<String, SessionState>,
    /// The session each lane's latest turn landed in.
    lanes: HashMap<String, String>,
    in_flight: HashMap<Uuid, InFlight>,
    /// When the in-flight set last became empty.
    quiet_since: Instant,
    /// Automatic flushes running: at most one on the daemon.
    flushing: usize,
    /// The running flush's local-model call: a turn that starts cancels it.
    local_call: Option<CancellationToken>,
    /// No model could answer the last call: one WARN when an outage starts,
    /// one INFO when learning resumes.
    outage: bool,
    /// The UTC date a cap's INFO was last logged for each session.
    cap_noticed: HashMap<String, String>,
}

/// The per-daemon learning queue. Build it with [`LearningQueue::new`] and
/// hand it to the gateway with `Gateway::with_learning`.
pub struct LearningQueue {
    deps: LearningDeps,
    mode: QueueMode,
    state: Mutex<QueueState>,
    /// Every flush the queue starts, tracked and cancellable.
    jobs: BackgroundJobs,
    hooks: Mutex<StoreHooks>,
    timings: Mutex<Timings>,
}

impl std::fmt::Debug for LearningQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LearningQueue")
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

/// The window as one blocking read saw it.
struct WindowRead {
    session: Option<Conversation>,
    cursor: LearningCursor,
    rows: Vec<LearningRow>,
    before: Vec<LearningRow>,
    /// Chat learning's calls and cost today (UTC), from `llm_usage_daily`.
    calls_today: i64,
    cost_today: f64,
}

fn read_window(db: &Database, session_id: &str) -> anyhow::Result<WindowRead> {
    let repo = ConversationRepository::new(db);
    let Some(session) = repo.get_session(session_id)? else {
        return Ok(WindowRead {
            session: None,
            cursor: LearningCursor::default(),
            rows: Vec::new(),
            before: Vec::new(),
            calls_today: 0,
            cost_today: 0.0,
        });
    };
    let cursor = LearningCursorRepository::new(db).get(session_id)?;
    let rows = repo.learning_window(session_id, cursor.extracted_through, WINDOW_READ_ROWS)?;
    let before = repo.learning_rows_before(session_id, cursor.extracted_through, 8)?;
    let (calls_today, cost_today) = LlmUsageRepository::new(db).usage_today(AGENT_LABEL)?;
    Ok(WindowRead {
        session: Some(session),
        cursor,
        rows,
        before,
        calls_today,
        cost_today,
    })
}

/// The owner is the user part of the lane key (design §1.1): a linked
/// connector user's lane carries their `global_id`.
fn lane_owner(lane_key: &str) -> &str {
    lane_key.rsplit_once(':').map_or(lane_key, |(user, _)| user)
}

/// Whether a base URL names this machine: `localhost`, `127.0.0.0/8` or
/// `::1` (ruling LR7's second clause).
fn is_loopback_url(base_url: &str) -> bool {
    let Ok(url) = url::Url::parse(base_url) else {
        return false;
    };
    match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Runs `f` on the blocking pool.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result,
        Err(e) => Err(anyhow::anyhow!("learning task failed: {e}")),
    }
}

impl LearningQueue {
    pub fn new(deps: LearningDeps, mode: QueueMode) -> Arc<Self> {
        Arc::new(Self {
            deps,
            mode,
            state: Mutex::new(QueueState {
                sessions: HashMap::new(),
                lanes: HashMap::new(),
                in_flight: HashMap::new(),
                quiet_since: Instant::now(),
                flushing: 0,
                local_call: None,
                outage: false,
                cap_noticed: HashMap::new(),
            }),
            jobs: BackgroundJobs::new(),
            hooks: Mutex::new(StoreHooks::default()),
            timings: Mutex::new(Timings::default()),
        })
    }

    pub fn mode(&self) -> QueueMode {
        self.mode
    }

    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn timings(&self) -> Timings {
        *self.timings.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[cfg(test)]
    pub(crate) fn set_store_hooks(&self, hooks: StoreHooks) {
        *self.hooks.lock().unwrap_or_else(|p| p.into_inner()) = hooks;
    }

    #[cfg(test)]
    pub(crate) fn set_timings(&self, timings: Timings) {
        *self.timings.lock().unwrap_or_else(|p| p.into_inner()) = timings;
    }

    /// The token every flush stops on.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.jobs.cancel_token()
    }

    // ------------------------------------------------------------------
    // The gateway's hook (design §1.5)
    // ------------------------------------------------------------------

    /// A turn begins (design §1.5): its request id is in flight **before**
    /// its user row exists, so a flush that reads the window between the
    /// insert and the reply stops before the row instead of learning a turn
    /// whose answer has not landed. On a local model it also cancels the
    /// running flush's call: the GPU serves the reply alone (§1.7). The
    /// returned guard ends the turn when it drops.
    ///
    /// `user_authored` is [`super::is_user_authored_turn`] of the turn: only
    /// such a turn counts towards the 10-turn trigger.
    pub fn turn_started(
        self: &Arc<Self>,
        request_id: Uuid,
        lane_key: &str,
        user_authored: bool,
    ) -> TurnGuard {
        let mut state = self.lock();
        state.in_flight.insert(
            request_id,
            InFlight {
                lane_key: lane_key.to_string(),
                session_id: None,
                user_authored,
            },
        );
        if let Some(call) = &state.local_call {
            call.cancel();
        }
        drop(state);
        TurnGuard {
            queue: Arc::clone(self),
            request_id,
        }
    }

    fn attach_session(&self, request_id: Uuid, session_id: &str) {
        let mut state = self.lock();
        let Some(turn) = state.in_flight.get_mut(&request_id) else {
            return;
        };
        turn.session_id = Some(session_id.to_string());
        let lane = turn.lane_key.clone();
        // A lane's turn landing in another session is the switch trigger for
        // the one it left (R48's project switch opens it right here).
        if let Some(previous) = state.lanes.insert(lane, session_id.to_string())
            && previous != session_id
            && let Some(left) = state.sessions.get_mut(&previous)
            && left.pending_user_turns > 0
        {
            left.due = true;
        }
    }

    fn turn_finished(&self, request_id: Uuid) {
        let turns_per_flush = self
            .deps
            .daemon_config
            .load()
            .orchestrator
            .learning
            .turns_per_flush
            .max(1);
        let mut state = self.lock();
        let Some(turn) = state.in_flight.remove(&request_id) else {
            return;
        };
        let now = Instant::now();
        if state.in_flight.is_empty() {
            state.quiet_since = now;
        }
        let Some(session_id) = turn.session_id else {
            // Nothing was persisted: there is nothing to learn.
            return;
        };
        let session = state.sessions.entry(session_id).or_default();
        session.last_turn_at = Some(now);
        if turn.user_authored {
            session.pending_user_turns = session.pending_user_turns.saturating_add(1);
            // MD-10's "10 new turns": due at once.
            if session.pending_user_turns >= turns_per_flush {
                session.due = true;
            }
        }
    }

    /// Whether this turn is in flight.
    pub fn is_in_flight(&self, request_id: Uuid) -> bool {
        self.lock().in_flight.contains_key(&request_id)
    }

    /// The session an in-flight turn was persisted in, once it was.
    pub fn in_flight_session(&self, request_id: Uuid) -> Option<String> {
        self.lock()
            .in_flight
            .get(&request_id)
            .and_then(|t| t.session_id.clone())
    }

    /// Turns in flight on the whole daemon.
    pub fn in_flight_count(&self) -> usize {
        self.lock().in_flight.len()
    }

    fn in_flight_ids(&self) -> HashSet<String> {
        self.lock()
            .in_flight
            .keys()
            .map(|id| id.to_string())
            .collect()
    }

    /// User-authored turns of a session that finished since it was last
    /// learned.
    pub fn pending_turns(&self, session_id: &str) -> u32 {
        self.lock()
            .sessions
            .get(session_id)
            .map_or(0, |s| s.pending_user_turns)
    }

    /// Whether a trigger made this session due.
    pub fn is_due(&self, session_id: &str) -> bool {
        self.lock().sessions.get(session_id).is_some_and(|s| s.due)
    }

    /// How long this session waits before it is retried, when a held window
    /// set a backoff.
    pub fn retry_in(&self, session_id: &str) -> Option<Duration> {
        let now = Instant::now();
        self.lock()
            .sessions
            .get(session_id)
            .and_then(|s| s.retry_at)
            .map(|at| at.saturating_duration_since(now))
    }

    /// Drops a session's queue state: its conversation was deleted or
    /// cleared, and its unlearned turns went with the messages (LQ4).
    pub fn forget_session(&self, session_id: &str) {
        let mut state = self.lock();
        if let Some(session) = state.sessions.get_mut(session_id) {
            if session.running {
                // The flush finds the session gone before it writes.
                session.pending_user_turns = 0;
                session.due = false;
            } else {
                state.sessions.remove(session_id);
            }
        }
        state.lanes.retain(|_, s| s != session_id);
        state.cap_noticed.remove(session_id);
    }

    // ------------------------------------------------------------------
    // Triggers (design §1.5)
    // ------------------------------------------------------------------

    /// One sweeper tick (every 15 s in the daemon): a session with unlearned
    /// turns whose last turn is `idle_secs` old becomes due; one that is no
    /// longer its lane's active session — "New chat", reopening an old
    /// conversation, a project switch — becomes due; one that no longer
    /// exists is forgotten (LQ4). Then a due session starts, if the daemon
    /// is quiet.
    pub async fn sweep(self: &Arc<Self>) {
        let idle = Duration::from_secs(
            self.deps
                .daemon_config
                .load()
                .orchestrator
                .learning
                .idle_secs,
        );
        let now = Instant::now();
        let watched: Vec<String> = {
            let mut state = self.lock();
            for session in state.sessions.values_mut() {
                if session.pending_user_turns > 0
                    && !idle.is_zero()
                    && session
                        .last_turn_at
                        .is_some_and(|at| now.duration_since(at) >= idle)
                {
                    session.due = true;
                }
            }
            state
                .sessions
                .iter()
                .filter(|(_, s)| s.pending_user_turns > 0 && !s.running)
                .map(|(id, _)| id.clone())
                .collect()
        };
        if !watched.is_empty() {
            let db = self.deps.db.clone();
            let ids = watched.clone();
            match blocking(move || {
                let repo = ConversationRepository::new(&db);
                let mut statuses = HashMap::new();
                for id in ids {
                    statuses.insert(id.clone(), repo.get_session(&id)?.map(|s| s.status));
                }
                Ok(statuses)
            })
            .await
            {
                Ok(statuses) => {
                    for (id, status) in statuses {
                        match status.as_deref() {
                            None => self.forget_session(&id),
                            Some(openalpaca_storage::SESSION_ARCHIVED) => {
                                if let Some(s) = self.lock().sessions.get_mut(&id) {
                                    s.due = true;
                                }
                            }
                            Some(_) => {}
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("memory learning: the sweeper could not read sessions: {e:#}")
                }
            }
        }
        self.try_start();
    }

    /// The boot catch-up, the durable half of the stop drain (design §1.5):
    /// every session with a user-authored row past its cursor is due, and
    /// the sessions are learned one at a time when the daemon is quiet.
    pub async fn catch_up(self: &Arc<Self>) {
        let db = self.deps.db.clone();
        let sessions = match blocking(move || {
            LearningCursorRepository::new(&db).sessions_with_unlearned_turns()
        })
        .await
        {
            Ok(sessions) => sessions,
            Err(e) => {
                tracing::warn!("memory learning: the boot catch-up could not read: {e:#}");
                return;
            }
        };
        if !sessions.is_empty() {
            tracing::info!(sessions = sessions.len(), "memory learning: catching up");
        }
        {
            let mut state = self.lock();
            for id in sessions {
                let session = state.sessions.entry(id).or_default();
                session.pending_user_turns = session.pending_user_turns.max(1);
                session.due = true;
            }
        }
        self.try_start();
    }

    /// Whether no turn is in flight and none ended in the last quiet period.
    fn is_quiet(&self) -> bool {
        let quiet = self.timings().quiet;
        let state = self.lock();
        state.in_flight.is_empty() && Instant::now().duration_since(state.quiet_since) >= quiet
    }

    /// Starts one due session's flush when the daemon is quiet and no flush
    /// runs (`MAX_CONCURRENT_FLUSHES` = 1). Automatic mode only.
    fn try_start(self: &Arc<Self>) {
        if self.mode != QueueMode::Automatic
            || !self.deps.daemon_config.load().orchestrator.learning.enabled
            || self.jobs.cancel_token().is_cancelled()
            || !self.is_quiet()
        {
            return;
        }
        let now = Instant::now();
        let session_id = {
            let mut state = self.lock();
            if state.flushing > 0 {
                return;
            }
            let Some(id) = state
                .sessions
                .iter()
                .filter(|(_, s)| s.due && !s.running && s.retry_at.is_none_or(|at| at <= now))
                .min_by_key(|(_, s)| s.last_turn_at)
                .map(|(id, _)| id.clone())
            else {
                return;
            };
            state.flushing += 1;
            id
        };
        let queue = Arc::clone(self);
        self.jobs.spawn(async move {
            let slot = FlushSlot(Arc::clone(&queue));
            let report = queue.run_flush(&session_id, FlushPolicy::AUTOMATIC).await;
            drop(slot);
            // The next due session — the boot catch-up's sessions go one at a
            // time — starts now only after progress. A flush that stopped
            // with none (a cap, the rate headroom, a busy daemon) waits for
            // the next sweep, or it would restart itself in a loop.
            if report.stopped.is_none() || report.chunks > 0 {
                queue.try_start();
            }
        });
    }

    // ------------------------------------------------------------------
    // Stop (design §1.5)
    // ------------------------------------------------------------------

    /// The stop drain: every session with unlearned turns is due, and they
    /// are flushed one at a time, ignoring the quiet rule — the server has
    /// already stopped — for at most `budget`. `true` when the drain
    /// finished; what it did not finish, the boot catch-up learns.
    pub async fn drain(self: &Arc<Self>, budget: Duration) -> bool {
        let sessions: Vec<String> = {
            let mut state = self.lock();
            let mut ids: Vec<(Option<Instant>, String)> = Vec::new();
            for (id, session) in state.sessions.iter_mut() {
                if session.pending_user_turns > 0 || session.due {
                    session.due = true;
                    ids.push((session.last_turn_at, id.clone()));
                }
            }
            ids.sort();
            ids.into_iter().map(|(_, id)| id).collect()
        };
        if sessions.is_empty() {
            return true;
        }
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let queue = Arc::clone(self);
        self.jobs.spawn(async move {
            for session_id in sessions {
                if queue.jobs.cancel_token().is_cancelled() {
                    return;
                }
                queue.run_flush(&session_id, FlushPolicy::DRAIN).await;
            }
            let _ = done_tx.send(());
        });
        matches!(tokio::time::timeout(budget, done_rx).await, Ok(Ok(())))
    }

    /// Cancels every flush and waits up to `grace` for them to stop. A
    /// chunk's store loop already running on the blocking pool finishes with
    /// its cursor advance; nothing else is written. `true` when they all did.
    pub async fn shutdown(&self, grace: Duration) -> bool {
        self.jobs.shutdown(grace).await
    }

    /// Waits until no flush the queue started is running.
    pub async fn wait_idle(&self) {
        self.jobs.wait_idle().await;
    }

    // ------------------------------------------------------------------
    // Flushing
    // ------------------------------------------------------------------

    /// Learns a session's whole window now: every chunk in order until the
    /// cursor reaches its last row, with no chunk cap and no quiet rule
    /// (design §7.2). One flush per session: a second call while one runs
    /// returns at once with [`FlushStop::AlreadyRunning`].
    pub async fn flush_session(&self, session_id: &str) -> FlushReport {
        self.run_flush(session_id, FlushPolicy::EXPLICIT).await
    }

    /// [`Self::flush_session`] for every session with unlearned turns, one
    /// at a time.
    pub async fn flush_all(&self) -> Vec<(String, FlushReport)> {
        let db = self.deps.db.clone();
        let sessions =
            blocking(move || LearningCursorRepository::new(&db).sessions_with_unlearned_turns())
                .await
                .unwrap_or_default();
        let mut reports = Vec::with_capacity(sessions.len());
        for id in sessions {
            let report = self.flush_session(&id).await;
            reports.push((id, report));
        }
        reports
    }

    /// An automatic flush of one session, now: what the sweeper starts.
    #[cfg(test)]
    pub(crate) async fn flush_automatic(&self, session_id: &str) -> FlushReport {
        self.run_flush(session_id, FlushPolicy::AUTOMATIC).await
    }

    async fn run_flush(&self, session_id: &str, policy: FlushPolicy) -> FlushReport {
        let Some(running) = RunningFlush::begin(self, session_id) else {
            return FlushReport {
                stopped: Some(FlushStop::AlreadyRunning),
                ..Default::default()
            };
        };
        let report = self.flush_chunks(session_id, policy).await;
        running.finish(&report);
        tracing::info!(
            session_id,
            calls = report.calls,
            chunks = report.chunks,
            stored = report.stored,
            active = report.active,
            candidates = report.candidates,
            deduplicated = report.deduplicated,
            dropped_over_cap = report.dropped_over_cap,
            dropped_below_threshold = report.dropped_below_threshold,
            dropped_invalid = report.dropped_invalid,
            dropped_suppressed = report.dropped_suppressed,
            failed = report.failed,
            skipped_short = report.skipped_short,
            skipped_no_owner = report.skipped_no_owner,
            skipped_unusable = report.skipped_unusable,
            input_tokens = report.input_tokens,
            output_tokens = report.output_tokens,
            stopped = ?report.stopped,
            "memory learning finished"
        );
        report
    }

    /// Whether the model a flush calls runs on this machine (ruling LR7): a
    /// provider that needs no key, or one whose base URL is loopback — a
    /// llama.cpp or LM Studio server behind the `openai` provider.
    fn is_local_model(&self, model: &str) -> bool {
        let router = &self.deps.router;
        let Some(provider) = router.model_registry().resolve_provider(model) else {
            return false;
        };
        if router.provider_requires_key(&provider) == Some(false) {
            return true;
        }
        router
            .runtime_config()
            .provider_defaults
            .get(&provider.to_string())
            .and_then(|d| d.base_url.as_deref())
            .is_some_and(is_loopback_url)
    }

    async fn flush_chunks(&self, session_id: &str, policy: FlushPolicy) -> FlushReport {
        let mut report = FlushReport::default();
        let cancel = self.jobs.cancel_token();
        loop {
            if !self.deps.daemon_config.load().orchestrator.learning.enabled {
                report.stopped = Some(FlushStop::Disabled);
                return report;
            }
            if policy.max_chunks.is_some_and(|max| report.chunks >= max) {
                report.stopped = Some(FlushStop::ChunkLimit);
                return report;
            }
            if cancel.is_cancelled() {
                report.stopped = Some(FlushStop::Cancelled);
                return report;
            }
            // Before every chunk, not only the first: a turn that started
            // mid-flush stops it at the chunk boundary (§1.7).
            if policy.shares_with_chat && !self.is_quiet() {
                report.stopped = Some(FlushStop::Busy);
                return report;
            }

            // Read the window. A turn in flight when the read starts, or
            // still in flight when it ends, ends the window before its row.
            let mut in_flight = self.in_flight_ids();
            let db = self.deps.db.clone();
            let owned_id = session_id.to_string();
            let read = match blocking(move || read_window(&db, &owned_id)).await {
                Ok(read) => read,
                Err(e) => {
                    tracing::warn!(
                        session_id,
                        "memory learning: the window could not be read: {e:#}"
                    );
                    report.stopped = Some(FlushStop::StoreError);
                    return report;
                }
            };
            in_flight.extend(self.in_flight_ids());
            let Some(session) = read.session else {
                self.forget_session(session_id);
                report.stopped = Some(FlushStop::SessionGone);
                return report;
            };

            let dcfg = self.deps.daemon_config.load();
            let model = dcfg
                .orchestrator
                .costs
                .extraction_model
                .clone()
                .filter(|m| !m.trim().is_empty());
            let answering = crate::orchestrator::attachment_adapt::answering_model(
                &self.deps.router,
                model.as_deref(),
            );
            let local = answering
                .as_ref()
                .is_some_and(|a| self.is_local_model(&a.id));
            // A keyed provider's rate budget caps the chunk too (§1.6).
            let headroom = match (&answering, local) {
                (Some(a), false) => self.deps.router.rate_headroom(&a.id).await,
                _ => None,
            };
            let budget = Budget::for_window(
                answering.as_ref().and_then(|a| a.window),
                dcfg.orchestrator.costs.extract_max_items_per_call,
                headroom.map(|h| h.tpm_capacity as u32),
            );
            let chunk = match window::next_chunk(
                &read.rows,
                &read.before,
                &in_flight,
                budget.transcript_bytes,
                WINDOW_READ_ROWS as usize,
            ) {
                NextChunk::Empty => return report,
                NextChunk::Blocked => {
                    report.stopped = Some(FlushStop::InFlight);
                    return report;
                }
                NextChunk::Chunk(chunk) => chunk,
            };
            let expected = read.cursor.extracted_through;

            // The minimum, per chunk: too little of the user's own text is
            // not worth a call, and the cursor moves past it.
            if chunk.users_text_bytes() < dcfg.orchestrator.costs.extract_min_content_len {
                self.advance(session_id, chunk.through, expected).await;
                report.chunks += 1;
                report.skipped_short += 1;
                continue;
            }

            // The owner must be a `global_user` row (design §3): anything
            // else learns nothing, and its cursor advances.
            let root = chunk.root.as_ref().map(PathBuf::from);
            let hint = match &root {
                Some(r) => WorkspaceHint::Root(r.as_path()),
                None => WorkspaceHint::None,
            };
            let ctx = match self.deps.memory_service.context(
                lane_owner(&session.lane_key),
                hint,
                Surface::Extraction,
                Capabilities::EXTRACTION,
                Attribution {
                    lane_key: Some(session.lane_key.clone()),
                    task_id: None,
                    request_id: chunk
                        .last_request_id()
                        .and_then(|r| Uuid::parse_str(r).ok()),
                    source: Some(session.source.clone()),
                },
            ) {
                Ok(ctx) => ctx,
                Err(MemoryError::NoOwner(_)) => {
                    self.advance(session_id, chunk.through, expected).await;
                    report.chunks += 1;
                    report.skipped_no_owner += 1;
                    continue;
                }
                Err(e) => {
                    tracing::warn!(session_id, "memory learning: context refused: {e}");
                    report.stopped = Some(FlushStop::StoreError);
                    return report;
                }
            };

            // A quarantined partition holds the window before any model
            // call: no cost is spent on a chunk that cannot be written.
            let service = self.deps.memory_service.clone();
            let preflight_ctx = ctx.clone();
            match tokio::task::spawn_blocking(move || service.learning_preflight(&preflight_ctx))
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(MemoryError::Quarantined { partition_key, .. })) => {
                    tracing::warn!(
                        session_id,
                        %partition_key,
                        "memory learning waits: the partition is quarantined; run `openalpaca memory doctor --repair`"
                    );
                    report.stopped = Some(FlushStop::Quarantined);
                    return report;
                }
                Ok(Err(e)) => {
                    tracing::warn!(session_id, "memory learning: pre-flight failed: {e}");
                    report.stopped = Some(FlushStop::StoreError);
                    return report;
                }
                Err(e) => {
                    tracing::warn!(session_id, "memory learning: pre-flight task failed: {e}");
                    report.stopped = Some(FlushStop::StoreError);
                    return report;
                }
            }

            // The durable daily gates (§1.8, ruling LR8): today's UTC calls
            // and cost, read from `llm_usage_daily`. A binding cap leaves the
            // window queued; learning lags and loses nothing.
            let max_calls = i64::from(dcfg.orchestrator.costs.extract_max_calls_per_day);
            let max_cost = dcfg.orchestrator.costs.extract_max_daily_cost_usd;
            if (max_calls > 0 && read.calls_today >= max_calls) || read.cost_today > max_cost {
                self.notice_cap(session_id, read.calls_today, read.cost_today);
                report.stopped = Some(FlushStop::Cap);
                return report;
            }

            // Build the call, then check the keyed model's rate headroom: a
            // chunk starts only when half of each bucket would remain after
            // its own estimate (§1.7) — the router's estimate, bytes ÷ 4.
            let (transcript, numbered) = chunk.transcript();
            let request =
                extractor::build_request(&transcript, budget.items, budget.out_tokens, model);
            let threshold = dcfg.orchestrator.memory.memory_confidence_threshold;
            drop(dcfg);
            if policy.shares_with_chat
                && let Some(h) = headroom
            {
                let estimate = (request
                    .messages
                    .iter()
                    .map(|m| m.content.len())
                    .sum::<usize>()
                    / 4)
                .max(100) as f64;
                if h.rpm_available - 1.0 < h.rpm_capacity / 2.0
                    || h.tpm_available - estimate < h.tpm_capacity / 2.0
                {
                    report.stopped = Some(FlushStop::RateHeadroom);
                    return report;
                }
            }

            // Ask the model. On a local model a turn that starts cancels
            // this call: nothing is written and nothing is counted.
            let local_call = (policy.shares_with_chat && local).then(|| {
                let token = cancel.child_token();
                self.lock().local_call = Some(token.clone());
                token
            });
            let yielded = async {
                match &local_call {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending().await,
                }
            };
            let started = std::time::Instant::now();
            let response = tokio::select! {
                biased;
                _ = cancel.cancelled() => None,
                _ = yielded => {
                    self.lock().local_call = None;
                    report.stopped = Some(FlushStop::Yielded);
                    return report;
                }
                result = self.deps.router.complete(request) => Some(result),
            };
            if local_call.is_some() {
                self.lock().local_call = None;
            }
            let Some(response) = response else {
                report.stopped = Some(FlushStop::Cancelled);
                return report;
            };
            let latency_ms = started.elapsed().as_millis() as i64;
            let response = match response {
                Ok(r) => r,
                // An answer the reasoning ate is a reachable model's bad
                // output, not an outage (M2, ruling LR20).
                Err(LlmRouterError::Llm(e @ LlmError::EmptyCompletion { .. })) => {
                    self.note_available();
                    report.calls += 1;
                    if self
                        .count_failure(session_id, &chunk, expected, &e.to_string(), &mut report)
                        .await
                    {
                        continue;
                    }
                    return report;
                }
                Err(e) => {
                    // §7b P10: the window holds, nothing is counted, and the
                    // outage is logged once, not once per retry.
                    self.note_unavailable(&e);
                    report.stopped = Some(FlushStop::Unavailable);
                    return report;
                }
            };
            self.note_available();
            report.calls += 1;
            report.input_tokens += u64::from(response.usage.input_tokens);
            report.output_tokens += u64::from(response.usage.output_tokens);

            let parsed = extractor::parse_items(&response.content);
            self.record_usage(&response, latency_ms, parsed.as_ref().err());
            let items = match parsed {
                Ok(items) => items,
                Err(why) => {
                    if self
                        .count_failure(session_id, &chunk, expected, &why, &mut report)
                        .await
                    {
                        continue;
                    }
                    return report;
                }
            };
            let validated = extractor::validate(&items, &numbered, threshold);
            report.dropped_invalid += validated.invalid;
            report.dropped_below_threshold += validated.below_threshold;

            // Store the chunk's notes and advance its cursor in one blocking
            // task (§1.4): dropping this future does not separate them.
            let job = StoreJob {
                db: self.deps.db.clone(),
                service: self.deps.memory_service.clone(),
                ctx,
                session_id: session_id.to_string(),
                items: validated.items,
                max_items: budget.items,
                through: chunk.through,
                expected,
                hooks: self.hooks.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            };
            let (counts, end) = match tokio::task::spawn_blocking(move || {
                extractor::store_and_advance(job)
            })
            .await
            {
                Ok(done) => done,
                Err(e) => (
                    StoreCounts::default(),
                    StoreEnd::Held(format!("the store task failed: {e}")),
                ),
            };
            report.add(&counts);
            match end {
                StoreEnd::Advanced | StoreEnd::CursorMoved => report.chunks += 1,
                StoreEnd::SessionGone => {
                    self.forget_session(session_id);
                    report.stopped = Some(FlushStop::SessionGone);
                    return report;
                }
                StoreEnd::Held(why) => {
                    tracing::warn!(session_id, "memory learning holds the window: {why}");
                    report.stopped = Some(FlushStop::StoreError);
                    return report;
                }
            }
        }
    }

    /// Counts one unusable answer against the chunk (ruling LR20). After
    /// `MAX_ATTEMPTS` of them the cursor moves past the chunk with one WARN,
    /// and `true` lets the flush go on to the next chunk; otherwise the
    /// window holds (`false`).
    async fn count_failure(
        &self,
        session_id: &str,
        chunk: &window::Chunk,
        expected: i64,
        error: &str,
        report: &mut FlushReport,
    ) -> bool {
        let db = self.deps.db.clone();
        let id = session_id.to_string();
        let message = error.to_string();
        let attempts = blocking(move || {
            LearningCursorRepository::new(&db).record_failure(&id, expected, &message)
        })
        .await;
        let attempts = match attempts {
            Ok(Some(n)) => n,
            Ok(None) => {
                // The cursor moved or the session went: nothing to count.
                report.stopped = Some(FlushStop::BadOutput);
                return false;
            }
            Err(e) => {
                tracing::warn!(
                    session_id,
                    "memory learning: the attempt was not counted: {e:#}"
                );
                report.stopped = Some(FlushStop::BadOutput);
                return false;
            }
        };
        if attempts < MAX_ATTEMPTS {
            tracing::info!(
                session_id,
                attempts,
                "memory learning: the model's answer was unusable, retrying later: {error}"
            );
            report.stopped = Some(FlushStop::BadOutput);
            return false;
        }
        let first = expected + 1;
        tracing::warn!(
            session_id,
            first_row = first,
            last_row = chunk.through,
            attempts,
            "memory learning skipped rows {first}..={} after {attempts} unusable answers: {error}",
            chunk.through
        );
        self.advance(session_id, chunk.through, expected).await;
        report.chunks += 1;
        report.skipped_unusable += 1;
        true
    }

    fn note_unavailable(&self, error: &LlmRouterError) {
        let mut state = self.lock();
        if !state.outage {
            state.outage = true;
            drop(state);
            tracing::warn!(
                "memory learning paused: no model can answer ({error}); the unlearned turns wait and nothing is lost"
            );
        } else {
            tracing::debug!("memory learning still waiting for a model: {error}");
        }
    }

    fn note_available(&self) {
        let mut state = self.lock();
        if state.outage {
            state.outage = false;
            drop(state);
            tracing::info!("memory learning resumed: a model answered again");
        }
    }

    /// One INFO per session per UTC day when a cap binds.
    fn notice_cap(&self, session_id: &str, calls: i64, cost: f64) {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let mut state = self.lock();
        if state.cap_noticed.get(session_id) == Some(&today) {
            return;
        }
        state.cap_noticed.insert(session_id.to_string(), today);
        drop(state);
        tracing::info!(
            session_id,
            calls_today = calls,
            cost_today = cost,
            "memory learning waits for tomorrow (UTC): a daily cap is reached; nothing is lost"
        );
    }

    /// Moves the cursor past a chunk that needs no call.
    async fn advance(&self, session_id: &str, through: i64, expected: i64) {
        let db = self.deps.db.clone();
        let id = session_id.to_string();
        if let Err(e) =
            blocking(move || LearningCursorRepository::new(&db).advance_cas(&id, through, expected))
                .await
        {
            tracing::warn!(
                session_id,
                "memory learning: the cursor did not move: {e:#}"
            );
        }
    }

    /// Records the call like every other internal call (`record_and_log`),
    /// which is also what the durable daily gates read.
    fn record_usage(
        &self,
        response: &openalpaca_llm::ChatResponse,
        latency_ms: i64,
        parse_error: Option<&String>,
    ) {
        let model = response.model.as_str();
        let provider = self
            .deps
            .router
            .model_registry()
            .resolve_provider(model)
            .map(|p| p.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let cost = self.deps.router.cost_tracker.calculate_cost(
            model,
            response.usage.input_tokens,
            response.usage.output_tokens,
        );
        let (status, error) = match parse_error {
            Some(_) => ("error", Some("JSON parse failure")),
            None => ("success", None),
        };
        if let Err(e) = LlmUsageRepository::new(&self.deps.db).record_and_log(
            AGENT_LABEL,
            None,
            &provider,
            model,
            response.usage.input_tokens as i32,
            response.usage.output_tokens as i32,
            cost,
            latency_ms,
            status,
            error,
        ) {
            tracing::warn!("Failed to persist memory learning usage: {e}");
        }
    }
}

/// The daemon's one automatic flush slot; dropping it frees the slot.
struct FlushSlot(Arc<LearningQueue>);

impl Drop for FlushSlot {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.flushing = state.flushing.saturating_sub(1);
    }
}

/// One session's running flush. Dropping it — the flush finished, or its
/// future was dropped — marks the session free again.
struct RunningFlush<'a> {
    queue: &'a LearningQueue,
    session_id: String,
    pending_at_start: u32,
}

impl<'a> RunningFlush<'a> {
    fn begin(queue: &'a LearningQueue, session_id: &str) -> Option<Self> {
        let mut state = queue.lock();
        let session = state.sessions.entry(session_id.to_string()).or_default();
        if session.running {
            return None;
        }
        session.running = true;
        let pending_at_start = session.pending_user_turns;
        Some(Self {
            queue,
            session_id: session_id.to_string(),
            pending_at_start,
        })
    }

    /// The session's bookkeeping once the flush ends. A flush that learned
    /// the window to its end has learned every turn it saw pending, and the
    /// session is no longer due; turns that finished while it ran stay
    /// pending. A held window waits out a doubling backoff; anything else
    /// that stopped early stays due.
    fn finish(self, report: &FlushReport) {
        let timings = self.queue.timings();
        let mut state = self.queue.lock();
        let Some(session) = state.sessions.get_mut(&self.session_id) else {
            return;
        };
        match report.stopped {
            None => {
                session.pending_user_turns = session
                    .pending_user_turns
                    .saturating_sub(self.pending_at_start);
                session.due = false;
                session.retry_at = None;
                session.backoff = None;
            }
            Some(stop) if stop.backs_off() => {
                let delay = session.backoff.unwrap_or(timings.backoff_base);
                session.retry_at = Some(Instant::now() + delay);
                session.backoff = Some((delay * 2).min(timings.backoff_cap));
                session.due = true;
            }
            Some(FlushStop::SessionGone | FlushStop::Disabled | FlushStop::AlreadyRunning) => {}
            Some(_) => session.due = true,
        }
    }
}

impl Drop for RunningFlush<'_> {
    fn drop(&mut self) {
        let mut state = self.queue.lock();
        if let Some(session) = state.sessions.get_mut(&self.session_id) {
            session.running = false;
        }
    }
}

/// One turn's membership of the in-flight set. Dropping it ends the turn —
/// after the gateway wrote the assistant row, or on any earlier exit.
#[derive(Debug)]
pub struct TurnGuard {
    queue: Arc<LearningQueue>,
    request_id: Uuid,
}

impl TurnGuard {
    /// Records the session persistence resolved for the turn.
    pub fn attach_session(&self, session_id: &str) {
        self.queue.attach_session(self.request_id, session_id);
    }
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.queue.turn_finished(self.request_id);
    }
}

#[cfg(test)]
mod tests {
    use super::is_loopback_url;

    #[test]
    fn loopback_urls_are_local() {
        for url in [
            "http://localhost:1234/v1",
            "http://LOCALHOST/v1",
            "http://127.0.0.1:8080/v1",
            "http://127.1.2.3/v1",
            "http://[::1]:11434/v1",
        ] {
            assert!(is_loopback_url(url), "{url}");
        }
        for url in [
            "https://api.openai.com/v1",
            "http://10.0.0.5/v1",
            "not a url",
        ] {
            assert!(!is_loopback_url(url), "{url}");
        }
    }
}
