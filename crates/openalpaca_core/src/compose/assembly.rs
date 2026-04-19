//! Layer 5 — Assembly. Deterministic stitching of the four upstream outputs.
//!
//! Pure function; never memoized. Phase 3 adds real token accounting and
//! head-of-history trimming per spec (§Component 1 Layer 5).

use std::sync::Arc;

use openalpaca_llm::{ChatMessage, Role, ToolDefinition};

use super::fingerprint::combine_composite;
use super::types::{
    ComposedFingerprints, ComposedRequest, DynamicContextOutput, HistoryOutput, LayerTrace,
    PersonaOutput, StaticPromptOutput, TokenBudget,
};

/// Budget threshold fraction: prompt must fit in `model_window * TRIM_THRESHOLD`.
/// Matches `PromptBuilder::build`'s 75% budget so assembly + static-prompt agree.
const TRIM_THRESHOLD: f64 = 0.75;

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
    let mut section_registry: Vec<&'static str> = input.static_prompt.section_registry.clone();

    // Stitch: system prompt first, then additional system blocks from dynamic
    // context, then the dynamic context messages, then the history messages.
    if !input.static_prompt.system_message.is_empty() {
        messages.push(ChatMessage::system(&input.static_prompt.system_message));
    }
    for block in &input.dynamic_context.additional_system_blocks {
        messages.push(ChatMessage::system(&block.content));
    }
    messages.extend(input.dynamic_context.context_messages.iter().cloned());
    messages.extend(input.history.messages.iter().cloned());

    // === Per-layer token accounting ==========================================
    let static_prompt_tokens =
        estimate_string_tokens(input.static_prompt.system_message.as_ref()) as u32;
    let persona_tokens: u32 = input
        .persona
        .blocks
        .iter()
        .map(|b| estimate_string_tokens(b.content.as_ref()) as u32)
        .sum();
    let dynamic_ctx_block_tokens: u32 = input
        .dynamic_context
        .additional_system_blocks
        .iter()
        .map(|b| estimate_string_tokens(b.content.as_ref()) as u32)
        .sum();
    let dynamic_ctx_msg_tokens =
        crate::runner::estimate_messages_tokens(&input.dynamic_context.context_messages);
    let dynamic_context_tokens = dynamic_ctx_block_tokens + dynamic_ctx_msg_tokens;
    let mut history_tokens =
        crate::runner::estimate_messages_tokens(&input.history.messages);

    // === Budget trim =========================================================
    // If the stitched messages exceed `model_window * TRIM_THRESHOLD`, drop the
    // oldest non-system messages until we fit. The system message at index 0
    // is preserved — the spec is explicit that Layer 5 trims from the head of
    // history, not from the system prompt.
    let threshold = (input.model_window as f64 * TRIM_THRESHOLD) as u32;
    let mut total = crate::runner::estimate_messages_tokens(&messages);
    let mut trimmed = 0usize;
    while total > threshold && messages.len() > 1 {
        let Some(idx_to_trim) = messages
            .iter()
            .position(|m| !matches!(m.role, Role::System))
        else {
            // No non-system messages left — stop trimming (spec forbids
            // removing the system prompt).
            break;
        };
        let removed = messages.remove(idx_to_trim);
        let removed_tokens = crate::runner::estimate_messages_tokens(std::slice::from_ref(&removed));
        total = total.saturating_sub(removed_tokens);
        history_tokens = history_tokens.saturating_sub(removed_tokens);
        trimmed += 1;
    }
    if trimmed > 0 {
        section_registry.push("<trimmed:history>");
    }

    let current_turn_tokens = input
        .history
        .messages
        .last()
        .map(|m| crate::runner::estimate_messages_tokens(std::slice::from_ref(m)))
        .unwrap_or(0);

    let token_budget = TokenBudget {
        persona_tokens,
        static_prompt_tokens,
        dynamic_context_tokens,
        history_tokens,
        current_turn_tokens,
        total,
        model_window: input.model_window,
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
        section_registry,
        layer_trace: input.layer_trace,
    }
}

/// Rough token estimate for a bare string. Uses the same `len/4` heuristic as
/// `estimate_messages_tokens` so per-layer token accounting stays consistent
/// with the runner's global estimate. Kept private — external callers should
/// use `runner::estimate_messages_tokens`.
fn estimate_string_tokens(s: &str) -> usize {
    s.len() / 4
}
