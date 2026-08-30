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
pub fn push_steering(
    shared_context: &SharedContext,
    bus: &EventBus,
    task_id: &str,
    lane_key: &str,
    msg: SteeringMsg,
) -> Result<usize, SteeringPushError> {
    let inbox = shared_context
        .steering_inbox(task_id)
        .ok_or(SteeringPushError::Closed)?;
    let request_id = msg.request_id;
    let depth = inbox.push(msg)?;
    bus.publish(SystemEvent::WorkflowSteered {
        task_id: task_id.to_string(),
        lane_key: lane_key.to_string(),
        request_id,
        timestamp: Utc::now(),
    });
    Ok(depth)
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

    #[test]
    fn test_push_steering_emits_workflow_steered() {
        let ctx = SharedContext::new();
        let bus = EventBus::default();
        let mut rx = bus.subscribe();

        // No inbox registered for the task → Closed, no event.
        assert_eq!(
            push_steering(&ctx, &bus, "task-1", "lane-1", msg("early")),
            Err(SteeringPushError::Closed)
        );
        assert!(rx.try_recv().is_err());

        let inbox = Arc::new(SteeringInbox::default());
        ctx.register_steering_inbox("task-1", inbox.clone());
        let m = msg("go");
        let expected_request_id = m.request_id;
        assert_eq!(push_steering(&ctx, &bus, "task-1", "lane-1", m), Ok(1));

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
            push_steering(&ctx, &bus, "task-1", "lane-1", msg("late")),
            Err(SteeringPushError::Closed)
        );
        assert!(rx.try_recv().is_err());
    }
}
