pub mod cli_backend;
pub mod config;
pub mod context_management;
pub mod embedder;
pub mod error;
pub mod keys;
pub mod providers;
pub mod routing;
pub mod streaming;
pub mod types;

// TODO: Remove backward-compat re-exports once all consumers use canonical paths
// (apps/ imports were updated in the reorganize branch; external consumers may remain)
pub use config::settings_service;
pub use keys::credential_discovery;
pub use keys::key_encryption;
pub use keys::key_pool;
pub use keys::secret_store;
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
    EmbeddingsConfig, EndpointsConfig, EnvVarsConfig, KeyConfig, LlmRouterConfig, LlmRuntimeConfig,
    ModelConfigEntry, OrchestratorLlmConfig, ProviderConfig, ProviderDefaults, SecurityConfig,
    TimeoutsConfig, WebSearchConfig, build_router, build_router_with_secret_store,
    collect_secret_refs, migrate_llm_secrets, read_config, resolve_key_from_config,
    reverse_migrate_llm_secrets, write_config,
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
pub use keys::secret_store::{
    CachingSecretStore, KeyringSecretStore, MemorySecretStore, SecretStore,
};
pub use routing::cost_tracker::{
    CacheStats, CallRecord, CostSnapshot, CostTracker, ModelUsageStats, UsageStats,
};
pub use routing::model_registry::{ModelEntry, ModelInfo, ModelRegistry, PricingInfo};
pub use routing::provider_usage::{ExternalUsage, ProviderUsageSummary, ProviderUsageTracker};
pub use routing::rate_limiter::{
    CircuitState, RateLimitConfig, RateLimiterRegistry, backoff_with_jitter,
};
pub use routing::router::{
    LlmCapacityInfo, LlmRouter, LlmRouterError, ProviderEntry, RequestContext, RouterRequest,
};
pub use streaming::collect_stream;
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

    /// Whether this provider supports streaming responses.
    fn supports_streaming(&self) -> bool {
        false
    }

    /// Streaming chat completion. Default falls back to non-streaming.
    async fn chat_streaming(&self, request: ChatRequest) -> Result<ChatStream, LlmError> {
        let response = self.chat(request).await?;
        Ok(Box::pin(futures_util::stream::iter(
            response_to_events(response).into_iter().map(Ok),
        )))
    }

    /// Streaming chat with specific key. Default delegates to `chat_streaming`.
    async fn chat_streaming_with_key(
        &self,
        _key: &str,
        request: ChatRequest,
    ) -> Result<ChatStream, LlmError> {
        self.chat_streaming(request).await
    }
}

/// Convert a complete ChatResponse into a StreamEvent sequence (for fallback).
pub fn response_to_events(response: ChatResponse) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    if let Some(ref thinking) = response.thinking {
        events.push(StreamEvent::ThinkingDelta {
            thinking: thinking.clone(),
        });
    }
    if !response.content.is_empty() {
        events.push(StreamEvent::TextDelta {
            text: response.content,
        });
    }
    for (i, tc) in response.tool_calls.iter().enumerate() {
        events.push(StreamEvent::ToolUseStart {
            index: i,
            id: tc.id.clone(),
            name: tc.name.clone(),
        });
        events.push(StreamEvent::InputJsonDelta {
            index: i,
            partial_json: tc.arguments.to_string(),
        });
    }
    events.push(StreamEvent::Usage(response.usage));
    events.push(StreamEvent::Done {
        finish_reason: response.finish_reason,
    });
    events
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

    #[test]
    fn test_tool_definition_backward_compat_deserialization() {
        // Old JSON without strict/input_examples → fields are None
        let json = r#"{"name":"search","description":"Search","parameters":{}}"#;
        let td: ToolDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(td.name, "search");
        assert!(td.strict.is_none());
        assert!(td.input_examples.is_none());
    }

    #[test]
    fn test_llm_error_stream_is_transient() {
        assert!(LlmError::Stream("broken pipe".to_string()).is_transient());
    }

    #[test]
    fn test_http_timeout_is_transient() {
        assert!(LlmError::Http("error sending request: operation timed out".into()).is_transient());
    }

    #[test]
    fn test_http_connection_reset_is_transient() {
        assert!(LlmError::Http("connection reset by peer".into()).is_transient());
    }

    #[test]
    fn test_http_broken_pipe_is_transient() {
        assert!(LlmError::Http("error sending request: broken pipe".into()).is_transient());
    }

    #[test]
    fn test_http_dns_error_not_transient() {
        assert!(!LlmError::Http("dns error: failed to lookup address".into()).is_transient());
    }

    #[test]
    fn test_http_tls_error_not_transient() {
        assert!(!LlmError::Http("error trying to connect: ssl connect error: certificate verify failed".into()).is_transient());
    }

    #[test]
    fn test_http_connection_refused_not_transient() {
        assert!(!LlmError::Http("tcp connect error: Connection refused (os error 61)".into()).is_transient());
    }

    #[test]
    fn test_http_connection_closed_is_transient() {
        assert!(LlmError::Http("connection closed before message completed".into()).is_transient());
    }

    #[test]
    fn test_http_incomplete_message_is_transient() {
        assert!(LlmError::Http("error reading response: incomplete message".into()).is_transient());
    }

    #[tokio::test]
    async fn test_response_to_events_roundtrip() {
        let original = ChatResponse {
            content: "Hello world".to_string(),
            tool_calls: vec![ToolCall {
                id: "tc1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"query": "rust"}),
            }],
            model: "test-model".to_string(),
            usage: Usage {
                input_tokens: 100,
                output_tokens: 50,
                ..Default::default()
            },
            finish_reason: FinishReason::ToolUse,
            thinking: Some("Let me think...".to_string()),
            parts: None,
        };

        let events = response_to_events(original.clone());
        let stream = Box::pin(futures_util::stream::iter(events.into_iter().map(Ok)));
        let collected = collect_stream(stream, "test-model".to_string())
            .await
            .unwrap();

        assert_eq!(collected.content, original.content);
        assert_eq!(collected.tool_calls.len(), original.tool_calls.len());
        assert_eq!(collected.tool_calls[0].name, "search");
        assert_eq!(collected.tool_calls[0].arguments["query"], "rust");
        assert_eq!(collected.thinking.as_deref(), Some("Let me think..."));
        assert_eq!(collected.usage.input_tokens, 100);
        assert_eq!(collected.usage.output_tokens, 50);
        assert_eq!(collected.finish_reason, FinishReason::ToolUse);
    }

    #[test]
    fn test_response_to_events_text_only() {
        let response = ChatResponse {
            content: "Hello".to_string(),
            tool_calls: vec![],
            model: "m".to_string(),
            usage: Usage::default(),
            finish_reason: FinishReason::Stop,
            thinking: None,
            parts: None,
        };
        let events = response_to_events(response);
        // TextDelta + Usage + Done = 3 events
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], StreamEvent::TextDelta { text } if text == "Hello"));
        assert!(matches!(&events[1], StreamEvent::Usage(_)));
        assert!(matches!(
            &events[2],
            StreamEvent::Done {
                finish_reason: FinishReason::Stop
            }
        ));
    }
}
