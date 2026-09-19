use crate::LlmProvider;
use crate::error::LlmError;
use crate::keys::key_pool::KeyPool;
use crate::types::*;
use arc_swap::ArcSwap;
use std::sync::Arc;

/// Context for a router request (agent/task identification).
#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    pub agent_id: Option<String>,
    pub task_id: Option<String>,
}

/// A request to the LLM router.
pub struct RouterRequest {
    pub model: Option<String>,
    pub messages: Arc<Vec<ChatMessage>>,
    pub tools: Arc<Vec<ToolDefinition>>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub context: RequestContext,
    pub tool_choice: Option<ToolChoice>,
    /// Pre-computed token estimate for tool definitions.
    /// When `Some`, `estimate_request_tokens` skips JSON re-serialization of tools.
    pub tools_token_estimate: Option<u32>,
    /// Enable Anthropic prompt caching.
    pub enable_caching: bool,
    /// How much the caller wants this model to reason; see
    /// [`ChatRequest::thinking`](crate::types::ChatRequest::thinking). Forwarded
    /// unchanged to whichever model the ladder settles on, so an internal call
    /// that asked for no reasoning still asks for none on the fallback rung.
    pub thinking: Option<ThinkingConfig>,
    /// Context management configuration (Anthropic only).
    pub context_management: Option<crate::context_management::ContextManagement>,
    /// Per-request fallback model chain (overrides global fallback when non-empty).
    pub fallback_models: Vec<String>,
    /// See ChatRequest::ephemeral_system_notice.
    pub ephemeral_system_notice: Option<String>,
}

/// A started stream, and the model the router actually called (V4).
///
/// The caller cannot work the effective model out for itself: the ladder (L3)
/// may answer a request for one id with another, and a stream carries no
/// `model` field of its own the way a non-streaming response body does. It
/// used to guess — `request.model` or the daemon default — so a turn answered
/// by a local model was labelled with the Anthropic default end to end, and
/// priced against it. The router resolved it; the router says so.
pub struct RoutedStream {
    /// The ladder's effective model: what answered, not what was asked for.
    pub model: String,
    pub stream: ChatStream,
}

/// Errors from the LLM router.
#[derive(Debug, Clone, thiserror::Error)]
pub enum LlmRouterError {
    #[error("Unknown model: {0}")]
    UnknownModel(String),

    #[error("Provider not configured: {0}")]
    ProviderNotConfigured(String),

    #[error("All keys are rate-limited for provider")]
    AllKeysRateLimited,

    #[error("No API-compatible keys configured for provider (only managed/OAuth keys found)")]
    NoApiCompatibleKeys,

    #[error("Max retries exceeded")]
    MaxRetriesExceeded,

    #[error("All fallback models failed")]
    AllFallbacksFailed,

    /// Nothing in the catalogue belongs to a provider that is loaded, so no
    /// model can be called at all. Replaces the "Unknown model: …" and "All
    /// fallback models failed" an owner used to see for a daemon that simply
    /// had no provider switched on (L3).
    #[error(
        "No model is available: no enabled provider offers one. Turn one on — Settings → \
         Models in the GUI, or `openalpaca config set ai.<provider>.enabled true` — then \
         `openalpaca llm status` says what is loaded. A local Ollama needs no API key, only \
         `ollama pull <model>`."
    )]
    NoRoutableModel,

    #[error("No fallback chain configured for model: {0}")]
    NoFallbackAvailable(String),

    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),
}

/// Structured LLM capacity information for adaptive concurrency decisions.
///
/// Returned by [`super::LlmRouter::estimated_llm_capacity`] so callers can base
/// stagger delay on the raw key count and reserve slots for the lead agent.
#[derive(Debug, Clone)]
pub struct LlmCapacityInfo {
    /// Number of API-compatible keys currently available (not in cooldown).
    pub available_api_keys: usize,
    /// Max concurrent calls allowed per key (from rate limiter config).
    pub per_key_concurrency: usize,
    /// `available_api_keys * per_key_concurrency`.
    pub key_capacity: usize,
    /// Whether a CLI fallback backend is registered for this provider.
    pub has_cli_fallback: bool,
    /// Effective parallel capacity: `min(key_capacity, global_available)`.
    /// When `key_capacity == 0` and `has_cli_fallback`, this is 1 (fallback only).
    pub effective_capacity: usize,
}

/// A provider entry with its key pool (swappable for hot-reload).
///
/// Both halves are `Arc`s, so cloning one is two refcount bumps — which is how
/// a request takes what it needs out of the router's `DashMap` and lets the
/// shard's lock go before it awaits anything (R59). A request that kept the
/// `Ref` would block `deregister_provider` for the length of its call.
#[derive(Clone)]
pub struct ProviderEntry {
    pub provider: Arc<dyn LlmProvider>,
    pub key_pool: Arc<ArcSwap<KeyPool>>,
}
