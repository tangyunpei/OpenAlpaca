//! Layer 4 — Conversation History (Phase 1 stub).
//!
//! Real implementation lands in Phase 3 (absorbs `build_context`). This stub
//! returns an empty output with a fingerprint derived from the
//! `lane_tip_fingerprint` so engine cache plumbing differentiates cache
//! entries across conversation turns.

use super::types::{HistoryInput, HistoryMode, HistoryOutput};

fn mode_tag(mode: &HistoryMode) -> u8 {
    match mode {
        HistoryMode::Default => 0,
        HistoryMode::Skip => 1,
        HistoryMode::FirstStepOnly { .. } => 2,
    }
}

pub fn compute(input: &HistoryInput) -> HistoryOutput {
    let mut h = blake3::Hasher::new();
    h.update(&input.lane_tip_fingerprint);
    h.update(&[mode_tag(&input.mode)]);
    HistoryOutput {
        messages: Vec::new(),
        fingerprint: h.finalize().into(),
    }
}
