//! Layer 5 — Assembly. Deterministic stitching of the four upstream outputs.
//!
//! Phase 1 implementation is the real one (pure function, never memoized);
//! Phase 3 expands to real token accounting and history-tail trimming.

use std::sync::Arc;

use openalpaca_llm::{ChatMessage, ToolDefinition};

use super::fingerprint::combine_composite;
use super::types::{
    ComposedFingerprints, ComposedRequest, DynamicContextOutput, HistoryOutput, LayerTrace,
    PersonaOutput, StaticPromptOutput, TokenBudget,
};

pub struct AssemblyInput<'a> {
    pub persona: &'a PersonaOutput,
    pub static_prompt: &'a StaticPromptOutput,
    pub dynamic_context: &'a DynamicContextOutput,
    pub history: &'a HistoryOutput,
    pub tools: Arc<Vec<ToolDefinition>>,
    pub model_window: u32,
    pub layer_trace: LayerTrace,
}

pub fn compose(input: AssemblyInput<'_>) -> ComposedRequest {
    let mut messages: Vec<ChatMessage> = Vec::new();

    if !input.static_prompt.system_message.is_empty() {
        messages.push(ChatMessage::system(&input.static_prompt.system_message));
    }

    for block in &input.dynamic_context.additional_system_blocks {
        messages.push(ChatMessage::system(&block.content));
    }

    messages.extend(input.dynamic_context.context_messages.iter().cloned());
    messages.extend(input.history.messages.iter().cloned());

    // Phase 1: trivial token budget (all zero). Phase 3 implements real
    // accounting plus head-of-history trimming per spec.
    let token_budget = TokenBudget {
        model_window: input.model_window,
        ..Default::default()
    };

    let fingerprints = ComposedFingerprints {
        persona: input.persona.fingerprint,
        static_prompt: input.static_prompt.fingerprint,
        dynamic_context: input.dynamic_context.fingerprint,
        history: input.history.fingerprint,
        composite: combine_composite(
            &input.persona.fingerprint,
            &input.static_prompt.fingerprint,
            &input.dynamic_context.fingerprint,
            &input.history.fingerprint,
        ),
    };

    ComposedRequest {
        messages: Arc::new(messages),
        tools: input.tools,
        fingerprints,
        token_budget,
        section_registry: input.static_prompt.section_registry.clone(),
        layer_trace: input.layer_trace,
    }
}
