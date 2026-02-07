use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: &str) -> Self {
        Self {
            role: Role::System,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: &str) -> Self {
        Self {
            role: Role::User,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: Role::Assistant,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant_with_tools(response: &super::ChatResponse) -> Self {
        Self {
            role: Role::Assistant,
            content: response.content.clone(),
            tool_calls: if response.tool_calls.is_empty() {
                None
            } else {
                Some(response.tool_calls.clone())
            },
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: &str, content: &str) -> Self {
        Self {
            role: Role::Tool,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDefinition>,
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub model: String,
    pub usage: Usage,
    pub finish_reason: FinishReason,
}

#[derive(Debug, Clone, Default)]
pub struct Usage {
    /// Total input tokens (includes cache tokens for Anthropic).
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// Anthropic: tokens used to create a new prompt cache entry (subset of input_tokens).
    pub cache_creation_input_tokens: u32,
    /// Anthropic: tokens served from an existing prompt cache (subset of input_tokens).
    pub cache_read_input_tokens: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FinishReason {
    Stop,
    ToolUse,
    MaxTokens,
    Error,
}
