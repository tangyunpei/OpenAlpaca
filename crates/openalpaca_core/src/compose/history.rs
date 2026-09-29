//! Layer 4 — Conversation History.
//!
//! Assembles the per-turn messages: working state (if any) + recent_messages +
//! current user turn. Supports two modes:
//!
//! - `Default` — working state (optionally wrapped) + recent history + current turn.
//! - `Skip`    — empty output (used by lead-agent / subagent paths).
//!
//! Fingerprint covers:
//! `blake3(lane_tip_fingerprint || hash_opt_arc_str(working_state) || summary_wrap_mode_tag
//!  || hash_opt_msg(current_user_turn) || mode_tag)`.

use openalpaca_llm::ChatMessage;

use super::fingerprint::{hash_opt_arc_str, hash_opt_msg};
use super::types::{HistoryInput, HistoryMode, HistoryOutput, SummaryWrapMode};

pub fn compute(input: &HistoryInput) -> HistoryOutput {
    let fingerprint = compute_fingerprint(input);

    let messages = match &input.mode {
        HistoryMode::Skip => Vec::new(),
        HistoryMode::Default => build_default(input),
    };

    HistoryOutput {
        messages,
        fingerprint,
    }
}

fn build_default(input: &HistoryInput) -> Vec<ChatMessage> {
    let mut messages: Vec<ChatMessage> = Vec::with_capacity(input.recent_messages.len() + 2);

    // 1. The working state (optional), in the session summary's old slot
    //    (design 11 §4) — wrapped per summary_wrap_mode. The untrusted path
    //    is the block `WorkingState::render` writes: quoted reference data
    //    under the trust label, never an instruction.
    if let Some(ref state) = input.working_state {
        let content = match input.summary_wrap_mode {
            SummaryWrapMode::UntrustedWrap => crate::orchestrator::wrap_untrusted_context(
                state.as_ref(),
                crate::orchestrator::working_state::BLOCK_TYPE,
                "user_derived",
            ),
            SummaryWrapMode::Plain => state.as_ref().to_string(),
        };
        messages.push(ChatMessage::user(&content));
    }

    // 2. Recent messages (already shaped by the caller; Layer 4 preserves order).
    messages.extend(input.recent_messages.iter().cloned());

    // 3. Current user turn (if present).
    if let Some(ref turn) = input.current_user_turn {
        messages.push(turn.clone());
    }

    messages
}

/// Public fingerprint helper used by `ComposeEngine::lookup_or_build_history`
/// to compute the cache key before the output is built.
pub(super) fn compute_fingerprint(input: &HistoryInput) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&input.lane_tip_fingerprint);
    h.update(&hash_opt_arc_str(&input.working_state));
    h.update(&[summary_wrap_mode_tag(input.summary_wrap_mode)]);
    h.update(&hash_opt_msg(&input.current_user_turn));
    h.update(&[mode_tag(&input.mode)]);
    h.finalize().into()
}

fn mode_tag(m: &HistoryMode) -> u8 {
    match m {
        HistoryMode::Default => 0,
        HistoryMode::Skip => 1,
    }
}

fn summary_wrap_mode_tag(m: SummaryWrapMode) -> u8 {
    match m {
        SummaryWrapMode::UntrustedWrap => 0,
        SummaryWrapMode::Plain => 1,
    }
}
