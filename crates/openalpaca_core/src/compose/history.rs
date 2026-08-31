//! Layer 4 — Conversation History.
//!
//! Assembles the per-turn messages: summary (if any) + recent_messages +
//! current user turn. Supports two modes:
//!
//! - `Default` — summary (optionally wrapped) + recent history + current turn.
//! - `Skip`    — empty output (used by lead-agent / subagent paths).
//!
//! Fingerprint covers:
//! `blake3(lane_tip_fingerprint || hash_opt_arc_str(summary) || summary_wrap_mode_tag
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

    // 1. Summary (optional) — wrapped per summary_wrap_mode. The untrusted
    //    path reuses the same `wrap_untrusted_context` envelope the live
    //    orchestrator uses so Phase 4's Social/Skill migrations can route
    //    through compose without shape drift.
    if let Some(ref summary) = input.summary {
        let content = match input.summary_wrap_mode {
            SummaryWrapMode::UntrustedWrap => crate::orchestrator::wrap_untrusted_context(
                summary.as_ref(),
                "session_summary",
                "user_derived",
            ),
            SummaryWrapMode::Plain => summary.as_ref().to_string(),
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
    h.update(&hash_opt_arc_str(&input.summary));
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
