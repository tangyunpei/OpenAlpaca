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
    let mut history_start = messages.len();
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
    let mut dynamic_context_tokens = dynamic_ctx_block_tokens + dynamic_ctx_msg_tokens;
    let mut history_tokens =
        crate::runner::estimate_messages_tokens(&input.history.messages);

    // === Budget trim =========================================================
    // If the stitched messages exceed `model_window * TRIM_THRESHOLD`, drop the
    // oldest non-system messages until we fit, then Layer 3's system blocks,
    // oldest first. The system prompt at index 0 is preserved — the spec is
    // explicit that Layer 5 trims from the head of history, not from the
    // system prompt. So is the working-state block (design 11 §4, CQ-4): it
    // is the one carrier of what the trim removes, so the trim takes the
    // message after it. The previous conversation's block may go, and is not
    // counted as a history row when it does (§3.2). A Layer 3 message, a
    // system block or a section routed as a user message, is not a history
    // row either: its tokens come off the dynamic context's figure, which
    // counted them. The current user turn is kept too (SA-2b): it is what
    // the request asks, so a turn that alone passes the threshold goes
    // whole, and a context-length error, if the provider returns one, is the
    // caller's — never a request with no user message (on Anthropic, an
    // empty `messages` list).
    let threshold = (input.model_window as f64 * TRIM_THRESHOLD) as u32;
    let mut total = crate::runner::estimate_messages_tokens(&messages);
    let mut history_trimmed = 0usize;
    let mut blocks_trimmed = 0usize;
    let mut history_head_dropped = 0usize;
    let system_prompt_at = (!input.static_prompt.system_message.is_empty()).then_some(0);
    let mut current_turn_at = input
        .history
        .ends_with_current_turn
        .then(|| messages.len() - 1);
    let is_kept = |i: usize, m: &ChatMessage, current: Option<usize>| {
        Some(i) == system_prompt_at
            || Some(i) == current
            || m.content
                .starts_with(crate::orchestrator::working_state::BLOCK_PREFIX)
    };
    while total > threshold && messages.len() > 1 {
        let candidate = |system: bool| {
            messages
                .iter()
                .enumerate()
                .find(|(i, m)| {
                    matches!(m.role, Role::System) == system && !is_kept(*i, m, current_turn_at)
                })
                .map(|(i, _)| i)
        };
        let Some(idx_to_trim) = candidate(false).or_else(|| candidate(true)) else {
            // Nothing left the trim may take — stop (spec forbids removing
            // the system prompt, and the working state and the current turn
            // stay).
            break;
        };
        let removed = messages.remove(idx_to_trim);
        if let Some(current) = current_turn_at.as_mut()
            && idx_to_trim < *current
        {
            *current -= 1;
        }
        let system = matches!(removed.role, Role::System);
        let in_history = idx_to_trim >= history_start;
        if !in_history {
            history_start -= 1;
            blocks_trimmed += usize::from(system);
        } else if !removed
            .content
            .starts_with(crate::orchestrator::working_state::PREVIOUS_BLOCK_PREFIX)
        {
            history_head_dropped += 1;
        }
        let removed_tokens = crate::runner::estimate_messages_tokens(std::slice::from_ref(&removed));
        total = total.saturating_sub(removed_tokens);
        if in_history {
            history_trimmed += 1;
            history_tokens = history_tokens.saturating_sub(removed_tokens);
        } else {
            // A Layer 3 message: its tokens come off the figure that counted it.
            dynamic_context_tokens = dynamic_context_tokens.saturating_sub(removed_tokens);
        }
    }
    if history_trimmed > 0 {
        section_registry.push("<trimmed:history>");
    }
    if blocks_trimmed > 0 {
        section_registry.push("<trimmed:system_blocks>");
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
        history_head_dropped,
    }
}

/// Rough token estimate for a bare string. Uses the same `len/4` heuristic as
/// `estimate_messages_tokens` so per-layer token accounting stays consistent
/// with the runner's global estimate. Kept private — external callers should
/// use `runner::estimate_messages_tokens`.
fn estimate_string_tokens(s: &str) -> usize {
    s.len() / 4
}
