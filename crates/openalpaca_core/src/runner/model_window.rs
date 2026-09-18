//! Which context window a run is actually budgeted against (M5).
//!
//! Every prompt budget in the lead-agent path used to read the window of the
//! model the agent *template pins*, or a hard-coded 200 000 — and a shipped
//! template pins a Claude id (`config/agents/lead_agent.md`). On an
//! Ollama-only install that pin is not routable: L3 substitutes, the call is
//! answered by an 8 192-token local model, and the budget said 200 000. The
//! loop then never compacted, and the provider refused the overflowing
//! request instead.
//!
//! So the window is read from the model that will *answer*, resolved the same
//! way the router resolves it, and a registry that knows no window for it
//! leaves each call site on its own documented default rather than on a zero.

use openalpaca_llm::LlmRouter;

/// The model a call pinning `model_id` will actually be sent as (L3's ladder,
/// as far as a caller outside the router can walk it).
///
/// The pin when it is routable — the ordinary cloud case, unchanged. Otherwise
/// the first routable rung of: the model's configured fallback chain, the
/// configured default's chain, then the effective default. `None` when nothing
/// at all is routable, which is the state `NoRoutableModel` reports on the
/// next call.
///
/// This is a *reader's* copy of [`LlmRouter::resolve_routable`], which is
/// crate-private to the router. It is deliberately not a second policy: it
/// walks the same rungs in the same order off the router's own public
/// accessors, and the one rung it cannot see — a per-request
/// `fallback_models` chain — is empty on every path that asks about a window.
pub fn routed_model(router: &LlmRouter, model_id: Option<&str>) -> Option<String> {
    let configured_default = router.default_model();
    let pinned = model_id
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or(configured_default.as_str());

    if router.is_routable(pinned) {
        return Some(pinned.to_string());
    }
    let chains = router
        .fallback_models(pinned)
        .into_iter()
        .flatten()
        .chain(router.fallback_models(&configured_default).into_iter().flatten());
    for candidate in chains {
        if router.is_routable(candidate) {
            return Some(candidate.clone());
        }
    }
    router.effective_default_model()
}

/// The context window of the model that will answer, in tokens.
///
/// `None` when nothing is routable, when the registry holds no entry for the
/// model that will answer, or when the entry's window is `0` — a window of
/// zero must never reach compaction, which divides by it. Callers apply their
/// own documented default to a `None`.
pub fn routed_context_window(router: &LlmRouter, model_id: Option<&str>) -> Option<usize> {
    let model = routed_model(router, model_id)?;
    router
        .model_registry()
        .get_model_info(&model)
        .map(|info| info.context_window as usize)
        .filter(|w| *w > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_llm::routing::model_registry::ModelInfo;
    use openalpaca_llm::{ProviderType, routing::model_registry::ModelRegistry};

    /// A router holding one local model, with the recording provider behind
    /// it: `default_model` is the Claude id every shipped template pins, and
    /// Anthropic is *not* loaded.
    fn ollama_only_router() -> LlmRouter {
        let provider = crate::test_util::RecordingProvider::new("hi");
        let router = LlmRouter::new(
            std::collections::HashMap::new(),
            ModelRegistry::with_defaults(),
            std::collections::HashMap::new(),
            std::sync::Arc::new(openalpaca_llm::routing::cost_tracker::CostTracker::new(
                ModelRegistry::with_defaults(),
            )),
            "claude-sonnet-4-6".to_string(),
        );
        router.model_registry().register(
            "qwen3:8b".to_string(),
            ModelInfo {
                provider: ProviderType::Ollama,
                input_price_per_million: 0.0,
                output_price_per_million: 0.0,
                context_window: 8192,
                discovered: true,
                supports_image: false,
                supports_audio: false,
                supports_document: false,
                supports_reasoning: false,
                supports_tools: true,
                declared: false,
            },
        );
        router.register_provider(
            ProviderType::Ollama,
            provider,
            openalpaca_llm::keys::key_pool::KeyPool::new(
                Vec::new(),
                openalpaca_llm::keys::key_pool::SelectionStrategy::RoundRobin,
            ),
        );
        router
    }

    /// **M5.** The lead's template pins Claude (200 000) and the only loaded
    /// provider is a local Ollama: the budget is the local model's 8 192, not
    /// the pin's, or the loop never compacts and the provider refuses the
    /// request.
    #[test]
    fn an_unroutable_pin_is_budgeted_against_the_model_that_answers() {
        let router = ollama_only_router();
        assert_eq!(
            routed_context_window(&router, Some("claude-sonnet-4-6")),
            Some(8192)
        );
        assert_eq!(
            routed_model(&router, Some("claude-sonnet-4-6")).as_deref(),
            Some("qwen3:8b")
        );
    }

    /// A routable pin is read as itself — the cloud case, unchanged.
    #[test]
    fn a_routable_pin_keeps_its_own_window() {
        let router = ollama_only_router();
        assert_eq!(routed_context_window(&router, Some("qwen3:8b")), Some(8192));
    }

    /// Nothing routable is `None`, so the caller falls back to its documented
    /// default instead of budgeting against zero.
    #[test]
    fn nothing_routable_has_no_window() {
        let router = LlmRouter::new(
            std::collections::HashMap::new(),
            ModelRegistry::with_defaults(),
            std::collections::HashMap::new(),
            std::sync::Arc::new(openalpaca_llm::routing::cost_tracker::CostTracker::new(
                ModelRegistry::with_defaults(),
            )),
            "claude-sonnet-4-6".to_string(),
        );
        assert_eq!(routed_context_window(&router, Some("claude-sonnet-4-6")), None);
    }

    /// A registered model whose window is `0` — a discovery that could not
    /// read one, a hand-written `[models]` row — is *not* a window. It must
    /// never reach compaction, which divides by it.
    #[test]
    fn a_zero_window_is_not_a_window() {
        let router = ollama_only_router();
        router.model_registry().register(
            "broken:0".to_string(),
            ModelInfo {
                provider: ProviderType::Ollama,
                input_price_per_million: 0.0,
                output_price_per_million: 0.0,
                context_window: 0,
                discovered: true,
                supports_image: false,
                supports_audio: false,
                supports_document: false,
                supports_reasoning: false,
                supports_tools: true,
                declared: false,
            },
        );
        assert_eq!(routed_context_window(&router, Some("broken:0")), None);
    }
}
