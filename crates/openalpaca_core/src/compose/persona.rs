//! Layer 1 — Persona.
//!
//! Reads the three persona documents (SystemPersona, UserDocument,
//! IdentityDocument) and produces a fingerprinted snapshot of blocks.
//!
//! Modes:
//! - `Default` — emit system_persona + (optional) user_document + (optional) identity_document
//! - `Minimal` — emit only the system_persona block
//! - `Skip`    — emit nothing (reserved; no default caller today)
//!
//! Fingerprint is `blake3(persona_version.to_le_bytes() || mode_tag)`. Per
//! spec, `Arc`-clone-equality implies content-equality, so the version counter
//! is the only thing that changes when the underlying documents mutate.

use std::sync::Arc;

use super::types::{
    IdentityDocument, PersonaInput, PersonaMode, PersonaOutput, SectionPriority, SystemBlock,
    SystemPersona, UserDocument,
};

/// Stable byte tag for the persona mode, used both for fingerprinting and
/// for discriminating cache entries. Kept in sync with `mode_tag` — if a new
/// variant is added, extend both.
pub(super) fn mode_tag(mode: PersonaMode) -> u8 {
    match mode {
        PersonaMode::Default => 0,
        PersonaMode::Minimal => 1,
        PersonaMode::Skip => 2,
    }
}

pub fn compute(input: &PersonaInput) -> PersonaOutput {
    let blocks = match input.mode {
        PersonaMode::Default => build_default(input),
        PersonaMode::Minimal => build_minimal(input),
        PersonaMode::Skip => Vec::new(),
    };

    PersonaOutput {
        blocks,
        fingerprint: compute_fingerprint(input),
    }
}

/// Public fingerprint helper used by `ComposeEngine::lookup_or_build_persona`
/// to compute the cache key before the output is built.
pub(super) fn compute_fingerprint(input: &PersonaInput) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&input.persona_version.to_le_bytes());
    h.update(&[mode_tag(input.mode)]);
    h.finalize().into()
}

fn build_default(input: &PersonaInput) -> Vec<SystemBlock> {
    let mut blocks = Vec::with_capacity(3);

    // System persona block: always included in Default.
    blocks.push(system_persona_block(&input.system_persona));

    // User document block (if present and has content).
    if let Some(ref user_doc) = *input.user_document
        && let Some(block) = user_document_block(user_doc)
    {
        blocks.push(block);
    }

    // Identity document block (if present and has content).
    if let Some(ref identity_doc) = *input.identity_document
        && let Some(block) = identity_document_block(identity_doc)
    {
        blocks.push(block);
    }

    blocks
}

fn build_minimal(input: &PersonaInput) -> Vec<SystemBlock> {
    // Minimal mode: only the system persona block. Used by Planner / Replanner /
    // Pipeline / DagNode / LeadAgent where per-user identity is not relevant.
    vec![system_persona_block(&input.system_persona)]
}

fn system_persona_block(persona: &SystemPersona) -> SystemBlock {
    // Phase 2 Default-mode note: Layer 2 takes the raw Layer-1 blocks and
    // feeds them to PromptBuilder::raw_system_block — so we just ship the
    // base_instructions verbatim here. Layer 2 is responsible for any XML
    // envelope formatting it needs (and the planner/social modes replicate
    // the existing inline formats byte-identically there).
    SystemBlock {
        name: "system_persona",
        content: Arc::<str>::from(persona.base_instructions.as_str()),
        priority: SectionPriority::Critical,
    }
}

fn user_document_block(doc: &UserDocument) -> Option<SystemBlock> {
    // `user_to_prompt_block` is the real helper name (plan doc used an older
    // name); accepts an `Option<usize>` budget, we pass None to defer trimming
    // to Layer 2's PromptBuilder.
    let rendered = crate::middleware::user::user_to_prompt_block(doc, None);
    if rendered.is_empty() {
        None
    } else {
        Some(SystemBlock {
            name: "user_document",
            content: Arc::<str>::from(rendered),
            priority: SectionPriority::High,
        })
    }
}

fn identity_document_block(doc: &IdentityDocument) -> Option<SystemBlock> {
    let rendered = crate::middleware::identity::identity_to_prompt_block(doc, None);
    if rendered.is_empty() {
        None
    } else {
        Some(SystemBlock {
            name: "identity_document",
            content: Arc::<str>::from(rendered),
            priority: SectionPriority::High,
        })
    }
}
