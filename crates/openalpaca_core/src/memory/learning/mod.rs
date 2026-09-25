//! Background learning from conversations (Learning v2, design
//! `tasks/memory-redesign/09-learning-design.md`, owner decision MD-10).
//!
//! **Every turn is queued, nothing is skipped, and a reply never waits.** The
//! gateway tells the [`LearningQueue`] when a turn starts and finishes — three
//! in-memory updates, no await, no database — and the queue extracts a
//! session's unlearned turns together, off the reply path, when the session
//! has been idle about two minutes, when ten new turns have accumulated, when
//! the user switches conversations, and before the daemon stops.
//!
//! The unit of learning is a **session** (§1.1): its rows past the durable
//! `learning_cursor` are the window.

pub mod queue;

pub use queue::{LearningQueue, QueueMode, TurnGuard};

/// The `source` the gateway stamps on a follow-up's re-entered turn: the
/// assistant's own `queue_followup` text, never the user's words (F7).
pub const INTERNAL_SOURCE: &str = "internal";

/// A `/slash` turn — a slash skill, `/steer`, a scheduled skill — is left out
/// of learning together with its answer (design §1.2), exactly as the
/// per-turn extractor's content filter left it out.
pub fn is_slash_turn(content: &str) -> bool {
    content.trim_start().starts_with('/')
}

/// Whether a turn the gateway is about to persist counts towards the
/// 10-turn trigger: a user turn the user typed — not a `/slash` turn and not
/// the assistant's own follow-up text (design §1.2's *user-authored* row).
pub fn is_user_authored_turn(source: &str, content: &str) -> bool {
    source != INTERNAL_SOURCE && !is_slash_turn(content)
}
