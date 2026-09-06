//! Steering inbox — mid-workflow user interjections (Routing V2, Phase 1).
//!
//! A [`SteeringInbox`] is registered per running lead-agent task on
//! `SharedContext` (next to the cancellation tokens). Producers (the
//! `steer_workflow` tool and the `/steer ` prefix) push [`SteeringMsg`]s;
//! the agentic loop drains them at its round boundary and injects them as
//! `<user_interjection>` user messages.

use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::events::SystemEvent;
use crate::security::policy::{Principal, Scope};
use chrono::{DateTime, Utc};
use openalpaca_storage::Database;
use openalpaca_storage::repository::FollowupRepository;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;
use uuid::Uuid;

/// Default maximum number of queued steering messages per inbox.
pub const DEFAULT_STEERING_INBOX_CAP: usize = 16;

/// Prefix of injected interjection messages. The compactor uses it to exempt
/// interjections from heuristic discard/truncation (spec §3).
pub const USER_INTERJECTION_PREFIX: &str = "<user_interjection";

/// A single user interjection targeted at a running workflow.
///
/// Carries the originating principal/scope/workspace so leftover messages
/// can re-enter the front door as a fresh turn (follow-up conversion).
#[derive(Debug, Clone)]
pub struct SteeringMsg {
    pub text: String,
    pub request_id: Uuid,
    pub principal: Principal,
    pub scope: Scope,
    pub workspace_path: Option<String>,
    pub received_at: DateTime<Utc>,
}

impl SteeringMsg {
    /// Render this message as the `<user_interjection>` block injected into
    /// the agentic loop's conversation history.
    pub fn to_interjection(&self) -> String {
        format!(
            "{USER_INTERJECTION_PREFIX} ts=\"{}\">{}</user_interjection>",
            self.received_at.to_rfc3339(),
            self.text
        )
    }
}

/// Push a steering message into a running workflow's inbox, emitting
/// [`SystemEvent::WorkflowSteered`] on success. Shared by every producer
/// (the `/steer ` prefix and the `steer_workflow` tool).
///
/// Returns the queue depth after the push. `Err(Closed)` when no inbox is
/// registered for `task_id` (the workflow already detached) or the inbox
/// has closed; `Err(Full)` at the configured cap.
///
/// `db` is the same `Option<&Database>` precedent as the byte-cap eviction
/// (T42): used only for the fallback below, and `None` is a legitimate
/// "no index to keep consistent" answer — not an error.
pub fn push_steering(
    shared_context: &SharedContext,
    bus: &EventBus,
    task_id: &str,
    lane_key: &str,
    msg: SteeringMsg,
    db: Option<&Database>,
) -> Result<usize, SteeringPushError> {
    let inbox = shared_context
        .steering_inbox(task_id)
        .ok_or(SteeringPushError::Closed)?;
    let request_id = msg.request_id;
    let text = msg.text.clone();
    let received_at = msg.received_at;
    // §5.6b's recovery re-queues an undelivered interjection as the *exact*
    // `lane_followups` row the graceful path writes
    // (`dispatcher/lead_agent.rs`), and that row's `principal_json` is NOT
    // NULL. The graceful path holds the whole `SteeringMsg`; the boot pass
    // holds only this record — so the record carries the two fields the row
    // needs, or the interjection is unrecoverable. `null` is written
    // explicitly when the turn had no project, so "absent" always means "an
    // older build wrote this line".
    let principal = serde_json::to_value(&msg.principal).unwrap_or(serde_json::Value::Null);
    let principal_json = serde_json::to_string(&msg.principal).ok();
    let workspace_path = msg.workspace_path.clone();
    let depth = inbox.push(msg)?;
    // §5.5: one line in the workflow's transcript, written on the *accepted*
    // push. That is what makes crash recovery of an interjection possible —
    // a `steering` record with no later `steering_drained` naming its request
    // id is an interjection the workflow never delivered.
    if let Some(log) = shared_context.task_session_log(task_id) {
        let recorded = log.emit(
            crate::session_log::Record::new(crate::session_log::RecordType::Steering)
                .task(Some(task_id))
                .with_data(serde_json::json!({
                    "request_id": request_id.to_string(),
                    "lane_key": lane_key,
                    "text": text,
                    "received_at": received_at.to_rfc3339(),
                    "queue_depth": depth,
                    "principal": principal,
                    "workspace_path": workspace_path,
                })),
        );
        // `emit` returns `false` on a full channel or a writer that is
        // already gone (T42) — the record never reaches disk, so the
        // crash-recovery scan (`session_log::recovery::undrained_steering`)
        // can never find it: this interjection is unrecoverable on a crash.
        // File it as the same `unprocessed_steering` row the graceful path
        // and the boot recovery both write, so a log-channel drop is no
        // worse than a crash — the interjection still surfaces on the
        // lane's next turn (`query_handler/unprocessed_steering.rs`)
        // instead of vanishing outright.
        if !recorded {
            tracing::warn!(
                task_id = %task_id,
                request_id = %request_id,
                "steering record dropped from the session log — this interjection will not \
                 survive a crash; filing it as an unprocessed_steering follow-up instead"
            );
            file_dropped_steering(
                db,
                lane_key,
                task_id,
                &text,
                principal_json.as_deref(),
                workspace_path.as_deref(),
            );
        }
    }
    bus.publish(SystemEvent::WorkflowSteered {
        task_id: task_id.to_string(),
        lane_key: lane_key.to_string(),
        request_id,
        timestamp: Utc::now(),
    });
    Ok(depth)
}

/// The fallback `push_steering` takes when the session log dropped its
/// `steering` record. Files exactly what
/// `FollowupRepository::recover_unprocessed_steering` would write for this
/// same interjection after a crash — same kind, same columns — so a
/// log-channel drop degrades to "recovered late" rather than "lost".
fn file_dropped_steering(
    db: Option<&Database>,
    lane_key: &str,
    task_id: &str,
    text: &str,
    principal_json: Option<&str>,
    workspace_path: Option<&str>,
) {
    let Some(db) = db else {
        tracing::warn!(
            task_id = %task_id,
            "no database available to file the dropped steering record — this interjection \
             is lost if the workflow crashes before draining it"
        );
        return;
    };
    let Some(principal_json) = principal_json else {
        tracing::warn!(
            task_id = %task_id,
            "dropped steering record has an unserializable principal — cannot file it as a \
             follow-up"
        );
        return;
    };
    match file_unprocessed_steering(db, lane_key, task_id, text, principal_json, workspace_path) {
        Ok(Some(_id)) => {}
        Ok(None) => {
            // The graceful-exit conversion (or a prior call here) already
            // filed this exact interjection — R56's guard, not a bug.
            tracing::debug!(
                task_id = %task_id,
                "dropped steering record already filed as a follow-up — skipping the duplicate"
            );
        }
        Err(e) => {
            tracing::warn!(
                task_id = %task_id,
                "failed to file the dropped steering record as a follow-up: {e}"
            );
        }
    }
}

/// File a single `unprocessed_steering` follow-up row, unless a row for the
/// same interjection has already been filed.
///
/// R56: the shared choke point every writer of this row goes through — the
/// dropped-record fallback above, the graceful-exit leftover conversion
/// (`orchestrator/dispatcher/lead_agent.rs`), and the boot-time crash
/// recovery (`FollowupRepository::recover_unprocessed_steering`) all reach
/// [`FollowupRepository::queue_unprocessed_steering_once`] — the single
/// guarded `INSERT … WHERE NOT EXISTS` — so the same interjection can never
/// be filed twice by two different call sites, even when both fire for the
/// same message at workflow detach (the bug this function closes).
///
/// The row is pinned to the lane's current active session, same as
/// [`FollowupRepository::queue`] — unlike the boot recovery, which pins to
/// the crashed run's own session because by the time a crash is noticed the
/// lane may be showing a different conversation.
///
/// Returns `Ok(Some(id))` when a new row was inserted, `Ok(None)` when a
/// matching row already existed and nothing changed.
pub(crate) fn file_unprocessed_steering(
    db: &Database,
    lane_key: &str,
    task_id: &str,
    text: &str,
    principal_json: &str,
    workspace_path: Option<&str>,
) -> anyhow::Result<Option<i64>> {
    let repo = FollowupRepository::new(db);
    let session_id = repo.active_session_id(lane_key)?;
    repo.queue_unprocessed_steering_once(
        lane_key,
        text,
        principal_json,
        workspace_path,
        task_id,
        session_id.as_deref(),
    )
}

/// Why a push into a [`SteeringInbox`] was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteeringPushError {
    /// The queue is at capacity — the caller should offer `queue_followup`.
    Full,
    /// The workflow has detached — the inbox no longer accepts messages.
    Closed,
}

/// Bounded, closable MPSC-style inbox for steering messages.
///
/// Concurrency contract:
/// - `push` and `close_and_drain` both take the queue lock and check/set
///   `closed` while holding it, so a push racing a close either lands in the
///   drained batch or gets `Err(Closed)` — never lost.
/// - `push` wakes waiters via `Notify::notify_waiters`. Waiters must use
///   [`SteeringInbox::notified`], which registers interest *before*
///   re-checking emptiness, so a check-then-wait consumer cannot lose a
///   wakeup: `loop { if !inbox.is_empty() { break } select! { _ = inbox.notified() => {} ... } }`.
pub struct SteeringInbox {
    queue: Mutex<VecDeque<SteeringMsg>>,
    closed: AtomicBool,
    cap: usize,
    notify: Notify,
}

impl SteeringInbox {
    pub fn new(cap: usize) -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            closed: AtomicBool::new(false),
            cap,
            notify: Notify::new(),
        }
    }

    /// Acquire the queue lock, recovering from poisoning if necessary.
    fn lock_queue(&self) -> std::sync::MutexGuard<'_, VecDeque<SteeringMsg>> {
        self.queue.lock().unwrap_or_else(|p| {
            tracing::warn!("SteeringInbox mutex poisoned — recovering");
            p.into_inner()
        })
    }

    /// Push a message. Returns the queue depth after the push.
    ///
    /// Fails with `Closed` once the workflow has detached, or `Full` at the
    /// configured cap. Wakes any waiter blocked in [`Self::notified`].
    pub fn push(&self, msg: SteeringMsg) -> Result<usize, SteeringPushError> {
        let depth = {
            let mut queue = self.lock_queue();
            // Checked under the lock: `close_and_drain` sets `closed` while
            // holding it, so a concurrent push cannot slip in after the drain.
            if self.closed.load(Ordering::SeqCst) {
                return Err(SteeringPushError::Closed);
            }
            if queue.len() >= self.cap {
                return Err(SteeringPushError::Full);
            }
            queue.push_back(msg);
            queue.len()
        };
        self.notify.notify_waiters();
        Ok(depth)
    }

    /// Take every queued message, preserving arrival order.
    pub fn drain_all(&self) -> Vec<SteeringMsg> {
        self.lock_queue().drain(..).collect()
    }

    /// Re-append messages to the *front* of the queue, preserving their
    /// order. Bypasses both the cap and the closed flag — used to return
    /// drained-but-unsent messages on budget exits so the cleanup path can
    /// convert them to follow-ups.
    pub fn push_front_all(&self, msgs: Vec<SteeringMsg>) {
        {
            let mut queue = self.lock_queue();
            for msg in msgs.into_iter().rev() {
                queue.push_front(msg);
            }
        }
        self.notify.notify_waiters();
    }

    /// Close the inbox and take every remaining message. After this returns,
    /// any concurrent or later `push` gets `Err(Closed)`.
    pub fn close_and_drain(&self) -> Vec<SteeringMsg> {
        let drained: Vec<SteeringMsg> = {
            let mut queue = self.lock_queue();
            // Set closed while holding the lock — see `push`.
            self.closed.store(true, Ordering::SeqCst);
            queue.drain(..).collect()
        };
        // Wake waiters so they can observe the closed state.
        self.notify.notify_waiters();
        drained
    }

    pub fn is_empty(&self) -> bool {
        self.lock_queue().is_empty()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Wait until a message may be available (or the inbox closes).
    ///
    /// Registers interest in the notification *before* re-checking state, so
    /// a push that lands between the caller's emptiness check and this await
    /// still wakes the caller (no lost wakeup). Spurious returns are
    /// possible — callers loop and re-check `is_empty()`.
    pub async fn notified(&self) {
        let fut = self.notify.notified();
        tokio::pin!(fut);
        // Register this waiter so a subsequent `notify_waiters` wakes it.
        fut.as_mut().enable();
        if !self.is_empty() || self.is_closed() {
            return;
        }
        fut.await;
    }
}

impl Default for SteeringInbox {
    fn default() -> Self {
        Self::new(DEFAULT_STEERING_INBOX_CAP)
    }
}

impl std::fmt::Debug for SteeringInbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SteeringInbox")
            .field("depth", &self.lock_queue().len())
            .field("cap", &self.cap)
            .field("closed", &self.is_closed())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    fn msg(text: &str) -> SteeringMsg {
        SteeringMsg {
            text: text.to_string(),
            request_id: Uuid::new_v4(),
            principal: Principal::System,
            scope: Scope::Global,
            workspace_path: None,
            received_at: Utc::now(),
        }
    }

    #[test]
    fn test_push_and_drain_preserves_order() {
        let inbox = SteeringInbox::default();
        assert!(inbox.is_empty());
        assert_eq!(inbox.push(msg("one")), Ok(1));
        assert_eq!(inbox.push(msg("two")), Ok(2));
        assert!(!inbox.is_empty());

        let drained = inbox.drain_all();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].text, "one");
        assert_eq!(drained[1].text, "two");
        assert!(inbox.is_empty());
    }

    #[test]
    fn test_push_full_at_cap() {
        let inbox = SteeringInbox::new(2);
        assert_eq!(inbox.push(msg("a")), Ok(1));
        assert_eq!(inbox.push(msg("b")), Ok(2));
        assert_eq!(inbox.push(msg("c")), Err(SteeringPushError::Full));
    }

    #[test]
    fn test_close_and_drain_then_push_is_closed() {
        let inbox = SteeringInbox::default();
        inbox.push(msg("pending")).unwrap();

        let drained = inbox.close_and_drain();
        assert_eq!(drained.len(), 1);
        assert!(inbox.is_closed());
        assert!(inbox.is_empty());
        assert_eq!(inbox.push(msg("late")), Err(SteeringPushError::Closed));
        assert!(inbox.drain_all().is_empty());
    }

    #[test]
    fn test_push_front_all_bypasses_cap_and_closed() {
        let inbox = SteeringInbox::new(1);
        inbox.push(msg("head")).unwrap();
        inbox.close_and_drain();

        // Re-append two messages past the cap into a closed inbox.
        inbox.push_front_all(vec![msg("first"), msg("second")]);
        assert!(inbox.is_closed());
        let drained = inbox.drain_all();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].text, "first");
        assert_eq!(drained[1].text, "second");
    }

    #[test]
    fn test_push_front_all_orders_before_existing() {
        let inbox = SteeringInbox::default();
        inbox.push(msg("newer")).unwrap();
        inbox.push_front_all(vec![msg("older-1"), msg("older-2")]);
        let drained = inbox.drain_all();
        let texts: Vec<&str> = drained.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, vec!["older-1", "older-2", "newer"]);
    }

    #[tokio::test]
    async fn test_notified_wakes_on_push() {
        let inbox = Arc::new(SteeringInbox::default());
        let waiter = {
            let inbox = Arc::clone(&inbox);
            tokio::spawn(async move {
                loop {
                    if !inbox.is_empty() {
                        break;
                    }
                    inbox.notified().await;
                }
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        inbox.push(msg("wake up")).unwrap();
        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter should wake after push")
            .unwrap();
    }

    #[tokio::test]
    async fn test_notified_no_lost_wakeup_when_push_races_wait() {
        // A message pushed before the waiter awaits must not be missed:
        // `notified()` re-checks emptiness after registering interest.
        let inbox = SteeringInbox::default();
        inbox.push(msg("already there")).unwrap();
        tokio::time::timeout(Duration::from_millis(500), inbox.notified())
            .await
            .expect("notified() must return immediately when non-empty");
    }

    #[test]
    fn test_interjection_format() {
        let m = msg("focus on the tests");
        let rendered = m.to_interjection();
        assert!(rendered.starts_with("<user_interjection ts=\""));
        assert!(rendered.starts_with(USER_INTERJECTION_PREFIX));
        assert!(rendered.ends_with(">focus on the tests</user_interjection>"));
    }

    /// §5.5: an accepted push is one line in the workflow's transcript. That
    /// is what makes an undrained interjection findable after a crash — a
    /// `steering` record whose request id no `steering_drained` ever names.
    #[tokio::test]
    async fn push_steering_narrates_into_the_runs_session_log() {
        use crate::session_log::{SessionLogLimits, SessionLogService, read_records};

        let ctx = SharedContext::new();
        let bus = EventBus::default();
        let dir = tempfile::tempdir().unwrap();
        let service = SessionLogService::new(
            dir.path().to_path_buf(),
            None,
            SessionLogLimits::default(),
            "test".to_string(),
        );
        let handle = service.handle_for("sess-1");
        ctx.register_steering_inbox("task-1", Arc::new(SteeringInbox::default()));
        ctx.register_task_session_log("task-1", handle.clone());

        let m = msg("focus on the tests");
        let request_id = m.request_id;
        assert_eq!(push_steering(&ctx, &bus, "task-1", "u:gui", m, None), Ok(1));
        assert!(handle.flush().await);

        let records = read_records(&dir.path().join("sess-1")).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, "steering");
        assert_eq!(records[0].task_id.as_deref(), Some("task-1"));
        assert_eq!(records[0].data["request_id"], request_id.to_string());
        assert_eq!(records[0].data["text"], "focus on the tests");
        assert_eq!(records[0].data["lane_key"], "u:gui");
        assert_eq!(records[0].data["queue_depth"], 1);
    }

    /// §5.6b's recovery writes "the exact rows the graceful path already
    /// writes", and `lane_followups.principal_json` is NOT NULL. The graceful
    /// path has the `SteeringMsg` in hand; the recovery pass has only the
    /// record — so the record carries what the row needs, or the interjection
    /// is unrecoverable.
    #[tokio::test]
    async fn a_steering_record_carries_what_a_recovered_followup_needs() {
        use crate::session_log::{SessionLogLimits, SessionLogService, read_records};

        let ctx = SharedContext::new();
        let bus = EventBus::default();
        let dir = tempfile::tempdir().unwrap();
        let service = SessionLogService::new(
            dir.path().to_path_buf(),
            None,
            SessionLogLimits::default(),
            "test".to_string(),
        );
        let handle = service.handle_for("sess-1");
        ctx.register_steering_inbox("task-1", Arc::new(SteeringInbox::default()));
        ctx.register_task_session_log("task-1", handle.clone());

        let mut m = msg("focus on the tests");
        m.principal = Principal::User {
            global_id: "u-42".to_string(),
        };
        m.workspace_path = Some("/repo".to_string());
        assert_eq!(push_steering(&ctx, &bus, "task-1", "u:gui", m, None), Ok(1));
        assert!(handle.flush().await);

        let records = read_records(&dir.path().join("sess-1")).unwrap();
        // The principal round-trips as the same JSON `FollowupRepository::queue`
        // is handed by `dispatcher/lead_agent.rs`.
        let principal: Principal =
            serde_json::from_value(records[0].data["principal"].clone()).unwrap();
        assert_eq!(
            principal,
            Principal::User {
                global_id: "u-42".to_string()
            }
        );
        assert_eq!(records[0].data["workspace_path"], "/repo");
    }

    /// A turn with no project writes an explicit null, not a missing key: the
    /// recovered row's `workspace_path` is nullable and "absent" must not be
    /// mistaken for "this build did not write it".
    #[tokio::test]
    async fn a_steering_record_with_no_project_says_so() {
        use crate::session_log::{SessionLogLimits, SessionLogService, read_records};

        let ctx = SharedContext::new();
        let bus = EventBus::default();
        let dir = tempfile::tempdir().unwrap();
        let service = SessionLogService::new(
            dir.path().to_path_buf(),
            None,
            SessionLogLimits::default(),
            "test".to_string(),
        );
        let handle = service.handle_for("sess-1");
        ctx.register_steering_inbox("task-1", Arc::new(SteeringInbox::default()));
        ctx.register_task_session_log("task-1", handle.clone());
        assert_eq!(
            push_steering(&ctx, &bus, "task-1", "u:gui", msg("no project"), None),
            Ok(1)
        );
        assert!(handle.flush().await);

        let records = read_records(&dir.path().join("sess-1")).unwrap();
        // `get`, not `[]`: indexing answers `Null` for an absent key too, and
        // the point of this test is that the key is written.
        assert_eq!(
            records[0].data.get("workspace_path"),
            Some(&serde_json::Value::Null)
        );
    }

    /// A rejected push writes nothing: the log narrates what happened, not
    /// what was attempted.
    #[tokio::test]
    async fn a_rejected_push_writes_no_record() {
        use crate::session_log::{SessionLogLimits, SessionLogService, read_records};

        let ctx = SharedContext::new();
        let bus = EventBus::default();
        let dir = tempfile::tempdir().unwrap();
        let service = SessionLogService::new(
            dir.path().to_path_buf(),
            None,
            SessionLogLimits::default(),
            "test".to_string(),
        );
        let handle = service.handle_for("sess-2");
        let inbox = Arc::new(SteeringInbox::new(1));
        inbox.push(msg("first")).unwrap();
        ctx.register_steering_inbox("task-2", inbox);
        ctx.register_task_session_log("task-2", handle.clone());

        assert_eq!(
            push_steering(&ctx, &bus, "task-2", "u:gui", msg("second"), None),
            Err(SteeringPushError::Full)
        );
        assert!(handle.flush().await);
        assert!(read_records(&dir.path().join("sess-2")).unwrap().is_empty());
    }

    /// §5.5's crash-recovery scan reads the `steering` record off disk — a
    /// record `emit()` drops (a full channel, or a writer that has already
    /// gone) never reaches it, and `push_steering` used to ignore that
    /// return value entirely. On a drop, fall back to writing the same
    /// `unprocessed_steering` row the graceful path and the boot recovery
    /// both write, so a log-channel drop is no worse than a crash: the
    /// interjection still surfaces on the lane's next turn
    /// (`query_handler/unprocessed_steering.rs`) instead of vanishing.
    #[tokio::test]
    async fn a_dropped_steering_record_is_filed_as_a_followup() {
        use crate::session_log::{SessionLogLimits, SessionLogService};
        use openalpaca_storage::Database;
        use openalpaca_storage::repository::{
            FOLLOWUP_KIND_UNPROCESSED_STEERING, FollowupRepository,
        };

        let ctx = SharedContext::new();
        let bus = EventBus::default();
        let log_dir = tempfile::tempdir().unwrap();
        // A one-slot channel: the first `emit` fills it, the second finds it
        // full. Nothing awaits between the two pushes, so on this
        // current-thread test runtime the writer never gets a chance to
        // drain it first — same determinism as
        // `session_log::tests::a_full_channel_drops_and_counts_instead_of_blocking`.
        let service = SessionLogService::new(
            log_dir.path().to_path_buf(),
            None,
            SessionLogLimits {
                channel_capacity: 1,
                ..SessionLogLimits::default()
            },
            "test".to_string(),
        );
        let handle = service.handle_for("sess-1");
        ctx.register_steering_inbox("task-1", Arc::new(SteeringInbox::default()));
        ctx.register_task_session_log("task-1", handle.clone());

        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let mut first = msg("first");
        first.workspace_path = Some("/repo".to_string());
        assert_eq!(
            push_steering(&ctx, &bus, "task-1", "u:gui", first, Some(&db)),
            Ok(1)
        );

        let mut second = msg("second");
        second.workspace_path = Some("/repo".to_string());
        assert_eq!(
            push_steering(&ctx, &bus, "task-1", "u:gui", second, Some(&db)),
            Ok(2)
        );
        assert!(
            handle.dropped() > 0,
            "the second push's record must have been dropped to exercise the fallback"
        );

        let repo = FollowupRepository::new(&db);
        let rows = repo.list_queued_by_lane("u:gui").unwrap();
        assert_eq!(
            rows.len(),
            1,
            "only the dropped push should be filed as a follow-up: {rows:?}"
        );
        assert_eq!(rows[0].kind, FOLLOWUP_KIND_UNPROCESSED_STEERING);
        assert_eq!(rows[0].content, "second");
        assert_eq!(rows[0].source_task_id.as_deref(), Some("task-1"));
        assert_eq!(rows[0].workspace_path.as_deref(), Some("/repo"));
    }

    /// The core of this round's fix (R56): the dropped-record fallback above
    /// and the graceful-exit leftover conversion
    /// (`orchestrator/dispatcher/lead_agent.rs:502-542`) both file an
    /// undelivered interjection through this exact function — so calling it
    /// twice for the same message, once for each call site, must not leave
    /// two rows behind (a steered instruction shown to the model twice via
    /// `<unprocessed_steering>`).
    #[tokio::test]
    async fn the_dropped_record_fallback_and_the_graceful_exit_conversion_agree_on_one_row() {
        use openalpaca_storage::Database;
        use openalpaca_storage::repository::FollowupRepository;

        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // Simulates `push_steering`'s fallback filing the interjection...
        let first = file_unprocessed_steering(
            &db,
            "u:gui",
            "task-1",
            "focus on the tests",
            "\"System\"",
            Some("/repo"),
        )
        .unwrap();
        assert!(first.is_some(), "the first writer must insert the row");

        // ...and the graceful-exit conversion later trying to file the very
        // same interjection at workflow detach.
        let second = file_unprocessed_steering(
            &db,
            "u:gui",
            "task-1",
            "focus on the tests",
            "\"System\"",
            Some("/repo"),
        )
        .unwrap();
        assert_eq!(
            second, None,
            "the second writer must see the row already filed"
        );

        let repo = FollowupRepository::new(&db);
        let rows = repo.list_queued_by_lane("u:gui").unwrap();
        assert_eq!(
            rows.len(),
            1,
            "one steered instruction must not surface twice: {rows:?}"
        );
        assert_eq!(rows[0].content, "focus on the tests");
    }

    /// With no database at all, a dropped record has nowhere to be filed.
    /// The push itself must still succeed — the inbox already has the
    /// message — and the caller is warned rather than the process panicking.
    #[tokio::test]
    async fn a_dropped_steering_record_with_no_database_does_not_panic() {
        use crate::session_log::{SessionLogLimits, SessionLogService};

        let ctx = SharedContext::new();
        let bus = EventBus::default();
        let dir = tempfile::tempdir().unwrap();
        let service = SessionLogService::new(
            dir.path().to_path_buf(),
            None,
            SessionLogLimits {
                channel_capacity: 1,
                ..SessionLogLimits::default()
            },
            "test".to_string(),
        );
        let handle = service.handle_for("sess-1");
        ctx.register_steering_inbox("task-1", Arc::new(SteeringInbox::default()));
        ctx.register_task_session_log("task-1", handle.clone());

        assert_eq!(
            push_steering(&ctx, &bus, "task-1", "u:gui", msg("first"), None),
            Ok(1)
        );
        assert_eq!(
            push_steering(&ctx, &bus, "task-1", "u:gui", msg("second"), None),
            Ok(2)
        );
        assert!(handle.dropped() > 0);
    }

    #[test]
    fn test_push_steering_emits_workflow_steered() {
        let ctx = SharedContext::new();
        let bus = EventBus::default();
        let mut rx = bus.subscribe();

        // No inbox registered for the task → Closed, no event.
        assert_eq!(
            push_steering(&ctx, &bus, "task-1", "lane-1", msg("early"), None),
            Err(SteeringPushError::Closed)
        );
        assert!(rx.try_recv().is_err());

        let inbox = Arc::new(SteeringInbox::default());
        ctx.register_steering_inbox("task-1", inbox.clone());
        let m = msg("go");
        let expected_request_id = m.request_id;
        assert_eq!(
            push_steering(&ctx, &bus, "task-1", "lane-1", m, None),
            Ok(1)
        );

        match rx.try_recv().expect("WorkflowSteered must be published") {
            SystemEvent::WorkflowSteered {
                task_id,
                lane_key,
                request_id,
                ..
            } => {
                assert_eq!(task_id, "task-1");
                assert_eq!(lane_key, "lane-1");
                assert_eq!(request_id, expected_request_id);
            }
            other => panic!("unexpected event: {other:?}"),
        }

        // Closed inbox → Closed, and no event for the failed push.
        inbox.close_and_drain();
        assert_eq!(
            push_steering(&ctx, &bus, "task-1", "lane-1", msg("late"), None),
            Err(SteeringPushError::Closed)
        );
        assert!(rx.try_recv().is_err());
    }
}
