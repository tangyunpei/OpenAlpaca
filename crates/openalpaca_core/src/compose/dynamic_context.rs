//! Layer 3 — Dynamic Context.
//!
//! Converts a pre-resolved `ContextBundle` (produced async by `ContextManager`)
//! into `context_messages` + `additional_system_blocks`. The layer stays pure
//! and sync — callers resolve the bundle before invoking compose.
//!
//! Modes:
//! - `Default` — walk the bundle's sections, routing each to the appropriate
//!   output slot based on its `InjectionMode`.
//! - `Skip`    — produce an empty output (used by planner, replanner, social,
//!   pipeline, dag, lead-agent paths per the spec default-modes table).
//!
//! Fingerprint: `blake3(query_hash || memory_retrieval_hash || path_tag ||
//! reserved_tokens || mode_tag)`. The `query_hash` captures the request text;
//! the `memory_retrieval_hash` is pre-computed by the caller over the
//! bundle's serialized content so identical retrieval outputs map to the
//! same fingerprint even when the bundle's internal timestamps differ.

use std::sync::Arc;

use openalpaca_llm::ChatMessage;

use super::types::{
    DynamicContextInput, DynamicContextMode, DynamicContextOutput, SystemBlock,
};
use crate::prompt_ctx::{ExecutionPath, InjectionMode, SectionPriority, TrustLevel};

fn mode_tag(mode: DynamicContextMode) -> u8 {
    match mode {
        DynamicContextMode::Default => 0,
        DynamicContextMode::Skip => 1,
    }
}

fn path_tag(path: &ExecutionPath) -> u8 {
    match path {
        ExecutionPath::SimpleQuery => 0,
        ExecutionPath::SkillInvocation { .. } => 2,
        ExecutionPath::LeadAgent => 5,
    }
}

pub fn compute(input: &DynamicContextInput) -> DynamicContextOutput {
    let fingerprint = compute_fingerprint(input);

    match input.mode {
        DynamicContextMode::Skip => DynamicContextOutput {
            context_messages: Vec::new(),
            additional_system_blocks: Vec::new(),
            fingerprint,
        },
        DynamicContextMode::Default => {
            let (context_messages, additional_system_blocks) =
                build_from_bundle(&input.context_bundle);
            DynamicContextOutput {
                context_messages,
                additional_system_blocks,
                fingerprint,
            }
        }
    }
}

/// Walk the bundle's sections once, routing each to the appropriate output
/// slot based on its `InjectionMode`. Matches the live call-site behavior in
/// `PromptBuilder::context_bundle` so Layer 3's output is a drop-in
/// replacement for the dynamic portion of the old prompt pipeline.
fn build_from_bundle(
    bundle: &crate::prompt_ctx::ContextBundle,
) -> (Vec<ChatMessage>, Vec<SystemBlock>) {
    let mut context_messages: Vec<ChatMessage> = Vec::new();
    let mut additional_system_blocks: Vec<SystemBlock> = Vec::new();

    for section in &bundle.sections {
        if section.content.is_empty() {
            continue;
        }
        match &section.injection {
            InjectionMode::SystemPrompt => {
                additional_system_blocks.push(SystemBlock {
                    name: section.source,
                    content: Arc::<str>::from(section.content.as_str()),
                    priority: section.priority,
                });
            }
            InjectionMode::SystemMessage => {
                additional_system_blocks.push(SystemBlock {
                    name: section.source,
                    content: Arc::<str>::from(section.content.as_str()),
                    priority: SectionPriority::High,
                });
            }
            InjectionMode::UserMessage { tag, trust } => {
                let msg_content = match trust {
                    TrustLevel::Untrusted => crate::orchestrator::wrap_untrusted_context(
                        &section.content,
                        tag,
                        "retrieved",
                    ),
                    TrustLevel::Trusted => section.content.clone(),
                };
                context_messages.push(ChatMessage::user(&msg_content));
            }
        }
    }

    (context_messages, additional_system_blocks)
}

/// Public fingerprint helper used by `ComposeEngine::lookup_or_build_dynamic_context`
/// to compute the cache key before the output is built.
pub(super) fn compute_fingerprint(input: &DynamicContextInput) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(blake3::hash(input.query.as_bytes()).as_bytes());
    h.update(&input.memory_retrieval_hash);
    h.update(&[path_tag(&input.path)]);
    h.update(&(input.reserved_tokens as u64).to_le_bytes());
    h.update(&[mode_tag(input.mode)]);
    h.finalize().into()
}
