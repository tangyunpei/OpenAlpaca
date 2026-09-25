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

use std::collections::HashMap;
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

#[derive(Debug, Default)]
struct SessionState {
    /// User-authored turns that finished since this session was last learned.
    pending_user_turns: u32,
    /// When the session's last turn finished (the idle trigger's clock).
    last_turn_at: Option<Instant>,
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
#[derive(Debug)]
pub struct LearningQueue {
    mode: QueueMode,
    state: Mutex<QueueState>,
}

impl LearningQueue {
    pub fn new(mode: QueueMode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            state: Mutex::new(QueueState {
                sessions: HashMap::new(),
                lanes: HashMap::new(),
                in_flight: HashMap::new(),
                quiet_since: Instant::now(),
            }),
        })
    }

    pub fn mode(&self) -> QueueMode {
        self.mode
    }

    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

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

    /// User-authored turns of a session that finished since it was last
    /// learned.
    pub fn pending_turns(&self, session_id: &str) -> u32 {
        self.lock()
            .sessions
            .get(session_id)
            .map_or(0, |s| s.pending_user_turns)
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
