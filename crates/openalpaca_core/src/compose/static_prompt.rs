//! Layer 2 — Static Prompt.
//!
//! Assembles the cachable, tenant-stable part of the system prompt:
//! persona blocks + agent persona + skills + bootstrap + tools + connector
//! guidance + raw blocks. Modes:
//!
//! - `Default` — wraps the existing `PromptBuilder` with all provided inputs
//!   in the SimpleQuery canonical order
//!   (persona -> agent_persona -> bootstrap -> skills_catalog -> skill_body
//!   -> tools -> connector_guidance -> send_context -> message_source ->
//!   raw_blocks).
//! - `SocialMinimal` — replicates `simple_query_handler.rs:512-526`'s inline
//!   `<system_instructions>` / `<agent_role>` format byte-for-byte.
//! - `SkillInvocationDefault` — Skill Invocation's pre-migration order
//!   (persona -> agent_persona -> bootstrap -> skills_catalog -> skill_body
//!   -> raw_blocks -> message_source -> connector_guidance -> tools ->
//!   send_context). Differs from `Default` in that `message_source` precedes
//!   `tools`/`connector_guidance`, and `raw_blocks` land between `skill_body`
//!   and `message_source` so `skill_context` sits at its pre-migration
//!   position. See `build_skill_invocation_default`.

use std::sync::Arc;

use super::fingerprint::{
    hash_connector_status, hash_opt_arc_str, hash_raw_blocks, hash_tool_set,
};
use super::types::{
    SectionPriority, StaticPromptInput, StaticPromptMode, StaticPromptOutput,
};

pub fn compute(input: &StaticPromptInput) -> StaticPromptOutput {
    match input.mode {
        StaticPromptMode::Default => build_default(input),
        StaticPromptMode::SocialMinimal => build_social_minimal(input),
        StaticPromptMode::SkillInvocationDefault => build_skill_invocation_default(input),
        StaticPromptMode::SubagentMinimal => build_subagent_minimal(input),
    }
}

/// Public fingerprint helper used by `ComposeEngine::lookup_or_build_static_prompt`
/// to compute the cache key before the output is built.
pub(super) fn compute_fingerprint(input: &StaticPromptInput) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&input.persona_output.fingerprint);
    h.update(&input.agent_config_fingerprint);
    h.update(&hash_tool_set(&input.tools));
    h.update(&hash_opt_arc_str(&input.skill_block));
    h.update(&hash_opt_arc_str(&input.skills_catalog));
    h.update(&hash_opt_arc_str(&input.bootstrap));
    h.update(&hash_connector_status(&input.connector_status));
    h.update(&hash_opt_arc_str(&input.send_tool_context));
    h.update(&hash_opt_arc_str(&input.message_source));
    h.update(&hash_raw_blocks(&input.raw_blocks));
    h.update(&[mode_tag(input.mode)]);
    h.update(&input.model_window.to_le_bytes());
    // AgentPersona: Debug-derive-based cheap canonical serializer. A future
    // phase may switch to bincode/serde_json for a stricter hash, but Debug
    // output is stable across a single deployment.
    if let Some(ref ap) = input.agent_persona {
        h.update(&[1u8]);
        let ap_bytes = format!("{:?}", ap).into_bytes();
        h.update(&(ap_bytes.len() as u64).to_le_bytes());
        h.update(&ap_bytes);
    } else {
        h.update(&[0u8]);
    }
    h.finalize().into()
}

/// Per-sub-field fingerprints for Layer 2 cache attribution. Each `*_fp` byte
/// array covers one logical sub-field of `StaticPromptInput`; on miss,
/// `attribute_static_prompt_miss` diffs these against the MRU cached entry's
/// sub-fingerprints to pick a specific `MissReason`.
///
/// The `agent_persona_fp` helper uses the same Debug-serializer trick as
/// `compute_fingerprint` does for `AgentPersona`.
pub(super) fn compute_sub_fingerprints(
    input: &StaticPromptInput,
) -> super::StaticPromptSubFingerprints {
    let agent_persona_fp: [u8; 32] = if let Some(ref ap) = input.agent_persona {
        let mut h = blake3::Hasher::new();
        h.update(&[1u8]);
        let ap_bytes = format!("{:?}", ap).into_bytes();
        h.update(&(ap_bytes.len() as u64).to_le_bytes());
        h.update(&ap_bytes);
        h.finalize().into()
    } else {
        let mut h = blake3::Hasher::new();
        h.update(&[0u8]);
        h.finalize().into()
    };

    super::StaticPromptSubFingerprints {
        persona_fp: input.persona_output.fingerprint,
        agent_config_fp: input.agent_config_fingerprint,
        agent_persona_fp,
        tools_fp: hash_tool_set(&input.tools),
        skills_catalog_fp: hash_opt_arc_str(&input.skills_catalog),
        skill_block_fp: hash_opt_arc_str(&input.skill_block),
        bootstrap_fp: hash_opt_arc_str(&input.bootstrap),
        connector_status_fp: hash_connector_status(&input.connector_status),
        raw_blocks_fp: hash_raw_blocks(&input.raw_blocks),
        send_tool_context_fp: hash_opt_arc_str(&input.send_tool_context),
        message_source_fp: hash_opt_arc_str(&input.message_source),
        mode_tag: mode_tag(input.mode),
        model_window: input.model_window,
    }
}

// Tag values keep their historical positions (1 and 3 belonged to the
// deleted planner-era modes). The gaps are harmless: tags only feed the
// in-memory cache fingerprint.
fn mode_tag(mode: StaticPromptMode) -> u8 {
    match mode {
        StaticPromptMode::Default => 0,
        StaticPromptMode::SocialMinimal => 2,
        StaticPromptMode::SkillInvocationDefault => 4,
        StaticPromptMode::SubagentMinimal => 5,
    }
}

fn build_default(input: &StaticPromptInput) -> StaticPromptOutput {
    use crate::prompt::PromptBuilder;

    // PromptBuilder takes a usize window; our input uses u32.
    let mut builder = PromptBuilder::new(input.model_window as usize);

    // Ship Layer 1's blocks as raw system blocks. Layer 1 Default mode emits
    // the full `<system_instructions>` wrap so this block is byte-identical to
    // what `PromptBuilder::system_persona` would produce.
    for block in &input.persona_output.blocks {
        builder = builder.raw_system_block(block.name, &block.content, block.priority);
    }

    // Agent persona (migration sites populate this).
    if let Some(ref ap) = input.agent_persona {
        builder = builder.agent_persona(ap);
    }

    if let Some(ref b) = input.bootstrap {
        builder = builder.bootstrap(b);
    }
    if let Some(ref sc) = input.skills_catalog {
        builder = builder.skills_catalog(sc);
    }
    if let Some(ref sb) = input.skill_block {
        builder = builder.raw_system_block("skill_body", sb, SectionPriority::High);
    }
    if !input.tools.is_empty() {
        builder = builder.tools(&input.tools);
    }

    // PromptBuilder's `connector_guidance` expects `&[(String, String)]` +
    // optional sendable list — adapt from our ConnectorSummary wrapper.
    if !input.connector_status.is_empty() {
        let statuses: Vec<(String, String)> = input
            .connector_status
            .iter()
            .map(|s| (s.id.clone(), s.status.clone()))
            .collect();
        let sendable: Vec<String> = input
            .connector_status
            .iter()
            .filter(|s| s.sendable)
            .map(|s| s.id.clone())
            .collect();
        let sendable_slice = if sendable.is_empty() {
            None
        } else {
            Some(sendable.as_slice())
        };
        builder = builder.connector_guidance(&statuses, sendable_slice);
    }

    if let Some(ref stc) = input.send_tool_context {
        builder = builder.raw_system_block("send_context", stc, SectionPriority::Normal);
    }
    if let Some(ref ms) = input.message_source {
        builder = builder.message_source(ms);
    }

    for block in &input.raw_blocks {
        builder = builder.raw_system_block(block.name, &block.content, block.priority);
    }

    let built = builder.build();

    StaticPromptOutput {
        system_message: Arc::<str>::from(built.system_message),
        section_registry: built.section_registry,
        fingerprint: compute_fingerprint(input),
    }
}

/// Pipeline / DAG / LeadAgent raw_blocks-only section emission.
///
/// Matches `orchestrator/dispatcher/pipeline_step.rs:341-440` (and structurally
/// identical DAG / LeadAgent pre-migration builders): NO persona wrap, NO
/// agent_persona, NO identity, NO bootstrap, NO skills_catalog, NO send_context,
/// NO message_source. The caller pre-renders tools via
/// `format_tool_guidance(...)` and pushes the rendered string as a raw_block
/// named "tools" at the correct position relative to other raw_blocks.
/// Connector guidance (already-formatted by the caller via
/// `format_connector_guidance(...)`) arrives as a raw_block named
/// "connector_guidance". Task-description text (previously routed through the
/// `ContextPackage` TaskDescription `InjectionMode::SystemPrompt` section,
/// which pre-migration `PromptBuilder::build()` appended inline to the system
/// message) is pushed as a raw_block named "task_description" at the end —
/// callers must exclude TaskDescription from the bundle they feed into
/// Layer 3, otherwise it will surface as a separate `ChatMessage::system`
/// per Layer 3's `SystemPrompt` routing.
///
/// Build order: iterate `input.raw_blocks` once; no other sections emitted.
fn build_subagent_minimal(input: &StaticPromptInput) -> StaticPromptOutput {
    use crate::prompt::PromptBuilder;

    let mut builder = PromptBuilder::new(input.model_window as usize);

    for block in &input.raw_blocks {
        builder = builder.raw_system_block(block.name, &block.content, block.priority);
    }

    let built = builder.build();

    StaticPromptOutput {
        system_message: Arc::<str>::from(built.system_message),
        section_registry: built.section_registry,
        fingerprint: compute_fingerprint(input),
    }
}

/// Skill Invocation-specific section order. Matches the pre-migration
/// `invoke_skill` PromptBuilder chain at `orchestrator/skill/invocation.rs:114-230`:
///   persona -> agent_persona -> identity -> bootstrap -> skill_body (raw, High) ->
///   raw_blocks (incl. skill_context raw, Normal when present) -> message_source
///   -> connector_guidance -> tools -> send_context (raw, when `send` tool active).
///
/// Differs from [`build_default`] in that `message_source` precedes `tools` /
/// `connector_guidance`, and `raw_blocks` land between `skill_body` and
/// `message_source` so `skill_context` sits at its pre-migration position.
fn build_skill_invocation_default(input: &StaticPromptInput) -> StaticPromptOutput {
    use crate::prompt::PromptBuilder;

    // PromptBuilder takes a usize window; our input uses u32.
    let mut builder = PromptBuilder::new(input.model_window as usize);

    // Ship Layer 1's blocks as raw system blocks. Layer 1 Default mode emits
    // the full `<system_instructions>` wrap so this block is byte-identical to
    // what `PromptBuilder::system_persona` would produce.
    for block in &input.persona_output.blocks {
        builder = builder.raw_system_block(block.name, &block.content, block.priority);
    }

    // Agent persona (migration sites populate this).
    if let Some(ref ap) = input.agent_persona {
        builder = builder.agent_persona(ap);
    }

    if let Some(ref b) = input.bootstrap {
        builder = builder.bootstrap(b);
    }
    if let Some(ref sc) = input.skills_catalog {
        builder = builder.skills_catalog(sc);
    }
    if let Some(ref sb) = input.skill_block {
        builder = builder.raw_system_block("skill_body", sb, SectionPriority::High);
    }

    // raw_blocks are emitted HERE — after skill_body, before message_source —
    // so the Skill migration can inject "skill_context" in its pre-migration
    // position (between skill_body and message_source; see
    // orchestrator/skill/invocation.rs pre-migration order).
    for block in &input.raw_blocks {
        builder = builder.raw_system_block(block.name, &block.content, block.priority);
    }

    // message_source BEFORE connector_guidance/tools to preserve byte-identical
    // order vs pre-migration Skill invocation (invocation.rs calls
    // `.message_source(...)` before tool resolution/`.tools(...)`).
    if let Some(ref ms) = input.message_source {
        builder = builder.message_source(ms);
    }

    // PromptBuilder's `connector_guidance` expects `&[(String, String)]` +
    // optional sendable list — adapt from our ConnectorSummary wrapper.
    if !input.connector_status.is_empty() {
        let statuses: Vec<(String, String)> = input
            .connector_status
            .iter()
            .map(|s| (s.id.clone(), s.status.clone()))
            .collect();
        let sendable: Vec<String> = input
            .connector_status
            .iter()
            .filter(|s| s.sendable)
            .map(|s| s.id.clone())
            .collect();
        let sendable_slice = if sendable.is_empty() {
            None
        } else {
            Some(sendable.as_slice())
        };
        builder = builder.connector_guidance(&statuses, sendable_slice);
    }

    if !input.tools.is_empty() {
        builder = builder.tools(&input.tools);
    }

    if let Some(ref stc) = input.send_tool_context {
        builder = builder.raw_system_block("send_context", stc, SectionPriority::Normal);
    }

    let built = builder.build();

    StaticPromptOutput {
        system_message: Arc::<str>::from(built.system_message),
        section_registry: built.section_registry,
        fingerprint: compute_fingerprint(input),
    }
}

fn build_social_minimal(input: &StaticPromptInput) -> StaticPromptOutput {
    // Replicates crates/openalpaca_core/src/orchestrator/query_handler/
    // simple_query_handler.rs:512-526 verbatim.
    //
    //     let system_prompt = format!(
    //         "<system_instructions>\n{}\n</system_instructions>\n\n\
    //          <agent_role>\nRole: Assistant\nTone: Concise and professional\n</agent_role>",
    //         system_persona.base_instructions
    //     );
    //
    // Layer 1 in Minimal mode already placed system_persona.base_instructions
    // (verbatim) as the content of the first block, so we pull from there.
    let persona_text = input
        .persona_output
        .blocks
        .first()
        .map(|b| b.content.as_ref())
        .unwrap_or("");

    let system_message = format!(
        "<system_instructions>\n{}\n</system_instructions>\n\n\
         <agent_role>\nRole: Assistant\nTone: Concise and professional\n</agent_role>",
        persona_text
    );

    StaticPromptOutput {
        system_message: Arc::<str>::from(system_message),
        section_registry: vec!["social_minimal"],
        fingerprint: compute_fingerprint(input),
    }
}
