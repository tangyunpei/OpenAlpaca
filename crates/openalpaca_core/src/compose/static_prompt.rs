//! Layer 2 — Static Prompt (Phase 1 stub).
//!
//! Real implementation lands in Phase 2 (wraps `PromptBuilder`, implements
//! `PlannerHierarchical` and `SocialMinimal` modes). This stub returns an
//! empty system message with a fingerprint derived from the persona output
//! fingerprint + mode tag + model window so the engine's cache plumbing
//! differentiates cache entries across modes and persona versions.

use std::sync::Arc;

use super::types::{StaticPromptInput, StaticPromptMode, StaticPromptOutput};

fn mode_tag(mode: StaticPromptMode) -> u8 {
    match mode {
        StaticPromptMode::Default => 0,
        StaticPromptMode::PlannerHierarchical => 1,
        StaticPromptMode::SocialMinimal => 2,
    }
}

pub fn compute(input: &StaticPromptInput) -> StaticPromptOutput {
    let mut h = blake3::Hasher::new();
    h.update(&input.persona_output.fingerprint);
    h.update(&[mode_tag(input.mode)]);
    h.update(&input.model_window.to_le_bytes());
    StaticPromptOutput {
        system_message: Arc::<str>::from(""),
        section_registry: Vec::new(),
        fingerprint: h.finalize().into(),
    }
}
