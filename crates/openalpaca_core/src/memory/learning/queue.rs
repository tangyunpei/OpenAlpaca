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
//! **A conversation's close** (design 11 §2, owner decision CQ-2) rides the
//! same queue with triggers of its own: a quiet period of 30 minutes, a
//! switch, the stop drain and its boot catch-up — never the two-minute idle
//! nor the ten turns. It brings the conversation's working state up to date
//! — one summariser call, only when a user turn lies past what the state
//! covers — and then, once four of the user's turns have accumulated since
//! the session's last summary (its durable episode cursor), writes a
//! **conversation summary** (design 09 §13.3, MD-16) with a call of its own
//! over the rows past that cursor (owner decision CQ-11, `episode.rs`). Both
//! calls are made on `summary_model` under the summariser's one daily
//! ceiling and learning's yielding to chat, each under its own label. A
//! session's facts are learned before it is closed.
//!
//! A **flush** learns a session's window chunk by chunk (§1.6): read the
//! window, ask the model, then store the chunk's notes and advance the
//! durable cursor in one blocking task (§1.4). Delivery is at-least-once: a
//! crash between the writes and the advance repeats the chunk, and the
//! guards of §1.4 make the repeat harmless. Before every chunk an automatic
//! flush checks again that chat is quiet; on a local model a turn that starts
//! cancels the call (the GPU serves the reply alone), and on a keyed remote
//! model a chunk starts only when half of the key's rate budget would remain.

use super::episode::{self, EPISODE_LABEL, EpisodeJob, EpisodeWrite};
use super::extractor::{
    self, AGENT_LABEL, ProjectCheck, PromptInput, StoreCounts, StoreEnd, StoreJob,
};
use super::lookup;
use super::window::{self, NextChunk};
use super::{
    BACKOFF_BASE, BACKOFF_CAP, Budget, EPISODE_MIN_USER_TURNS, EPISODE_OUT_TOKENS, EPISODE_QUIET,
    MAX_ATTEMPTS, MAX_CHUNKS_PER_FLUSH, RESUME_QUIET, UNUSABLE_EXCERPT_CHARS, WINDOW_READ_ROWS,
};
use crate::daemon_config::DaemonConfig;
use crate::memory::service::{
    Attribution, Capabilities, MemoryAccessContext, MemoryError, MemoryService, Surface,
    WorkspaceHint,
};
use crate::orchestrator::BackgroundJobs;
use crate::orchestrator::working_state::{self, TranscriptLine, WorkingState};
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
    /// A note write failed on I/O, or the chunk's project is being
    /// re-pointed. The window holds, with backoff.
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
    /// Candidates: `extracted_other` notes and suggestions.
    pub candidates: usize,
    /// Among `active`: automatic replacements of a learned note (MD-11).
    pub replaced: usize,
    /// Among `candidates`: suggestions (MD-11) — a change in the user's
    /// words to a note they saved, edited or imported, and an inferred
    /// change to any note — each naming the note it would replace, which
    /// stays as it is until the user confirms one.
    pub suggestions: usize,
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
    /// Rule 11: a second item with the same key or the same target.
    pub dropped_duplicate_key: usize,
    /// Rule 13: what this chunk already taught (a re-run).
    pub dropped_already_learned: usize,
    /// §2.5: an item that only restates the note it would replace.
    pub dropped_unchanged: usize,
    /// §2.5: the target moved between the lookup and the write.
    pub dropped_conflict: usize,
    /// Rule 9: a `replaces` naming no listed note; the item was kept.
    pub replaces_unknown: usize,
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
    /// Chunks whose project is no longer registered — its data was deleted
    /// (ST-21): advanced past with no call, and nothing is re-created. A
    /// missing project's chunks are learned.
    pub skipped_root_gone: usize,
    /// Each unusable answer this flush received, in order — in memory only
    /// (the L8 fix slice's P2).
    pub unusable: Vec<UnusableAnswer>,
    pub stopped: Option<FlushStop>,
}

/// One answer a reachable model gave that could not be used (ruling LR20),
/// kept in its flush's report so an evaluation can show what failed. Never
/// persisted and never logged above DEBUG: an answer quotes the user's own
/// words. The cursor keeps [`Self::reason`] alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnusableAnswer {
    /// Why it was unusable: the parser's reason, or `cut at max_tokens (n)`.
    pub reason: String,
    /// The answer's length in characters; 0 for an `EmptyCompletion`.
    pub chars: usize,
    /// How the model said the answer ended — `stop`, `max_tokens`,
    /// `tool_use` or `error`; `None` when the router answered no response
    /// (an `EmptyCompletion`: the reasoning ate the budget).
    pub finish_reason: Option<&'static str>,
    /// The answer's first [`super::UNUSABLE_EXCERPT_CHARS`] characters — the
    /// whole answer when it is at most twice that long.
    pub head: String,
    /// Its last [`super::UNUSABLE_EXCERPT_CHARS`] characters; empty when
    /// `head` holds the whole answer.
    pub tail: String,
}

impl UnusableAnswer {
    /// An answer that came back and did not parse, cut on character
    /// boundaries.
    fn answered(reason: &str, response: &openalpaca_llm::ChatResponse) -> Self {
        use openalpaca_llm::FinishReason;
        let text = response.content.as_str();
        let chars = text.chars().count();
        let (head, tail) = if chars <= 2 * UNUSABLE_EXCERPT_CHARS {
            (text.to_string(), String::new())
        } else {
            (
                text.chars().take(UNUSABLE_EXCERPT_CHARS).collect(),
                text.chars().skip(chars - UNUSABLE_EXCERPT_CHARS).collect(),
            )
        };
        Self {
            reason: reason.to_string(),
            chars,
            finish_reason: Some(match response.finish_reason {
                FinishReason::Stop => "stop",
                FinishReason::MaxTokens => "max_tokens",
                FinishReason::ToolUse => "tool_use",
                FinishReason::Error => "error",
            }),
            head,
            tail,
        }
    }

    /// An `EmptyCompletion`: no response, nothing to quote.
    fn empty(reason: &str) -> Self {
        Self {
            reason: reason.to_string(),
            chars: 0,
            finish_reason: None,
            head: String::new(),
            tail: String::new(),
        }
    }

    /// The answer's excerpt at DEBUG — the one place it is logged.
    fn log(&self, session_id: &str) {
        tracing::debug!(
            session_id,
            chars = self.chars,
            finish_reason = self.finish_reason,
            head = %self.head,
            tail = %self.tail,
            "memory learning: the unusable answer ({})",
            self.reason
        );
    }
}

impl FlushReport {
    fn add(&mut self, counts: &StoreCounts) {
        self.stored += counts.stored;
        self.active += counts.active;
        self.candidates += counts.candidates;
        self.replaced += counts.replaced;
        self.suggestions += counts.suggestions;
        self.deduplicated += counts.deduplicated;
        self.dropped_over_cap += counts.dropped_over_cap;
        self.dropped_suppressed += counts.dropped_suppressed;
        self.dropped_duplicate_key += counts.dropped_duplicate_key;
        self.dropped_already_learned += counts.dropped_already_learned;
        self.dropped_unchanged += counts.dropped_unchanged;
        self.dropped_conflict += counts.dropped_conflict;
        self.failed += counts.failed;
    }
}

/// What one close of a conversation did (design 11 §2): its working state's
/// refresh, then its conversation summaries, each written by a call of its
/// own (design 09 §13.3, owner decision CQ-11).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EpisodeReport {
    /// Summariser calls made: the working state's refresh (at most one) and
    /// the summaries' own calls.
    pub calls: usize,
    /// `episode` notes written.
    pub episodes: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Windows with less of the user's own text than
    /// `extract_min_content_len`: passed with no call.
    pub skipped_short: usize,
    /// Windows of a lane whose owner is not a `global_user` row.
    pub skipped_no_owner: usize,
    /// Windows whose project is no longer registered — its data was deleted
    /// (ST-21).
    pub skipped_root_gone: usize,
    /// A refresh given up after `MAX_ATTEMPTS` unusable answers — the state
    /// stays as it was, and the summary is written all the same — and
    /// windows passed after `MAX_ATTEMPTS` unusable answers to a summary's
    /// call.
    pub skipped_unusable: usize,
    /// Windows a summary already covers — a re-run after a crash between the
    /// write and the cursor's advance: passed with no call.
    pub skipped_summarized: usize,
    /// Windows left with none of the user's turns once the turns a forget
    /// suppressed are taken out: passed with no call.
    pub skipped_forgotten: usize,
    /// Summaries the service refused (a cited row deleted meanwhile).
    pub refused: usize,
    /// Each unusable answer this flush received, in order — in memory only
    /// (the L8 fix slice's P2).
    pub unusable: Vec<UnusableAnswer>,
    /// Why the close stopped early; `None` when the state is current (or its
    /// refresh was given up) and every window with enough of the user's
    /// turns was summarised.
    pub stopped: Option<FlushStop>,
}

impl EpisodeReport {
    /// Windows the episode cursor moved past, with a summary or without.
    pub fn windows(&self) -> usize {
        self.episodes
            + self.refused
            + self.skipped_short
            + self.skipped_no_owner
            + self.skipped_root_gone
            + self.skipped_unusable
            + self.skipped_summarized
            + self.skipped_forgotten
    }
}

/// What the daemon's one flush slot runs next.
enum Job {
    /// Learn a session's window (design 09 §1).
    Learn(String),
    /// Close a conversation: refresh its working state and write its
    /// summary (design 11 §2, owner decision CQ-11).
    Close(String),
}

/// Test seams inside the store-and-advance closure. Production uses
/// [`StoreHooks::default`]: a no-op and the advance.
#[derive(Clone)]
pub(crate) struct StoreHooks {
    /// Runs on the blocking thread before the store loop's `n`-th item (from
    /// 0) is written: a re-point or a delete landing between two items.
    pub before_store: Arc<dyn Fn(usize) + Send + Sync>,
    /// Runs on the blocking thread after the store loop, before the advance.
    pub after_store_loop: Arc<dyn Fn() + Send + Sync>,
    /// Skips the advance: a crash between §1.4's steps 3 and 4.
    pub skip_advance: bool,
}

impl Default for StoreHooks {
    fn default() -> Self {
        Self {
            before_store: Arc::new(|_| {}),
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

/// The model a flush calls, and what it may spend.
struct CallSetup {
    model: Option<String>,
    /// It runs on this machine: a turn that starts cancels its call.
    local: bool,
    /// A keyed remote model's rate buckets, read without consuming.
    headroom: Option<openalpaca_llm::RateHeadroom>,
    budget: Budget,
}

/// How one model call ended.
enum CallOutcome {
    Answered(openalpaca_llm::ChatResponse, i64),
    /// A reachable model answered nothing — the reasoning ate the budget:
    /// a call, and an unusable answer (M2, ruling LR20).
    Empty(LlmError),
    /// No answer: the rate headroom, a yield, a cancellation or no model.
    Stopped(FlushStop),
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
    /// The user's turns that finished since the session was last closed
    /// (design 11 §2). A marker: the working state's `through`, the episode
    /// cursor and the rows past them decide what the close does.
    episode_turns: u32,
    /// A close trigger fired: a quiet period, a switch, the drain.
    episode_due: bool,
    /// A held close waits until then.
    episode_retry_at: Option<Instant>,
    episode_backoff: Option<Duration>,
    /// Unusable answers to the refresh past this working-state `through` —
    /// kept in memory: a restart retries a poisoned refresh at most three
    /// more times.
    refresh_failures: Option<(i64, u32)>,
    /// The newest row of the rows a refresh was given up on: a close held
    /// after it — by its summary's call — does not ask about the same rows
    /// again; a row past it does.
    refresh_given_up: Option<i64>,
    /// Unusable answers to the summary window past this episode cursor —
    /// kept in memory: a restart retries a poisoned window at most three
    /// more times.
    episode_failures: Option<(i64, u32)>,
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
    /// The extraction's calls and cost today (UTC), from `llm_usage_daily`.
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

/// A summary's window as one blocking read saw it: the session row, the
/// episode cursor, and the rows past it.
struct CloseRead {
    session: Option<Conversation>,
    /// The episode cursor.
    episode_through: i64,
    /// The rows past the episode cursor.
    rows: Vec<LearningRow>,
}

fn read_close(db: &Database, session_id: &str) -> anyhow::Result<CloseRead> {
    let repo = ConversationRepository::new(db);
    let Some(session) = repo.get_session(session_id)? else {
        return Ok(CloseRead {
            session: None,
            episode_through: 0,
            rows: Vec::new(),
        });
    };
    let episode_through = LearningCursorRepository::new(db).episode_through(session_id)?;
    let rows = repo.learning_window(session_id, episode_through, WINDOW_READ_ROWS)?;
    Ok(CloseRead {
        session: Some(session),
        episode_through,
        rows,
    })
}

/// The session row's working state; `None` while it is empty, and when it
/// is not one (as the history layer reads it).
fn stored_state(session: &Conversation) -> Option<WorkingState> {
    if session.working_state.is_empty() {
        return None;
    }
    serde_json::from_str(&session.working_state).ok()
}

/// The owner is the user part of the lane key (design §1.1): a linked
/// connector user's lane carries their `global_id`.
pub(super) fn lane_owner(lane_key: &str) -> &str {
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

/// How the reason of an answer cut at its output ceiling begins — what the
/// retry reads back from the cursor (owner decision LQ20 (e)).
const CUT_REASON: &str = "cut at max_tokens";

/// Why an answer that did not parse is unusable: the parser's reason, unless
/// the model stopped at the request's output ceiling — then the answer was
/// cut there, and the reason says so (the L8 fix slice's P1). Either way it
/// is one unusable answer, counted as ruling LR20 counts it.
fn unusable_reason(
    response: &openalpaca_llm::ChatResponse,
    max_tokens: u32,
    parse_error: String,
) -> String {
    if response.finish_reason == openalpaca_llm::FinishReason::MaxTokens {
        format!("{CUT_REASON} ({max_tokens})")
    } else {
        parse_error
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

/// [`extractor::check_project`] before the model call, on the blocking pool:
/// two small file reads, and a listing only when no folder records the
/// root. A task that could not run reads as "learn" — the write checks
/// again.
async fn check_project_now(root: Option<PathBuf>) -> ProjectCheck {
    tokio::task::spawn_blocking(move || extractor::check_project(root.as_deref(), None))
        .await
        .unwrap_or(ProjectCheck::Learn(None))
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
        {
            if left.pending_user_turns > 0 {
                left.due = true;
            }
            // Design 11 §2: a switch closes the conversation left.
            if left.episode_turns > 0 {
                left.episode_due = true;
            }
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
            session.episode_turns = session.episode_turns.saturating_add(1);
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
                session.episode_turns = 0;
                session.episode_due = false;
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
                // Design 11 §2: a close waits for a quiet period of 30
                // minutes — two would cut a conversation too often.
                if session.episode_turns > 0
                    && session
                        .last_turn_at
                        .is_some_and(|at| now.duration_since(at) >= EPISODE_QUIET)
                {
                    session.episode_due = true;
                }
            }
            state
                .sessions
                .iter()
                .filter(|(_, s)| (s.pending_user_turns > 0 || s.episode_turns > 0) && !s.running)
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
                                    if s.pending_user_turns > 0 {
                                        s.due = true;
                                    }
                                    if s.episode_turns > 0 {
                                        s.episode_due = true;
                                    }
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
        let read = blocking(move || {
            let repo = LearningCursorRepository::new(&db);
            Ok((
                repo.sessions_with_unlearned_turns()?,
                // Design 09 §13.3: the summaries the stop drain did not
                // write — four of the user's turns past the episode cursor;
                // their close refreshes the state first (design 11 §2).
                repo.sessions_with_unsummarized_turns(EPISODE_MIN_USER_TURNS)?,
            ))
        })
        .await;
        let (sessions, unsummarized) = match read {
            Ok(found) => found,
            Err(e) => {
                tracing::warn!("memory learning: the boot catch-up could not read: {e:#}");
                return;
            }
        };
        if !sessions.is_empty() || !unsummarized.is_empty() {
            tracing::info!(
                sessions = sessions.len(),
                summaries = unsummarized.len(),
                "memory learning: catching up"
            );
        }
        {
            let mut state = self.lock();
            for id in sessions {
                let session = state.sessions.entry(id).or_default();
                session.pending_user_turns = session.pending_user_turns.max(1);
                session.due = true;
            }
            for id in unsummarized {
                let session = state.sessions.entry(id).or_default();
                session.episode_turns = session.episode_turns.max(1);
                session.episode_due = true;
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
        let job = {
            let mut state = self.lock();
            if state.flushing > 0 {
                return;
            }
            // A session's facts are learned before it is closed, and any due
            // learning goes before any close.
            let learn = state
                .sessions
                .iter()
                .filter(|(_, s)| s.due && !s.running && s.retry_at.is_none_or(|at| at <= now))
                .min_by_key(|(_, s)| s.last_turn_at)
                .map(|(id, _)| Job::Learn(id.clone()));
            let job = learn.or_else(|| {
                state
                    .sessions
                    .iter()
                    .filter(|(_, s)| {
                        s.episode_due
                            && !s.due
                            && !s.running
                            && s.episode_retry_at.is_none_or(|at| at <= now)
                    })
                    .min_by_key(|(_, s)| s.last_turn_at)
                    .map(|(id, _)| Job::Close(id.clone()))
            });
            let Some(job) = job else {
                return;
            };
            state.flushing += 1;
            job
        };
        let queue = Arc::clone(self);
        self.jobs.spawn(async move {
            let slot = FlushSlot(Arc::clone(&queue));
            let progress = match job {
                Job::Learn(session_id) => {
                    let report = queue.run_flush(&session_id, FlushPolicy::AUTOMATIC).await;
                    report.stopped.is_none() || report.chunks > 0
                }
                Job::Close(session_id) => {
                    let report = queue
                        .run_episodes(&session_id, FlushPolicy::AUTOMATIC)
                        .await;
                    report.stopped.is_none() || report.windows() > 0
                }
            };
            drop(slot);
            // The next due session — the boot catch-up's sessions go one at a
            // time — starts now only after progress. A flush that stopped
            // with none (a cap, the rate headroom, a busy daemon) waits for
            // the next sweep, or it would restart itself in a loop.
            if progress {
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
        let (sessions, summaries): (Vec<String>, Vec<String>) = {
            let mut state = self.lock();
            let mut ids: Vec<(Option<Instant>, String)> = Vec::new();
            let mut episodes: Vec<(Option<Instant>, String)> = Vec::new();
            for (id, session) in state.sessions.iter_mut() {
                if session.pending_user_turns > 0 || session.due {
                    session.due = true;
                    ids.push((session.last_turn_at, id.clone()));
                }
                // Design 09 §13.3: "in the shutdown drain".
                if session.episode_turns > 0 || session.episode_due {
                    session.episode_due = true;
                    episodes.push((session.last_turn_at, id.clone()));
                }
            }
            ids.sort();
            episodes.sort();
            (
                ids.into_iter().map(|(_, id)| id).collect(),
                episodes.into_iter().map(|(_, id)| id).collect(),
            )
        };
        if sessions.is_empty() && summaries.is_empty() {
            return true;
        }
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let queue = Arc::clone(self);
        self.jobs.spawn(async move {
            // Every session's facts first, then the closes: what MD-10
            // guarantees before what MD-16 and design 11 add.
            for session_id in sessions {
                if queue.jobs.cancel_token().is_cancelled() {
                    return;
                }
                queue.run_flush(&session_id, FlushPolicy::DRAIN).await;
            }
            for session_id in summaries {
                if queue.jobs.cancel_token().is_cancelled() {
                    return;
                }
                queue.run_episodes(&session_id, FlushPolicy::DRAIN).await;
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

    /// Closes a session now (design 11 §2): refreshes its working state when
    /// a user turn lies past what it covers, then writes one `episode` per
    /// window of at least four of the user's turns past its episode cursor,
    /// each by a call of its own (design 09 §13.3, owner decision CQ-11),
    /// with no chunk cap and no quiet rule — the harness's deterministic
    /// stand-in for the quiet period, the switch and the drain. One flush per
    /// session: a call while one runs returns at once with
    /// [`FlushStop::AlreadyRunning`].
    pub async fn flush_episodes(&self, session_id: &str) -> EpisodeReport {
        self.run_episodes(session_id, FlushPolicy::EXPLICIT).await
    }

    /// An automatic close of one session, now: what the sweeper starts.
    #[cfg(test)]
    pub(crate) async fn flush_episodes_automatic(&self, session_id: &str) -> EpisodeReport {
        self.run_episodes(session_id, FlushPolicy::AUTOMATIC).await
    }

    /// Whether a close trigger made this session due (design 11 §2).
    pub fn is_episode_due(&self, session_id: &str) -> bool {
        self.lock()
            .sessions
            .get(session_id)
            .is_some_and(|s| s.episode_due)
    }

    /// The user's turns that finished since the session was last closed.
    pub fn episode_turns(&self, session_id: &str) -> u32 {
        self.lock()
            .sessions
            .get(session_id)
            .map_or(0, |s| s.episode_turns)
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
        // A flush that did nothing — a cap, the rate headroom, a busy daemon,
        // a yield — is tried again on every sweep, so it logs at DEBUG: a
        // capped session's one INFO a day is the cap's own (§1.8).
        macro_rules! finished {
            ($level:expr) => {
                tracing::event!(
                    $level,
                    session_id,
                    calls = report.calls,
                    chunks = report.chunks,
                    stored = report.stored,
                    active = report.active,
                    candidates = report.candidates,
                    replaced = report.replaced,
                    suggestions = report.suggestions,
                    deduplicated = report.deduplicated,
                    dropped_over_cap = report.dropped_over_cap,
                    dropped_below_threshold = report.dropped_below_threshold,
                    dropped_invalid = report.dropped_invalid,
                    dropped_suppressed = report.dropped_suppressed,
                    dropped_duplicate_key = report.dropped_duplicate_key,
                    dropped_already_learned = report.dropped_already_learned,
                    dropped_unchanged = report.dropped_unchanged,
                    dropped_conflict = report.dropped_conflict,
                    replaces_unknown = report.replaces_unknown,
                    failed = report.failed,
                    skipped_short = report.skipped_short,
                    skipped_no_owner = report.skipped_no_owner,
                    skipped_unusable = report.skipped_unusable,
                    skipped_root_gone = report.skipped_root_gone,
                    input_tokens = report.input_tokens,
                    output_tokens = report.output_tokens,
                    stopped = ?report.stopped,
                    "memory learning finished"
                );
            };
        }
        if report.calls > 0 || report.chunks > 0 {
            finished!(tracing::Level::INFO);
        } else {
            finished!(tracing::Level::DEBUG);
        }
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
            let setup = self.call_setup(&dcfg, model).await;
            let budget = setup.budget;
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

            // A project whose data was deleted (ST-21: no longer a registered
            // project) learns nothing: its turns are dropped, as a deleted
            // conversation's are (LQ4), and the cursor moves past them. A
            // missing project — its folder gone, its record kept — is still
            // learned into its notes, and one being re-pointed waits, with no
            // call, until it is found at its new root.
            let root = chunk.root.as_ref().map(PathBuf::from);
            let project_id = match check_project_now(root.clone()).await {
                ProjectCheck::Learn(id) => id,
                ProjectCheck::Gone => {
                    tracing::info!(
                        session_id,
                        first_row = expected + 1,
                        last_row = chunk.through,
                        "memory learning skipped rows whose project is no longer registered (its data was deleted)"
                    );
                    self.advance(session_id, chunk.through, expected).await;
                    report.chunks += 1;
                    report.skipped_root_gone += 1;
                    continue;
                }
                ProjectCheck::Hold => {
                    tracing::info!(
                        session_id,
                        first_row = expected + 1,
                        last_row = chunk.through,
                        "memory learning holds the window: {}",
                        extractor::hold_reason(root.as_deref())
                    );
                    report.stopped = Some(FlushStop::StoreError);
                    return report;
                }
            };

            // The owner must be a `global_user` row (design §3): anything
            // else learns nothing, and its cursor advances.
            let ctx = match self
                .owner_context(session_id, &session, root.clone(), chunk.last_request_id())
                .await
            {
                Ok(Some(ctx)) => ctx,
                Ok(None) => {
                    self.advance(session_id, chunk.through, expected).await;
                    report.chunks += 1;
                    report.skipped_no_owner += 1;
                    continue;
                }
                Err(stop) => {
                    report.stopped = Some(stop);
                    return report;
                }
            };
            // A quarantined partition holds the window before any model
            // call: no cost is spent on a chunk that cannot be written.
            if let Err(stop) = self.preflight(session_id, &ctx).await {
                report.stopped = Some(stop);
                return report;
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

            // The known-notes lookup (§2.3): what the owner already has that
            // the chunk's rows bring up, and the keys in use — read-only, on
            // the blocking pool, recorded nowhere.
            let service = self.deps.memory_service.clone();
            let lookup_ctx = ctx.clone();
            let queries = lookup::queries_for(&chunk);
            let chunk_ids: Vec<i64> = chunk.rows.iter().map(|r| r.id).collect();
            let view = match tokio::task::spawn_blocking(move || {
                service.learning_view(&lookup_ctx, &queries, &chunk_ids)
            })
            .await
            {
                Ok(Ok(view)) => view,
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
                    tracing::warn!(session_id, "memory learning: the lookup failed: {e}");
                    report.stopped = Some(FlushStop::StoreError);
                    return report;
                }
                Err(e) => {
                    tracing::warn!(session_id, "memory learning: the lookup task failed: {e}");
                    report.stopped = Some(FlushStop::StoreError);
                    return report;
                }
            };
            let lookup = lookup::render(view, budget.known_bytes);

            // Build the call, then check the keyed model's rate headroom: a
            // chunk starts only when half of each bucket would remain after
            // its own estimate (§1.7) — the router's estimate, bytes ÷ 4.
            let (transcript, numbered) = chunk.transcript();
            let today = chunk.today();
            // Owner decision LQ20 (e): at temperature 0 the same prompt
            // fails the same way, so the retry of an answer cut at its output
            // ceiling asks for half the items and the JSON first. Rule 14's
            // cap stays the budget's.
            let after_cut = read.cursor.attempts > 0
                && read
                    .cursor
                    .last_error
                    .as_deref()
                    .is_some_and(|e| e.starts_with(CUT_REASON));
            let request = extractor::build_request(
                &PromptInput {
                    today: &today,
                    known_notes: &lookup.notes_text,
                    known_keys: &lookup.keys_text,
                    conversation: &transcript,
                    max_items: if after_cut {
                        (budget.items / 2).max(1)
                    } else {
                        budget.items
                    },
                    after_cut,
                },
                budget.out_tokens,
                setup.model.clone(),
            );
            let threshold = dcfg.orchestrator.memory.memory_confidence_threshold;
            drop(dcfg);
            let (response, latency_ms) =
                match self.call_model(request, policy, &setup, &cancel).await {
                    CallOutcome::Answered(response, latency_ms) => (response, latency_ms),
                    // An answer the reasoning ate is a reachable model's bad
                    // output, not an outage (M2, ruling LR20).
                    CallOutcome::Empty(e) => {
                        report.calls += 1;
                        let why = e.to_string();
                        let unusable = UnusableAnswer::empty(&why);
                        unusable.log(session_id);
                        report.unusable.push(unusable);
                        if self
                            .count_failure(session_id, &chunk, expected, &why, &mut report)
                            .await
                        {
                            continue;
                        }
                        return report;
                    }
                    CallOutcome::Stopped(stop) => {
                        report.stopped = Some(stop);
                        return report;
                    }
                };
            report.calls += 1;
            report.input_tokens += u64::from(response.usage.input_tokens);
            report.output_tokens += u64::from(response.usage.output_tokens);

            // LQ13: an answer that ended on its own is unwrapped; a cut one
            // is held to the forms read before (the L8 fix slice's P1).
            let parsed = if response.finish_reason == openalpaca_llm::FinishReason::MaxTokens {
                extractor::parse_cut_items(&response.content)
            } else {
                extractor::parse_items(&response.content)
            };
            self.record_usage(AGENT_LABEL, &response, latency_ms, parsed.is_err())
                .await;
            let items = match parsed {
                Ok(items) => items,
                Err(why) => {
                    let why = unusable_reason(&response, budget.out_tokens, why);
                    let unusable = UnusableAnswer::answered(&why, &response);
                    unusable.log(session_id);
                    report.unusable.push(unusable);
                    if self
                        .count_failure(session_id, &chunk, expected, &why, &mut report)
                        .await
                    {
                        continue;
                    }
                    return report;
                }
            };
            let validated = extractor::validate(&items, &numbered, threshold, lookup.listed.len());
            report.dropped_invalid += validated.invalid;
            report.dropped_below_threshold += validated.below_threshold;
            report.replaces_unknown += validated.replaces_unknown;
            report.dropped_duplicate_key += validated.duplicate_key;

            // Store the chunk's notes and advance its cursor in one blocking
            // task (§1.4): dropping this future does not separate them.
            let job = StoreJob {
                db: self.deps.db.clone(),
                service: self.deps.memory_service.clone(),
                ctx,
                session_id: session_id.to_string(),
                root,
                project_id,
                items: validated.items,
                listed: lookup.listed,
                from_this_window: lookup.from_this_window,
                today,
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
                // Deleted while the model answered: skipped as above.
                StoreEnd::RootGone => {
                    tracing::info!(
                        session_id,
                        first_row = expected + 1,
                        last_row = chunk.through,
                        "memory learning skipped rows whose project's data was deleted during the call"
                    );
                    report.chunks += 1;
                    report.skipped_root_gone += 1;
                }
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

    /// The model a flush calls, and what it may spend (design 09 §1.6,
    /// §1.7): `model` as the answering model resolves it, whether it runs on
    /// this machine, a keyed provider's rate headroom and the byte budget
    /// from its window. Learning's chunks call the extraction model; a
    /// close's refresh calls the summariser's (design 11 §3.1).
    async fn call_setup(&self, dcfg: &DaemonConfig, model: Option<String>) -> CallSetup {
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
        CallSetup {
            model,
            local,
            headroom,
            budget,
        }
    }

    /// One model call that shares the model with chat (design 09 §1.7). On
    /// a keyed remote model it starts only when half of each rate bucket
    /// would remain after its own estimate — the router's, bytes ÷ 4. On a
    /// local model a turn that starts cancels it: nothing is written and
    /// nothing is counted. The call is registered under the lock that reads
    /// the in-flight set, so a turn that started since the quiet check — the
    /// window read, the pre-flight and the gates all await — yields here,
    /// before the call goes out, and a later one finds the token to cancel.
    /// An explicit flush and the stop drain share nothing and just call.
    async fn call_model(
        &self,
        request: openalpaca_llm::RouterRequest,
        policy: FlushPolicy,
        setup: &CallSetup,
        cancel: &CancellationToken,
    ) -> CallOutcome {
        if policy.shares_with_chat
            && let Some(h) = setup.headroom
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
                return CallOutcome::Stopped(FlushStop::RateHeadroom);
            }
        }
        let local_call = if policy.shares_with_chat && setup.local {
            let token = cancel.child_token();
            let mut state = self.lock();
            if !state.in_flight.is_empty() {
                return CallOutcome::Stopped(FlushStop::Yielded);
            }
            state.local_call = Some(token.clone());
            Some(token)
        } else {
            None
        };
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
                return CallOutcome::Stopped(FlushStop::Yielded);
            }
            result = self.deps.router.complete(request) => Some(result),
        };
        if local_call.is_some() {
            self.lock().local_call = None;
        }
        let Some(response) = response else {
            return CallOutcome::Stopped(FlushStop::Cancelled);
        };
        let latency_ms = started.elapsed().as_millis() as i64;
        match response {
            Ok(response) => {
                self.note_available();
                CallOutcome::Answered(response, latency_ms)
            }
            Err(LlmRouterError::Llm(e @ LlmError::EmptyCompletion { .. })) => {
                self.note_available();
                CallOutcome::Empty(e)
            }
            Err(e) => {
                // §7b P10: the window holds, nothing is counted, and the
                // outage is logged once, not once per retry.
                self.note_unavailable(&e);
                CallOutcome::Stopped(FlushStop::Unavailable)
            }
        }
    }

    /// A chunk's memory context: the lane's owner — the user part of its key
    /// (design 09 §1.1) — the chunk's own project root, the `Extraction`
    /// surface. `Ok(None)` when the owner is not a `global_user` row (design
    /// §3): such a lane learns nothing. The owner check reads the database,
    /// so it runs on the blocking pool (§1.9).
    async fn owner_context(
        &self,
        session_id: &str,
        session: &Conversation,
        root: Option<PathBuf>,
        request_id: Option<&str>,
    ) -> Result<Option<MemoryAccessContext>, FlushStop> {
        let service = self.deps.memory_service.clone();
        let owner = lane_owner(&session.lane_key).to_string();
        let attribution = Attribution {
            lane_key: Some(session.lane_key.clone()),
            task_id: None,
            request_id: request_id.and_then(|r| Uuid::parse_str(r).ok()),
            source: Some(session.source.clone()),
        };
        let context = blocking(move || {
            let hint = match &root {
                Some(r) => WorkspaceHint::Root(r.as_path()),
                None => WorkspaceHint::None,
            };
            Ok(service.context(
                &owner,
                hint,
                Surface::Extraction,
                Capabilities::EXTRACTION,
                attribution,
            ))
        })
        .await;
        match context {
            Ok(Ok(ctx)) => Ok(Some(ctx)),
            Ok(Err(MemoryError::NoOwner(_))) => Ok(None),
            Ok(Err(e)) => {
                tracing::warn!(session_id, "memory learning: context refused: {e}");
                Err(FlushStop::StoreError)
            }
            Err(e) => {
                tracing::warn!(session_id, "memory learning: context task failed: {e:#}");
                Err(FlushStop::StoreError)
            }
        }
    }

    /// Refuses a chunk whose partition is quarantined, before any model
    /// call (design §2): the window waits for `memory doctor --repair`.
    async fn preflight(
        &self,
        session_id: &str,
        ctx: &MemoryAccessContext,
    ) -> Result<(), FlushStop> {
        let service = self.deps.memory_service.clone();
        let preflight_ctx = ctx.clone();
        match tokio::task::spawn_blocking(move || service.learning_preflight(&preflight_ctx)).await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(MemoryError::Quarantined { partition_key, .. })) => {
                tracing::warn!(
                    session_id,
                    %partition_key,
                    "memory learning waits: the partition is quarantined; run `openalpaca memory doctor --repair`"
                );
                Err(FlushStop::Quarantined)
            }
            Ok(Err(e)) => {
                tracing::warn!(session_id, "memory learning: pre-flight failed: {e}");
                Err(FlushStop::StoreError)
            }
            Err(e) => {
                tracing::warn!(session_id, "memory learning: pre-flight task failed: {e}");
                Err(FlushStop::StoreError)
            }
        }
    }

    // ------------------------------------------------------------------
    // A conversation's close (design 11 §2): its working state, then its
    // summaries (design 09 §13.3)
    // ------------------------------------------------------------------

    async fn run_episodes(&self, session_id: &str, policy: FlushPolicy) -> EpisodeReport {
        let Some(running) = RunningFlush::begin(self, session_id) else {
            return EpisodeReport {
                stopped: Some(FlushStop::AlreadyRunning),
                ..Default::default()
            };
        };
        let report = self.close(session_id, policy).await;
        running.finish_episodes(&report);
        macro_rules! finished {
            ($level:expr) => {
                tracing::event!(
                    $level,
                    session_id,
                    calls = report.calls,
                    episodes = report.episodes,
                    refused = report.refused,
                    skipped_short = report.skipped_short,
                    skipped_no_owner = report.skipped_no_owner,
                    skipped_root_gone = report.skipped_root_gone,
                    skipped_unusable = report.skipped_unusable,
                    skipped_summarized = report.skipped_summarized,
                    skipped_forgotten = report.skipped_forgotten,
                    input_tokens = report.input_tokens,
                    output_tokens = report.output_tokens,
                    stopped = ?report.stopped,
                    "memory learning: conversation closed"
                );
            };
        }
        if report.calls > 0 || report.windows() > 0 {
            finished!(tracing::Level::INFO);
        } else {
            finished!(tracing::Level::DEBUG);
        }
        report
    }

    /// A session's close (design 11 §2): its working state brought up to
    /// date ([`Self::refresh`]), then its summaries written, each by a call
    /// of its own ([`Self::summarize`], owner decision CQ-11). A refresh that
    /// stops holds the close before any summary — among others, one whose
    /// rows name a project in the middle of a re-point (R20).
    async fn close(&self, session_id: &str, policy: FlushPolicy) -> EpisodeReport {
        let mut report = EpisodeReport::default();
        if let Err(stop) = self.refresh(session_id, policy, &mut report).await {
            report.stopped = Some(stop);
            return report;
        }
        self.summarize(session_id, policy, report).await
    }

    /// The close's refresh (design 11 §2, §3.1): **one** summariser call only
    /// when at least one of the user's turns lies past the rows the working
    /// state covers — none when the state is current. The call folds every
    /// row past them, up to a turn still in flight, into the state, and
    /// `through` becomes the newest row it read (§3.2). It is made on
    /// `summary_model` under the summariser's label and daily ceiling, which
    /// is read first, and shares the model with chat as learning's calls do
    /// (design 09 §1.7). The state is written under its optimistic version.
    /// An answer that is not a state leaves the row untouched and holds the
    /// close, as learning holds a window; the `MAX_ATTEMPTS`-th gives the
    /// refresh up, and a close held after that — by its summary's call —
    /// does not ask about the same rows again. `Err` stops the close.
    async fn refresh(
        &self,
        session_id: &str,
        policy: FlushPolicy,
        report: &mut EpisodeReport,
    ) -> Result<(), FlushStop> {
        let dcfg = self.deps.daemon_config.load_full();
        if !dcfg.orchestrator.learning.enabled {
            return Err(FlushStop::Disabled);
        }
        let cancel = self.jobs.cancel_token();
        if cancel.is_cancelled() {
            return Err(FlushStop::Cancelled);
        }
        if policy.shares_with_chat && !self.is_quiet() {
            return Err(FlushStop::Busy);
        }

        let mut in_flight = self.in_flight_ids();
        let db = self.deps.db.clone();
        let owned_id = session_id.to_string();
        let read = blocking(move || {
            let repo = ConversationRepository::new(&db);
            let Some(session) = repo.get_session(&owned_id)? else {
                return Ok(None);
            };
            let rows =
                repo.learning_window(&owned_id, session.working_state_through, WINDOW_READ_ROWS)?;
            Ok(Some((session, rows)))
        })
        .await;
        let (session, mut rows) = match read {
            Ok(Some(found)) => found,
            Ok(None) => {
                self.forget_session(session_id);
                return Err(FlushStop::SessionGone);
            }
            Err(e) => {
                tracing::warn!(
                    session_id,
                    "memory learning: a conversation's rows could not be read for its close: {e:#}"
                );
                return Err(FlushStop::StoreError);
            }
        };
        in_flight.extend(self.in_flight_ids());
        // Up to a turn still in flight: its answer has not landed.
        if let Some(end) = rows.iter().position(|r| {
            r.role == "user" && r.request_id.as_ref().is_some_and(|q| in_flight.contains(q))
        }) {
            rows.truncate(end);
        }
        // "1 only if ≥ 1 user turn lies past `through`": else it is current.
        if window::user_turns(&rows, &in_flight) == 0 {
            return Ok(());
        }
        let Some(through) = rows.last().map(|r| r.id) else {
            return Ok(());
        };
        // R20 (the final fix wave): a project of these rows whose re-point
        // has not finished — a rename that failed after the rows were
        // rewritten, until the next boot finishes it — is recorded by no
        // folder, so its partition, and the forgets it holds, cannot be
        // read. The close holds here, before any read of the forgets or any
        // call, as the summaries' own check holds a window.
        let mut roots: Vec<&str> = rows
            .iter()
            .filter_map(|r| r.workspace_root.as_deref())
            .collect();
        roots.sort_unstable();
        roots.dedup();
        for root in roots {
            let root = PathBuf::from(root);
            if check_project_now(Some(root.clone())).await == ProjectCheck::Hold {
                tracing::info!(
                    session_id,
                    "memory learning holds a conversation's close: {}",
                    extractor::hold_reason(Some(root.as_path()))
                );
                return Err(FlushStop::StoreError);
            }
        }
        // Rev 1.7 (F5; design 03 §6): a row a forget took back is never
        // read, nor the rest of its turn — `through` still passes them.
        let service = self.deps.memory_service.clone();
        let (lane, checked) = (session.lane_key.clone(), rows.clone());
        let forgotten = match tokio::task::spawn_blocking(move || {
            window::forgotten_rows(&service, &lane, &checked)
        })
        .await
        {
            Ok(Ok(forgotten)) => forgotten,
            Ok(Err(MemoryError::Quarantined { .. })) => return Err(FlushStop::Quarantined),
            Ok(Err(e)) => {
                tracing::warn!(
                    session_id,
                    "memory learning: the forgotten rows of a conversation's close could not be read: {e}"
                );
                return Err(FlushStop::StoreError);
            }
            Err(e) => {
                tracing::warn!(
                    session_id,
                    "memory learning: the forgotten rows' task failed: {e}"
                );
                return Err(FlushStop::StoreError);
            }
        };
        rows.retain(|r| !forgotten.contains(&r.id));
        if window::user_turns(&rows, &in_flight) == 0 {
            return Ok(());
        }
        // Given up on these very rows already, in a close still held.
        if self
            .lock()
            .sessions
            .get(session_id)
            .is_some_and(|s| s.refresh_given_up == Some(through))
        {
            return Ok(());
        }

        // D12: the summariser's daily ceiling — the state's and the
        // summaries' calls together (CQ-11) — before any call.
        let spent = working_state::spent_today(&self.deps.router).await;
        if spent > dcfg.orchestrator.costs.summary_max_daily_cost_usd {
            tracing::debug!(
                session_id,
                "working state not refreshed at the close: summary cost ${spent:.2} exceeds the cap"
            );
            return Err(FlushStop::Cap);
        }

        let lines: Vec<TranscriptLine<'_>> = rows
            .iter()
            .filter(|r| (r.role == "user" || r.role == "assistant") && !r.content.is_empty())
            .map(|r| TranscriptLine {
                role: &r.role,
                text: &r.content,
            })
            .collect();
        let request = working_state::update(
            stored_state(&session).as_ref(),
            &lines,
            dcfg.orchestrator.costs.summary_model.clone(),
            dcfg.orchestrator.memory.msg_trunc_chars,
        );
        let setup = self.call_setup(&dcfg, request.model.clone()).await;
        let (response, latency_ms) = match self.call_model(request, policy, &setup, &cancel).await {
            CallOutcome::Answered(response, latency_ms) => (response, latency_ms),
            CallOutcome::Empty(e) => {
                report.calls += 1;
                let why = e.to_string();
                let unusable = UnusableAnswer::empty(&why);
                unusable.log(session_id);
                report.unusable.push(unusable);
                return self.count_refresh_failure(
                    session_id,
                    (session.working_state_through, through),
                    &why,
                    report,
                );
            }
            CallOutcome::Stopped(stop) => return Err(stop),
        };
        report.calls += 1;
        report.input_tokens += u64::from(response.usage.input_tokens);
        report.output_tokens += u64::from(response.usage.output_tokens);
        let parsed = WorkingState::parse(&response);
        let (db, router) = (self.deps.db.clone(), self.deps.router.clone());
        let (answer, recorded) = (response.clone(), parsed.clone());
        if let Err(e) = blocking(move || {
            working_state::record_usage(&db, &router, &answer, latency_ms, &recorded);
            Ok(())
        })
        .await
        {
            tracing::warn!(session_id, "the working-state call's usage: {e:#}");
        }
        match parsed {
            Ok(state) => {
                let db = self.deps.db.clone();
                let id = session_id.to_string();
                let version = session.working_state_version;
                // A write that does not land — the turn end wrote meanwhile,
                // twice — leaves the row to that writer (design 11 §12).
                if let Err(e) = blocking(move || {
                    working_state::write(&db, &id, version, &state, through);
                    Ok(())
                })
                .await
                {
                    tracing::warn!(session_id, "the working state's write: {e:#}");
                }
                if let Some(s) = self.lock().sessions.get_mut(session_id) {
                    s.refresh_failures = None;
                    s.refresh_given_up = None;
                }
                Ok(())
            }
            Err(why) => {
                let unusable = UnusableAnswer::answered(&why.0, &response);
                unusable.log(session_id);
                report.unusable.push(unusable);
                self.count_refresh_failure(
                    session_id,
                    (session.working_state_through, through),
                    &why.0,
                    report,
                )
            }
        }
    }

    /// Counts one unusable answer against the close's refresh, in memory
    /// (ruling LR20's rule, without its durable count), keyed by the working
    /// state's `through` — the first of `(through, newest)`, the second the
    /// newest row the refresh read. Before the `MAX_ATTEMPTS`-th the close
    /// holds (`BadOutput`, with learning's backoff); that one gives the
    /// refresh up with one WARN — the state stays as it was, and the close
    /// goes on to the summaries — and marks the rows given up on, so a close
    /// held after it does not ask about them again.
    fn count_refresh_failure(
        &self,
        session_id: &str,
        (through, newest): (i64, i64),
        error: &str,
        report: &mut EpisodeReport,
    ) -> Result<(), FlushStop> {
        let attempts = {
            let mut state = self.lock();
            let session = state.sessions.entry(session_id.to_string()).or_default();
            let attempts = match session.refresh_failures {
                Some((at, n)) if at == through => n + 1,
                _ => 1,
            };
            session.refresh_failures = Some((through, attempts));
            attempts
        };
        if attempts < MAX_ATTEMPTS {
            tracing::info!(
                session_id,
                attempts,
                "memory learning: the working state's refresh was unusable, retrying later: {error}"
            );
            return Err(FlushStop::BadOutput);
        }
        tracing::warn!(
            session_id,
            attempts,
            "memory learning gave up refreshing a conversation's working state after {attempts} unusable answers; it stays as it was: {error}"
        );
        if let Some(s) = self.lock().sessions.get_mut(session_id) {
            s.refresh_failures = None;
            s.refresh_given_up = Some(newest);
        }
        report.skipped_unusable += 1;
        Ok(())
    }

    /// The close's summaries (design 09 §13.3, owner decision CQ-11): while
    /// at least four of the user's turns lie past the session's episode
    /// cursor, one call per window — the next chunk that fits the byte
    /// budget of the summariser's model and never mixes roots — on
    /// `summary_model` under the summariser's daily ceiling (read before
    /// each call, the refresh's spend counted), learning's quiet rule and its
    /// yielding to chat, each summary written and the cursor moved in one
    /// blocking task.
    async fn summarize(
        &self,
        session_id: &str,
        policy: FlushPolicy,
        mut report: EpisodeReport,
    ) -> EpisodeReport {
        let cancel = self.jobs.cancel_token();
        let mut windows = 0usize;
        loop {
            if !self.deps.daemon_config.load().orchestrator.learning.enabled {
                report.stopped = Some(FlushStop::Disabled);
                return report;
            }
            if policy.max_chunks.is_some_and(|max| windows >= max) {
                report.stopped = Some(FlushStop::ChunkLimit);
                return report;
            }
            if cancel.is_cancelled() {
                report.stopped = Some(FlushStop::Cancelled);
                return report;
            }
            if policy.shares_with_chat && !self.is_quiet() {
                report.stopped = Some(FlushStop::Busy);
                return report;
            }

            let mut in_flight = self.in_flight_ids();
            let db = self.deps.db.clone();
            let owned_id = session_id.to_string();
            let read = match blocking(move || read_close(&db, &owned_id)).await {
                Ok(read) => read,
                Err(e) => {
                    tracing::warn!(
                        session_id,
                        "memory learning: an episode note's window could not be read: {e:#}"
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
            // "Only when at least 4 user turns have accumulated since the
            // session's last episode."
            if window::user_turns(&read.rows, &in_flight) < EPISODE_MIN_USER_TURNS as usize {
                return report;
            }

            let dcfg = self.deps.daemon_config.load();
            let setup = self
                .call_setup(&dcfg, dcfg.orchestrator.costs.summary_model.clone())
                .await;
            // No context rows: a summary covers its own window.
            let mut chunk = match window::next_chunk(
                &read.rows,
                &[],
                &in_flight,
                setup.budget.transcript_bytes,
                WINDOW_READ_ROWS as usize,
            ) {
                NextChunk::Empty => return report,
                NextChunk::Blocked => {
                    report.stopped = Some(FlushStop::InFlight);
                    return report;
                }
                NextChunk::Chunk(chunk) => chunk,
            };
            let expected = read.episode_through;

            if chunk.users_text_bytes() < dcfg.orchestrator.costs.extract_min_content_len {
                self.advance_episode(session_id, chunk.through, expected)
                    .await;
                windows += 1;
                report.skipped_short += 1;
                continue;
            }
            let root = chunk.root.as_ref().map(PathBuf::from);
            let project_id = match check_project_now(root.clone()).await {
                ProjectCheck::Learn(id) => id,
                ProjectCheck::Gone => {
                    self.advance_episode(session_id, chunk.through, expected)
                        .await;
                    windows += 1;
                    report.skipped_root_gone += 1;
                    continue;
                }
                ProjectCheck::Hold => {
                    tracing::info!(
                        session_id,
                        "memory learning holds an episode note's window: {}",
                        extractor::hold_reason(root.as_deref())
                    );
                    report.stopped = Some(FlushStop::StoreError);
                    return report;
                }
            };
            let ctx = match self
                .owner_context(session_id, &session, root.clone(), chunk.last_request_id())
                .await
            {
                Ok(Some(ctx)) => ctx,
                Ok(None) => {
                    self.advance_episode(session_id, chunk.through, expected)
                        .await;
                    windows += 1;
                    report.skipped_no_owner += 1;
                    continue;
                }
                Err(stop) => {
                    report.stopped = Some(stop);
                    return report;
                }
            };
            if let Err(stop) = self.preflight(session_id, &ctx).await {
                report.stopped = Some(stop);
                return report;
            }

            // A summary already covers the start of this window — a re-run
            // after a crash between its write and the cursor's advance: the
            // cursor moves past what it covers, with no call and no second
            // summary, and the turns after it are summarised next. Then the
            // turns a forget suppressed are taken out, so no summary restates
            // what the user said to forget (design 03 §6's hard guarantee).
            let service = self.deps.memory_service.clone();
            let check_ctx = ctx.clone();
            let ids: Vec<i64> = chunk.rows.iter().map(|r| r.id).collect();
            let checked = tokio::task::spawn_blocking(move || {
                let covered = service.summary_coverage(&check_ctx, &ids)?;
                let keys: Vec<String> = ids.iter().map(|id| format!("message:{id}")).collect();
                let suppressed = service.suppressed_sources(&check_ctx, &keys)?;
                Ok::<_, MemoryError>((covered, suppressed))
            })
            .await;
            let (covered, suppressed) = match checked {
                Ok(Ok(found)) => found,
                Ok(Err(MemoryError::Quarantined { .. })) => {
                    report.stopped = Some(FlushStop::Quarantined);
                    return report;
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        session_id,
                        "memory learning: an episode note's lookup failed: {e}"
                    );
                    report.stopped = Some(FlushStop::StoreError);
                    return report;
                }
                Err(e) => {
                    tracing::warn!(
                        session_id,
                        "memory learning: an episode note's lookup task failed: {e}"
                    );
                    report.stopped = Some(FlushStop::StoreError);
                    return report;
                }
            };
            if let Some(through) = covered {
                self.advance_episode(session_id, through, expected).await;
                windows += 1;
                report.skipped_summarized += 1;
                continue;
            }
            if !suppressed.is_empty() {
                let forgotten_turns: HashSet<String> = chunk
                    .rows
                    .iter()
                    .filter(|r| suppressed.contains(&format!("message:{}", r.id)))
                    .filter_map(|r| r.request_id.clone())
                    .collect();
                chunk.rows.retain(|r| {
                    !suppressed.contains(&format!("message:{}", r.id))
                        && r.request_id
                            .as_ref()
                            .is_none_or(|q| !forgotten_turns.contains(q))
                });
                if episode::user_turns(&chunk.rows) == 0 {
                    self.advance_episode(session_id, chunk.through, expected)
                        .await;
                    windows += 1;
                    report.skipped_forgotten += 1;
                    continue;
                }
            }

            // D12, CQ-11: the summariser's daily ceiling — the refresh's
            // spend and the summaries' together — before the call.
            let spent = working_state::spent_today(&self.deps.router).await;
            if spent > dcfg.orchestrator.costs.summary_max_daily_cost_usd {
                tracing::debug!(
                    session_id,
                    "conversation summary not written: summary cost ${spent:.2} exceeds the cap"
                );
                report.stopped = Some(FlushStop::Cap);
                return report;
            }

            let (transcript, _) = chunk.transcript();
            let today = chunk.today();
            let out_tokens = EPISODE_OUT_TOKENS.min(setup.budget.out_tokens);
            let request =
                episode::build_request(&today, &transcript, out_tokens, setup.model.clone());
            drop(dcfg);
            let (response, latency_ms) =
                match self.call_model(request, policy, &setup, &cancel).await {
                    CallOutcome::Answered(response, latency_ms) => (response, latency_ms),
                    CallOutcome::Empty(e) => {
                        report.calls += 1;
                        let why = e.to_string();
                        let unusable = UnusableAnswer::empty(&why);
                        unusable.log(session_id);
                        report.unusable.push(unusable);
                        if self
                            .count_episode_failure(session_id, &chunk, expected, &why, &mut report)
                            .await
                        {
                            windows += 1;
                            continue;
                        }
                        return report;
                    }
                    CallOutcome::Stopped(stop) => {
                        report.stopped = Some(stop);
                        return report;
                    }
                };
            report.calls += 1;
            report.input_tokens += u64::from(response.usage.input_tokens);
            report.output_tokens += u64::from(response.usage.output_tokens);
            let parsed = if response.finish_reason == openalpaca_llm::FinishReason::MaxTokens {
                episode::parse_cut_summary(&response.content)
            } else {
                episode::parse_summary(&response.content)
            };
            self.record_usage(EPISODE_LABEL, &response, latency_ms, parsed.is_err())
                .await;
            let summary = match parsed {
                Ok(summary) => summary,
                Err(why) => {
                    let why = unusable_reason(&response, out_tokens, why);
                    let unusable = UnusableAnswer::answered(&why, &response);
                    unusable.log(session_id);
                    report.unusable.push(unusable);
                    if self
                        .count_episode_failure(session_id, &chunk, expected, &why, &mut report)
                        .await
                    {
                        windows += 1;
                        continue;
                    }
                    return report;
                }
            };

            let job = EpisodeJob {
                db: self.deps.db.clone(),
                service: self.deps.memory_service.clone(),
                ctx,
                session_id: session_id.to_string(),
                root,
                project_id,
                input: episode::store_input(&summary, &chunk.rows, &today),
                through: chunk.through,
                expected,
                hooks: self.hooks.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            };
            let (written, end) =
                match tokio::task::spawn_blocking(move || episode::store_and_advance(job)).await {
                    Ok(done) => done,
                    Err(e) => (
                        EpisodeWrite::Refused,
                        StoreEnd::Held(format!("the store task failed: {e}")),
                    ),
                };
            match written {
                EpisodeWrite::Written => report.episodes += 1,
                EpisodeWrite::Existing | EpisodeWrite::NotAttempted => {}
                EpisodeWrite::Refused => report.refused += 1,
            }
            if end == StoreEnd::RootGone {
                // Deleted while the model wrote: skipped as above.
                report.skipped_root_gone += 1;
            }
            match end {
                StoreEnd::Advanced | StoreEnd::CursorMoved | StoreEnd::RootGone => {
                    windows += 1;
                    if let Some(s) = self.lock().sessions.get_mut(session_id) {
                        s.episode_failures = None;
                    }
                }
                StoreEnd::SessionGone => {
                    self.forget_session(session_id);
                    report.stopped = Some(FlushStop::SessionGone);
                    return report;
                }
                StoreEnd::Held(why) => {
                    tracing::warn!(
                        session_id,
                        "memory learning holds an episode note's window: {why}"
                    );
                    report.stopped = Some(FlushStop::StoreError);
                    return report;
                }
            }
        }
    }

    /// Counts one unusable answer against a summary's window, in memory
    /// (ruling LR20's rule, without its durable count). After `MAX_ATTEMPTS`
    /// of them the episode cursor moves past the window with one WARN and
    /// `true` lets the close go on; otherwise the window holds (`false`).
    async fn count_episode_failure(
        &self,
        session_id: &str,
        chunk: &window::Chunk,
        expected: i64,
        error: &str,
        report: &mut EpisodeReport,
    ) -> bool {
        let attempts = {
            let mut state = self.lock();
            let session = state.sessions.entry(session_id.to_string()).or_default();
            let attempts = match session.episode_failures {
                Some((at, n)) if at == expected => n + 1,
                _ => 1,
            };
            session.episode_failures = Some((expected, attempts));
            attempts
        };
        if attempts < MAX_ATTEMPTS {
            tracing::info!(
                session_id,
                attempts,
                "memory learning: the model's episode note was unusable, retrying later: {error}"
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
            "memory learning skipped rows {first}..={} for an episode note after {attempts} unusable answers: {error}",
            chunk.through
        );
        self.advance_episode(session_id, chunk.through, expected)
            .await;
        if let Some(s) = self.lock().sessions.get_mut(session_id) {
            s.episode_failures = None;
        }
        report.skipped_unusable += 1;
        true
    }

    /// Moves the episode cursor past a window that needs no summary.
    async fn advance_episode(&self, session_id: &str, through: i64, expected: i64) {
        let db = self.deps.db.clone();
        let id = session_id.to_string();
        if let Err(e) = blocking(move || {
            LearningCursorRepository::new(&db).advance_episode_cas(&id, through, expected)
        })
        .await
        {
            tracing::warn!(
                session_id,
                "memory learning: the episode cursor did not move: {e:#}"
            );
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
    /// which is also what the durable daily gates read — on the blocking
    /// pool (§1.9), and before the next chunk's gate reads it.
    async fn record_usage(
        &self,
        label: &'static str,
        response: &openalpaca_llm::ChatResponse,
        latency_ms: i64,
        parse_failed: bool,
    ) {
        let model = response.model.clone();
        let provider = self
            .deps
            .router
            .model_registry()
            .resolve_provider(&model)
            .map(|p| p.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let (input_tokens, output_tokens) =
            (response.usage.input_tokens, response.usage.output_tokens);
        let cost =
            self.deps
                .router
                .cost_tracker
                .calculate_cost(&model, input_tokens, output_tokens);
        let (status, error) = if parse_failed {
            ("error", Some("JSON parse failure"))
        } else {
            ("success", None)
        };
        let db = self.deps.db.clone();
        if let Err(e) = blocking(move || {
            LlmUsageRepository::new(&db).record_and_log(
                label,
                None,
                &provider,
                &model,
                input_tokens as i32,
                output_tokens as i32,
                cost,
                latency_ms,
                status,
                error,
            )
        })
        .await
        {
            tracing::warn!("Failed to persist memory learning usage: {e:#}");
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
    episode_turns_at_start: u32,
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
        let episode_turns_at_start = session.episode_turns;
        Some(Self {
            queue,
            session_id: session_id.to_string(),
            pending_at_start,
            episode_turns_at_start,
        })
    }

    /// The session's close bookkeeping once a close ends. One that left the
    /// state current and summarised every window with enough of the user's
    /// turns has looked at every turn it saw pending, and the session is no
    /// longer due; turns that finished while it ran stay pending. A held
    /// close waits out a doubling backoff; anything else that stopped early
    /// stays due.
    fn finish_episodes(self, report: &EpisodeReport) {
        let timings = self.queue.timings();
        let mut state = self.queue.lock();
        let Some(session) = state.sessions.get_mut(&self.session_id) else {
            return;
        };
        match report.stopped {
            None => {
                session.episode_turns = session
                    .episode_turns
                    .saturating_sub(self.episode_turns_at_start);
                session.episode_due = false;
                session.episode_retry_at = None;
                session.episode_backoff = None;
            }
            Some(stop) if stop.backs_off() => {
                let delay = session.episode_backoff.unwrap_or(timings.backoff_base);
                session.episode_retry_at = Some(Instant::now() + delay);
                session.episode_backoff = Some((delay * 2).min(timings.backoff_cap));
                session.episode_due = true;
            }
            Some(FlushStop::SessionGone | FlushStop::Disabled | FlushStop::AlreadyRunning) => {}
            Some(_) => session.episode_due = true,
        }
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
