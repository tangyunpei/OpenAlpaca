pub mod cli_backend;
pub mod config;
pub mod embedder;
pub mod error;
pub mod keys;
pub mod providers;
pub mod routing;
pub mod types;

// TODO: Remove backward-compat re-exports once all consumers use canonical paths
// (apps/ imports were updated in the reorganize branch; external consumers may remain)
pub use keys::credential_discovery;
pub use keys::key_encryption;
pub use keys::key_pool;
pub use keys::secret_store;
pub use config::settings_service;
pub use routing::cost_tracker;
pub use routing::model_registry;
pub use routing::provider_usage;
pub use routing::rate_limiter;
pub use routing::router;

pub use cli_backend::{
    ClaudeCodeCliProvider, CliBackendConfig, CliBackendStatus, CliBackendsConfig, CodexCliProvider,
    detect_cli_backends,
};
pub use config::llm_config::{
    EmbeddingsConfig, EndpointsConfig, EnvVarsConfig, KeyConfig, LlmConfig, LlmRouterConfig,
    LlmRuntimeConfig, ModelConfigEntry, OrchestratorLlmConfig, ProviderConfig, ProviderDefaults,
    SecurityConfig, TimeoutsConfig, build_provider, build_provider_with_runtime, build_router,
    build_router_with_secret_store, collect_secret_refs, migrate_llm_secrets, read_config,
    resolve_key_from_config, reverse_migrate_llm_secrets, write_config,
};
pub use config::settings_service::{
    LlmSettingsService, OrchestratorConfigResponse, UpdateOrchestratorRequest,
};
pub use embedder::{EmbedError, Embedder, build_embedder, build_embedder_with_runtime};
pub use error::LlmError;
pub use keys::credential_discovery::{
    CredentialDiscoveryConfig, CredentialSource, DiscoveredCredential, DiscoveredCredentialInfo,
    OAuthToken, TokenManager,
};
pub use keys::key_pool::{
    ApiKey, CallResult, KeyGuard, KeyHealthStatus, KeyPool, KeyPoolError, KeyPriority, KeySource,
    KeyStatus, ProviderType, SelectionStrategy, mask_secret,
};
pub use keys::secret_store::{CachingSecretStore, KeyringSecretStore, MemorySecretStore, SecretStore};
pub use routing::cost_tracker::{CallRecord, CostSnapshot, CostTracker, ModelUsageStats, UsageStats};
pub use routing::model_registry::{ModelEntry, ModelInfo, ModelRegistry, PricingInfo};
pub use routing::provider_usage::{ExternalUsage, ProviderUsageSummary, ProviderUsageTracker};
pub use routing::rate_limiter::{CircuitState, RateLimitConfig, RateLimiterRegistry, backoff_with_jitter};
pub use routing::router::{
    LlmCapacityInfo, LlmRouter, LlmRouterError, ProviderEntry, RequestContext, RouterRequest,
};
pub use types::*;

use async_trait::async_trait;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn supports_tools(&self) -> bool;
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError>;

    /// Chat using a specific API key. Default delegates to `chat()`.
    /// Providers override this to inject the key into their HTTP requests.
    async fn chat_with_key(
        &self,
        _key: &str,
        request: ChatRequest,
    ) -> Result<ChatResponse, LlmError> {
        self.chat(request).await
    }

    /// List models available from this provider using the given API key.
    /// Default returns empty. Providers override with real API calls.
    async fn list_models_with_key(&self, _key: &str) -> Result<Vec<String>, LlmError> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_serialization() {
        let json = serde_json::to_string(&Role::System).unwrap();
        assert_eq!(json, "\"system\"");
        let json = serde_json::to_string(&Role::User).unwrap();
        assert_eq!(json, "\"user\"");
        let json = serde_json::to_string(&Role::Assistant).unwrap();
        assert_eq!(json, "\"assistant\"");
        let json = serde_json::to_string(&Role::Tool).unwrap();
        assert_eq!(json, "\"tool\"");
    }

    #[test]
    fn test_chat_message_with_tools() {
        let msg = ChatMessage {
            role: Role::Assistant,
            content: "Let me help.".to_string(),
            parts: None,
            tool_calls: Some(vec![ToolCall {
                id: "tc1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"query": "rust"}),
            }]),
            tool_call_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("tool_calls"));
        assert!(json.contains("search"));
        // tool_call_id should be absent (skip_serializing_if)
        assert!(!json.contains("tool_call_id"));
    }

    #[test]
    fn test_chat_message_without_tools() {
        let msg = ChatMessage::user("hello");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("tool_calls"));
        assert!(!json.contains("tool_call_id"));
    }

    #[test]
    fn test_usage_default() {
        let usage = Usage::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
    }

    #[test]
    fn test_finish_reason_eq() {
        assert_eq!(FinishReason::Stop, FinishReason::Stop);
        assert_ne!(FinishReason::Stop, FinishReason::ToolUse);
        assert_eq!(FinishReason::MaxTokens, FinishReason::MaxTokens);
    }
}
