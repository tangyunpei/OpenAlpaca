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
    // identity_budget is part of the rendered identity block length, so
    // the Layer-1 cache must bust when it changes. Tag None vs Some to
    // avoid collisions with Some(0).
    match input.identity_budget {
        None => h.update(&[0u8]),
        Some(n) => {
            h.update(&[1u8]);
            h.update(&(n as u64).to_le_bytes())
        }
    };
    match input.user_budget {
        None => h.update(&[0u8]),
        Some(n) => {
            h.update(&[1u8]);
            h.update(&(n as u64).to_le_bytes())
        }
    };
    h.finalize().into()
}

/// Per-sub-field fingerprints for Layer 1 cache attribution. Encoded so
/// that two inputs producing the same overall `compute_fingerprint` also
/// produce identical `PersonaSubFingerprints` (up to `PartialEq`).
///
/// The budget fields use a 9-byte encoding: `[tag, u64_le]`. `None` → all
/// zeros; `Some(n)` → `[1, n_le_bytes]`.
pub(super) fn compute_sub_fingerprints(
    input: &PersonaInput,
) -> super::PersonaSubFingerprints {
    let mut ib = [0u8; 9];
    if let Some(n) = input.identity_budget {
        ib[0] = 1;
        ib[1..9].copy_from_slice(&(n as u64).to_le_bytes());
    }
    let mut ub = [0u8; 9];
    if let Some(n) = input.user_budget {
        ub[0] = 1;
        ub[1..9].copy_from_slice(&(n as u64).to_le_bytes());
    }
    super::PersonaSubFingerprints {
        persona_version: input.persona_version,
        mode_tag: mode_tag(input.mode),
        identity_budget: ib,
        user_budget: ub,
    }
}

fn build_default(input: &PersonaInput) -> Vec<SystemBlock> {
    let mut blocks = Vec::with_capacity(3);

    // System persona block: always included in Default. Emits the full
    // <system_instructions> wrap (Identity + Core Values + Safety Rules +
    // Base Instructions) byte-identically to `PromptBuilder::system_persona`
    // so Default-mode migrations (Skill, SimpleQuery) stay byte-identical.
    blocks.push(system_persona_block_full(&input.system_persona));

    // User document block (if present and has content).
    if let Some(ref user_doc) = *input.user_document
        && let Some(block) = user_document_block(user_doc, input.user_budget)
    {
        blocks.push(block);
    }

    // Identity document block (if present and has content).
    if let Some(ref identity_doc) = *input.identity_document
        && let Some(block) = identity_document_block(identity_doc, input.identity_budget)
    {
        blocks.push(block);
    }

    blocks
}

fn build_minimal(input: &PersonaInput) -> Vec<SystemBlock> {
    // Minimal mode: only the system persona block as bare `base_instructions`.
    // Used by Planner / Replanner / Social / Pipeline / DagNode / LeadAgent
    // where callers wrap the persona in their own templates (e.g. Social's
    // `<system_instructions>{persona_text}</system_instructions>` format).
    vec![system_persona_block_bare(&input.system_persona)]
}

/// Bare persona block: emits only `persona.base_instructions` verbatim. Used
/// by `PersonaMode::Minimal` — callers (SocialMinimal in Layer 2) wrap the
/// content in their own template.
fn system_persona_block_bare(persona: &SystemPersona) -> SystemBlock {
    SystemBlock {
        name: "system_persona",
        content: Arc::<str>::from(persona.base_instructions.as_str()),
        priority: SectionPriority::Critical,
    }
}

/// Full persona block: emits the complete `<system_instructions>` wrap in
/// byte-identical form to `PromptBuilder::system_persona` so Default-mode
/// migrations reproduce pre-migration output exactly.
fn system_persona_block_full(persona: &SystemPersona) -> SystemBlock {
    let mut block = String::new();
    block.push_str("<system_instructions>\n");
    block.push_str(&format!("Identity: {}\n", persona.name));
    block.push_str("Core Values:\n");
    for v in &persona.core_values {
        block.push_str(&format!("- {}\n", v));
    }
    block.push_str("Safety Rules:\n");
    for r in &persona.safety_rules {
        block.push_str(&format!("- {}\n", r));
    }
    block.push_str(&format!(
        "Base Instructions: {}\n",
        persona.base_instructions
    ));
    block.push_str("</system_instructions>\n");
    SystemBlock {
        name: "system_persona",
        content: Arc::<str>::from(block),
        priority: SectionPriority::Critical,
    }
}

fn user_document_block(doc: &UserDocument, budget: Option<usize>) -> Option<SystemBlock> {
    let rendered = crate::middleware::user::user_to_prompt_block(doc, budget);
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

fn identity_document_block(doc: &IdentityDocument, budget: Option<usize>) -> Option<SystemBlock> {
    let rendered = crate::middleware::identity::identity_to_prompt_block(doc, budget);
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
