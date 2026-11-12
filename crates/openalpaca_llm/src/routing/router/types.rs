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
    /// Whether the provider serving the stream needs a key
    /// ([`LlmProvider::requires_key`]): `false` for a local model, whose
    /// streamed round the agentic loop allows an hour where it allows any
    /// other ten minutes (LOCAL-CALL-HOUR).
    ///
    /// [`LlmProvider::requires_key`]: crate::LlmProvider::requires_key
    pub requires_key: bool,
    pub stream: ChatStream,
}

/// How much of a model's rate budget is left right now (Learning v2, design
/// 09 §1.7): the minimum over its provider's keys, per bucket. Background
/// work reads it before it spends any, so a chat round that starts meanwhile
/// still finds the key's buckets at least half full.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateHeadroom {
    pub rpm_available: f64,
    pub rpm_capacity: f64,
    pub tpm_available: f64,
    pub tpm_capacity: f64,
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

    /// The provider refused the credentials on every attempt the call made —
    /// an HTTP 401 or 403, or its own authentication failure — on each key
    /// the pool offered, and nothing else went wrong. Not transient: the same
    /// keys are refused again until one is fixed, so a caller does not retry
    /// it (N-LEAD-5). Before it existed the rotation ended as
    /// [`Self::MaxRetriesExceeded`], then the fallback ladder's
    /// [`Self::AllFallbacksFailed`], which read as a passing failure.
    #[error(
        "The provider refused the API key on every attempt (authentication failed). Fix or \
         replace the key — Settings → Models & keys in the GUI, or `openalpaca llm keys add` \
         — then try again."
    )]
    AllKeysRefused,

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

    /// A call to a provider that needs no key — a local model — used up its
    /// whole timeout with no answer: the deadline of a non-streaming call, the
    /// read gap of a streamed one before its stream opened, or a streamed
    /// round's own bound
    /// (LOCAL-HOUR-NO-RETRY, the owner's answer of 2026-11-12). Not
    /// transient: that provider is not asked again — not on its slot, not on
    /// the fallback ladder, not by the agentic loop, and a streamed round not
    /// again without streaming. A rung on another provider may still answer
    /// (`LlmRouter::complete_elsewhere`); when none does, this is the call's
    /// error, so the turn ends naming it.
    #[error(
        "{model} ({provider}, a local model) did not finish answering within its whole \
         timeout of {timeout_secs} s, so it was not asked again. A smaller model or a shorter \
         request may finish in time. A non-streaming call's timeout is \
         `request_timeout_secs` under `[providers.{provider}]` in llm.toml; a streamed \
         round's is fixed."
    )]
    LocalCallTimedOut {
        model: String,
        provider: String,
        timeout_secs: u64,
    },

    #[error("No fallback chain configured for model: {0}")]
    NoFallbackAvailable(String),

    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),
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

/// What [`LlmRouter::resolve_alias`](super::LlmRouter::resolve_alias)
/// settled on for an alias (M-4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasResolution {
    /// The routable model the alias names here.
    pub model: String,
    /// The candidates passed over on the way, because they were not
    /// routable: the parent's provider's pin, then its compiled id. Each is
    /// announced once (M-5).
    pub passed_over: Vec<String>,
}
