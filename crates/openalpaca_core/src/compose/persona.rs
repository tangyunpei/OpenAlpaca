//! Layer 1 — Persona (Phase 1 stub).
//!
//! Real implementation lands in Phase 2. This stub returns empty blocks with
//! a fingerprint derived from `persona_version` + `mode` so the engine's cache
//! plumbing is exercisable end-to-end.

use super::types::{PersonaInput, PersonaMode, PersonaOutput};

fn mode_tag(mode: PersonaMode) -> u8 {
    match mode {
        PersonaMode::Default => 0,
        PersonaMode::Minimal => 1,
        PersonaMode::Skip => 2,
    }
}

pub fn compute(input: &PersonaInput) -> PersonaOutput {
    let mut h = blake3::Hasher::new();
    h.update(&input.persona_version.to_le_bytes());
    h.update(&[mode_tag(input.mode)]);
    PersonaOutput {
        blocks: Vec::new(),
        fingerprint: h.finalize().into(),
    }
}
