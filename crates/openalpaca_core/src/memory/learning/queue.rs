//! The learning queue (design 09 §1.5).
//!
//! In memory it keeps three things: per session, the user-authored turns not
//! yet learned and when the last turn landed; per lane, the current session;
//! and the set of turns in flight. The gateway feeds it through a
//! [`TurnGuard`]: [`LearningQueue::turn_started`] before the user row is
//! persisted, [`TurnGuard::attach_session`] once persistence resolves the
//! session, and the guard's drop after the assistant row is written — or on
//! any earlier exit. Each is one `std::sync::Mutex` update: no await, no
//! database, no model.
//!
//! A **flush** learns a session's window chunk by chunk (§1.6): read the
//! window, ask the model, then store the chunk's notes and advance the
//! durable cursor in one blocking task (§1.4). Delivery is at-least-once: a
//! crash between the writes and the advance repeats the chunk, and the
//! guards of §1.4 make the repeat harmless.

use super::extractor::{self, AGENT_LABEL, StoreCounts, StoreEnd, StoreJob};
use super::window::{self, NextChunk};
use super::{Budget, WINDOW_READ_ROWS};
use crate::daemon_config::DaemonConfig;
use crate::memory::service::{
    Attribution, Capabilities, MemoryError, MemoryService, Surface, WorkspaceHint,
};
use crate::orchestrator::BackgroundJobs;
use arc_swap::ArcSwap;
use openalpaca_llm::LlmRouter;
use openalpaca_storage::repository::LlmUsageRepository;
use openalpaca_storage::{
    Conversation, ConversationRepository, Database, LearningCursor, LearningCursorRepository,
    LearningRow,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::time::Instant;
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
    /// The queue's shutdown token fired before the model answered.
    Cancelled,
    /// No model could answer (§7b P10). The window holds.
    Unavailable,
    /// A reachable model answered with something that is not
    /// `{"items": [...]}`. The window holds.
    BadOutput,
    /// A partition the chunk would write to is quarantined (design §2).
    Quarantined,
    /// A daily cap binds (§1.8). The window stays queued.
    Cap,
    /// A note write failed on I/O. The window holds.
    StoreError,
    /// The session was deleted (LQ4): its unlearned turns went with it.
    SessionGone,
    /// The flush ran its chunk budget; the rest stays due.
    ChunkLimit,
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

/// How one flush runs.
#[derive(Debug, Clone, Copy)]
struct FlushPolicy {
    /// At most this many chunks; `None` runs to the window's end.
    max_chunks: Option<usize>,
}

#[derive(Debug, Default)]
struct SessionState {
    /// User-authored turns that finished since this session was last learned.
    pending_user_turns: u32,
    /// When the session's last turn finished (the idle trigger's clock).
    last_turn_at: Option<Instant>,
    /// A flush of this session is running.
    running: bool,
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
}

fn read_window(db: &Database, session_id: &str) -> anyhow::Result<WindowRead> {
    let repo = ConversationRepository::new(db);
    let Some(session) = repo.get_session(session_id)? else {
        return Ok(WindowRead {
            session: None,
            cursor: LearningCursor::default(),
            rows: Vec::new(),
            before: Vec::new(),
        });
    };
    let cursor = LearningCursorRepository::new(db).get(session_id)?;
    let rows = repo.learning_window(session_id, cursor.extracted_through, WINDOW_READ_ROWS)?;
    let before = repo.learning_rows_before(session_id, cursor.extracted_through, 8)?;
    Ok(WindowRead {
        session: Some(session),
        cursor,
        rows,
        before,
    })
}

/// The owner is the user part of the lane key (design §1.1): a linked
/// connector user's lane carries their `global_id`.
fn lane_owner(lane_key: &str) -> &str {
    lane_key.rsplit_once(':').map_or(lane_key, |(user, _)| user)
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
            }),
            jobs: BackgroundJobs::new(),
            hooks: Mutex::new(StoreHooks::default()),
        })
    }

    pub fn mode(&self) -> QueueMode {
        self.mode
    }

    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[cfg(test)]
    pub(crate) fn set_store_hooks(&self, hooks: StoreHooks) {
        *self.hooks.lock().unwrap_or_else(|p| p.into_inner()) = hooks;
    }

    /// The token every flush stops on.
    pub fn shutdown_token(&self) -> tokio_util::sync::CancellationToken {
        self.jobs.cancel_token()
    }

    // ------------------------------------------------------------------
    // The gateway's hook (design §1.5)
    // ------------------------------------------------------------------

    /// A turn begins (design §1.5): its request id is in flight **before**
    /// its user row exists, so a flush that reads the window between the
    /// insert and the reply stops before the row instead of learning a turn
    /// whose answer has not landed. The returned guard ends the turn when it
    /// drops.
    ///
    /// `user_authored` is [`super::is_user_authored_turn`] of the turn: only
    /// such a turn counts towards the 10-turn trigger.
    pub fn turn_started(
        self: &Arc<Self>,
        request_id: Uuid,
        lane_key: &str,
        user_authored: bool,
    ) -> TurnGuard {
        self.lock().in_flight.insert(
            request_id,
            InFlight {
                lane_key: lane_key.to_string(),
                session_id: None,
                user_authored,
            },
        );
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
        state.lanes.insert(lane, session_id.to_string());
    }

    fn turn_finished(&self, request_id: Uuid) {
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

    /// Drops a session's queue state: its conversation was deleted or
    /// cleared, and its unlearned turns went with the messages (LQ4).
    pub fn forget_session(&self, session_id: &str) {
        let mut state = self.lock();
        if let Some(session) = state.sessions.get_mut(session_id) {
            if session.running {
                // The flush finds the session gone before it writes.
                session.pending_user_turns = 0;
            } else {
                state.sessions.remove(session_id);
            }
        }
        state.lanes.retain(|_, s| s != session_id);
    }

    // ------------------------------------------------------------------
    // Flushing
    // ------------------------------------------------------------------

    /// Learns a session's whole window now: every chunk in order until the
    /// cursor reaches its last row, with no chunk cap and no quiet rule
    /// (design §7.2). One flush per session: a second call while one runs
    /// returns at once with [`FlushStop::AlreadyRunning`].
    pub async fn flush_session(&self, session_id: &str) -> FlushReport {
        self.run_flush(session_id, FlushPolicy { max_chunks: None })
            .await
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
            input_tokens = report.input_tokens,
            output_tokens = report.output_tokens,
            stopped = ?report.stopped,
            "memory learning finished"
        );
        report
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
            let budget = Budget::for_window(
                answering.as_ref().and_then(|a| a.window),
                dcfg.orchestrator.costs.extract_max_items_per_call,
                None,
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
                    tracing::warn!(session_id, %partition_key, "memory learning waits: the partition is quarantined; run `openalpaca memory doctor --repair`");
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

            // The daily cost cap.
            let spent = self
                .deps
                .router
                .cost_tracker
                .get_agent_usage(AGENT_LABEL)
                .await
                .map(|s| s.total_cost_usd)
                .unwrap_or(0.0);
            if spent > dcfg.orchestrator.costs.extract_max_daily_cost_usd {
                report.stopped = Some(FlushStop::Cap);
                return report;
            }

            // Ask the model.
            let (transcript, numbered) = chunk.transcript();
            let request =
                extractor::build_request(&transcript, budget.items, budget.out_tokens, model);
            let threshold = dcfg.orchestrator.memory.memory_confidence_threshold;
            drop(dcfg);
            let started = std::time::Instant::now();
            let response = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    report.stopped = Some(FlushStop::Cancelled);
                    return report;
                }
                result = self.deps.router.complete(request) => result,
            };
            let latency_ms = started.elapsed().as_millis() as i64;
            let response = match response {
                Ok(r) => r,
                Err(e) => {
                    // §7b P10: the window holds and the turn was never
                    // affected.
                    tracing::warn!(session_id, "memory learning unavailable: {e}");
                    report.stopped = Some(FlushStop::Unavailable);
                    return report;
                }
            };
            report.calls += 1;
            report.input_tokens += u64::from(response.usage.input_tokens);
            report.output_tokens += u64::from(response.usage.output_tokens);

            let parsed = extractor::parse_items(&response.content);
            self.record_usage(&response, latency_ms, parsed.as_ref().err());
            let items = match parsed {
                Ok(items) => items,
                Err(why) => {
                    tracing::warn!(
                        session_id,
                        "memory learning: the model's answer was unusable: {why}"
                    );
                    report.stopped = Some(FlushStop::BadOutput);
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

    /// A flush that learned the window to its end has learned every turn it
    /// saw pending; turns that finished while it ran stay pending.
    fn finish(self, report: &FlushReport) {
        if report.stopped.is_none() {
            let mut state = self.queue.lock();
            if let Some(session) = state.sessions.get_mut(&self.session_id) {
                session.pending_user_turns = session
                    .pending_user_turns
                    .saturating_sub(self.pending_at_start);
            }
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
