pub mod cli_backend;
pub mod config;
pub mod cost_tracker;
pub mod credential_discovery;
pub mod error;
pub mod key_encryption;
pub mod key_pool;
pub mod model_registry;
pub mod provider_usage;
pub mod providers;
pub mod router;
pub mod settings_service;
pub mod types;

pub use cli_backend::{CliBackendsConfig, CliBackendConfig, CliBackendStatus, ClaudeCodeCliProvider, CodexCliProvider, detect_cli_backends};
pub use config::{LlmConfig, LlmRouterConfig, ProviderConfig, KeyConfig, OrchestratorLlmConfig, build_provider, build_router, read_config, write_config};
pub use cost_tracker::{CallRecord, CostTracker, ModelUsageStats, UsageStats};
pub use credential_discovery::{CredentialDiscoveryConfig, CredentialSource, DiscoveredCredential, DiscoveredCredentialInfo, OAuthToken, TokenManager};
pub use error::LlmError;
pub use key_pool::{ApiKey, CallResult, KeyGuard, KeyHealthStatus, KeyPool, KeyPoolError, KeyPriority, KeySource, KeyStatus, ProviderType, SelectionStrategy, mask_secret};
pub use model_registry::{ModelEntry, ModelInfo, ModelRegistry, PricingInfo};
pub use provider_usage::{ExternalUsage, ProviderUsageSummary, ProviderUsageTracker};
pub use router::{LlmRouter, LlmRouterError, ProviderEntry, RequestContext, RouterRequest};
pub use settings_service::{LlmSettingsService, OrchestratorConfigResponse, UpdateOrchestratorRequest};
pub use types::*;

use async_trait::async_trait;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn supports_tools(&self) -> bool;
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError>;

    /// Chat using a specific API key. Default delegates to `chat()`.
    /// Providers override this to inject the key into their HTTP requests.
    async fn chat_with_key(&self, _key: &str, request: ChatRequest) -> Result<ChatResponse, LlmError> {
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
