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

mod extractor;
pub mod queue;
#[cfg(test)]
mod tests;
mod window;

pub use queue::{FlushReport, FlushStop, LearningDeps, LearningQueue, QueueMode, TurnGuard};

// Engineering bounds, not owner-facing choices (design §1.10): only MD-10's
// two numbers, the off switch and the daily call cap are configuration.

/// The most transcript one extraction call carries, whatever the window.
pub const MAX_TRANSCRIPT_BYTES: usize = 24 * 1024;
/// The most known-note text one call carries (L2 fills it).
pub const MAX_KNOWN_BYTES: usize = 8 * 1024;
/// The most output tokens one call asks for.
pub const MAX_OUTPUT_TOKENS: usize = 2800;
/// The window a model with no known context window is budgeted against:
/// Ollama's discovery fallback.
pub const UNKNOWN_WINDOW_TOKENS: usize = 8192;
/// A user row over this is possibly quoted text (ruling LR21) and keeps its
/// first 6 KiB and last 2 KiB.
pub const LONG_USER_ROW_BYTES: usize = 8 * 1024;
/// An answer or an assistant-written task over this keeps 3 KiB + 1 KiB.
pub const REPLY_CUT_BYTES: usize = 4 * 1024;
/// A context row keeps this much.
pub const CONTEXT_ROW_BYTES: usize = 1024;
/// Rows one window read takes; a longer window is read again after its
/// first chunks advance the cursor.
pub(crate) const WINDOW_READ_ROWS: i64 = 400;
/// The most chunks one automatic flush runs; the rest stays due and runs at
/// the next quiet start.
pub const MAX_CHUNKS_PER_FLUSH: usize = 4;
/// Consecutive unusable answers to one chunk before the cursor moves past it
/// with one WARN (ruling LR20).
pub const MAX_ATTEMPTS: u32 = 3;
/// The longest the stop drain waits (ruling LR6); what it does not finish,
/// the boot catch-up learns.
pub const SHUTDOWN_DRAIN: std::time::Duration = std::time::Duration::from_secs(3);
/// How long after start the boot catch-up looks for unlearned sessions.
pub const BOOT_CATCHUP_DELAY: std::time::Duration = std::time::Duration::from_secs(30);
/// How long the daemon must have had no turn in flight before a flush
/// starts (ruling LR7).
pub const RESUME_QUIET: std::time::Duration = std::time::Duration::from_secs(20);
/// How often the sweeper looks for idle and switched-away sessions.
pub const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);
/// The first retry delay after a held window; it doubles to [`BACKOFF_CAP`].
pub const BACKOFF_BASE: std::time::Duration = std::time::Duration::from_secs(60);
/// The longest retry delay.
pub const BACKOFF_CAP: std::time::Duration = std::time::Duration::from_secs(3600);

/// What one extraction call may spend (design §1.6), from the extraction
/// model's context window:
///
/// ```text
/// window   = the model's window, or 8192 tokens when unknown
/// out_tok  = min(2800, window / 4)                      the request's max_tokens
/// items    = min(extract_max_items_per_call, (out_tok − 300) / 180)
/// in_bytes = (window − out_tok − 1000) × 2               2 bytes/token: safe for CJK
/// transcript_bytes = min(24 KiB, in_bytes × 7 / 10)
/// known_bytes      = min(8 KiB,  in_bytes × 3 / 10)
/// ```
///
/// On a provider that needs a key, `in_bytes` is also capped so the chunk's
/// estimated input (bytes ÷ 4, the router's own estimate) is at most 40 % of
/// the key's TPM capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub out_tokens: u32,
    pub items: usize,
    pub transcript_bytes: usize,
    pub known_bytes: usize,
}

impl Budget {
    pub fn for_window(
        window_tokens: Option<usize>,
        max_items_per_call: usize,
        tpm_capacity: Option<u32>,
    ) -> Self {
        let window = window_tokens
            .filter(|w| *w > 0)
            .unwrap_or(UNKNOWN_WINDOW_TOKENS);
        let out_tokens = MAX_OUTPUT_TOKENS.min(window / 4);
        let items = max_items_per_call
            .max(1)
            .min((out_tokens.saturating_sub(300) / 180).max(1));
        let mut in_bytes = window.saturating_sub(out_tokens + 1000) * 2;
        if let Some(tpm) = tpm_capacity {
            // 40 % of TPM, in tokens, at 4 bytes a token.
            in_bytes = in_bytes.min(tpm as usize * 4 * 2 / 5);
        }
        Self {
            out_tokens: out_tokens as u32,
            items,
            transcript_bytes: MAX_TRANSCRIPT_BYTES.min(in_bytes * 7 / 10),
            known_bytes: MAX_KNOWN_BYTES.min(in_bytes * 3 / 10),
        }
    }
}

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
