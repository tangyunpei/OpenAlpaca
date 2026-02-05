pub mod config;
pub mod error;
pub mod providers;
pub mod types;

pub use config::{LlmConfig, build_provider};
pub use error::LlmError;
pub use types::*;

use async_trait::async_trait;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn supports_tools(&self) -> bool;
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError>;
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
