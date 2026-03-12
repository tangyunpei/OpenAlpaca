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
    /// Extended thinking config (Anthropic only).
    pub thinking: Option<ThinkingConfig>,
    /// Context management configuration (Anthropic only).
    pub context_management: Option<crate::context_management::ContextManagement>,
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
pub struct ProviderEntry {
    pub provider: Arc<dyn LlmProvider>,
    pub key_pool: Arc<ArcSwap<KeyPool>>,
}
