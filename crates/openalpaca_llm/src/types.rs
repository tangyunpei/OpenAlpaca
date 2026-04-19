use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

// ── Multimodal content types ────────────────────────────────────────

/// A single content part within a multimodal message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    Image {
        source: ImageSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    Audio {
        #[serde(
            serialize_with = "serialize_arc_string",
            deserialize_with = "deserialize_arc_string"
        )]
        data: Arc<String>,
        format: String,
    },
    Document {
        file_id: String,
        filename: String,
        mime_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        extracted_text: Option<String>,
    },
    FileRef {
        file_id: String,
        filename: String,
        mime_type: String,
    },
}

/// Source for an image content part.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 {
        media_type: String,
        #[serde(
            serialize_with = "serialize_arc_string",
            deserialize_with = "deserialize_arc_string"
        )]
        data: Arc<String>,
    },
    Url {
        url: String,
    },
    /// Resolved at provider serialization boundary — loads from disk and base64-encodes lazily.
    FileAsset {
        file_id: String,
        media_type: String,
    },
}

fn serialize_arc_string<S>(val: &Arc<String>, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    s.serialize_str(val)
}

fn deserialize_arc_string<'de, D>(d: D) -> Result<Arc<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    Ok(Arc::new(s))
}

// ── ChatMessage ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Multimodal content parts. When present, providers serialize these
    /// instead of the plain `content` string. When `None`, the message is
    /// text-only and `content` is used directly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<ContentPart>>,
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
            parts: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: &str) -> Self {
        Self {
            role: Role::User,
            content: content.to_string(),
            parts: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Create a user message with multimodal content parts.
    pub fn user_with_parts(parts: Vec<ContentPart>) -> Self {
        // Extract text content for the `content` field (used for logging, search, intent)
        let text: String = parts
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        Self {
            role: Role::User,
            content: text,
            parts: Some(parts),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: Role::Assistant,
            content: content.to_string(),
            parts: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant_with_tools(response: &super::ChatResponse) -> Self {
        Self {
            role: Role::Assistant,
            content: response.content.clone(),
            parts: response.parts.clone(),
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
            parts: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
        }
    }

    /// Create a tool-result message with multimodal content parts.
    pub fn tool_result_with_parts(tool_call_id: &str, content: &str, parts: Vec<ContentPart>) -> Self {
        Self {
            role: Role::Tool,
            content: content.to_string(),
            parts: Some(parts),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
        }
    }

    /// Returns the effective content parts for this message.
    /// If `parts` is `Some`, returns those; otherwise wraps `content` in a single `Text` part.
    pub fn effective_parts(&self) -> Vec<ContentPart> {
        match &self.parts {
            Some(parts) if !parts.is_empty() => parts.clone(),
            _ => vec![ContentPart::Text {
                text: self.content.clone(),
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    /// When `true`, the model guarantees valid JSON matching the schema exactly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    /// Concrete examples of valid tool inputs (Anthropic `input_examples`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_examples: Option<Vec<serde_json::Value>>,
}

/// Controls which tool the model should use.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolChoice {
    /// Model decides whether to use a tool (default API behaviour).
    Auto,
    /// Model must use *some* tool (any).
    Any,
    /// Model must call this specific tool.
    Tool(String),
}

/// Cache control hint for Anthropic prompt caching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub type_: String,
    /// Optional TTL in seconds. Default: provider-determined (Anthropic = 5 min).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u32>,
}

impl CacheControl {
    /// Ephemeral cache (provider default TTL, typically 5 min).
    pub fn ephemeral() -> Self {
        Self {
            type_: "ephemeral".to_string(),
            ttl: None,
        }
    }

    /// Ephemeral cache with 1-hour TTL.
    // Forward-looking: Anthropic does not yet accept TTL hints.
    pub fn ephemeral_1h() -> Self {
        Self {
            type_: "ephemeral".to_string(),
            ttl: Some(3600),
        }
    }
}

/// Configuration for Claude's extended thinking capability.
#[derive(Debug, Clone)]
pub enum ThinkingConfig {
    /// Thinking enabled with a fixed token budget (minimum 1024).
    Enabled { budget_tokens: u32 },
    /// Claude adaptively decides whether and how much to think.
    Adaptive,
    /// Thinking explicitly disabled.
    Disabled,
}

impl ThinkingConfig {
    /// Create an Enabled config, clamping budget_tokens to at least 1024.
    pub fn enabled(budget_tokens: u32) -> Self {
        Self::Enabled {
            budget_tokens: budget_tokens.max(1024),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub messages: Arc<Vec<ChatMessage>>,
    pub tools: Arc<Vec<ToolDefinition>>,
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub tool_choice: Option<ToolChoice>,
    /// Enable Anthropic prompt caching (non-Anthropic providers ignore this).
    pub enable_caching: bool,
    /// Extended thinking config (Anthropic only). Non-Anthropic providers ignore this.
    pub thinking: Option<ThinkingConfig>,
    /// Context management configuration (Anthropic only). Other providers ignore this.
    pub context_management: Option<crate::context_management::ContextManagement>,
    /// Ephemeral system notice (e.g., budget pressure) injected into this
    /// request only. Never persisted to conversation history. Providers
    /// place it per their convention (spec P0).
    pub ephemeral_system_notice: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub model: String,
    pub usage: Usage,
    pub finish_reason: FinishReason,
    /// Extended thinking output (Anthropic only).
    pub thinking: Option<String>,
    /// Multimodal content parts from the response. When present,
    /// `assistant_with_tools()` propagates these to the ChatMessage.
    pub parts: Option<Vec<ContentPart>>,
}

#[derive(Debug, Clone, Default)]
pub struct Usage {
    /// Total input tokens (includes cache tokens for Anthropic/OpenAI).
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// Tokens used to create a new prompt cache entry (Anthropic only; subset of input_tokens).
    pub cache_creation_input_tokens: u32,
    /// Tokens served from an existing prompt cache (Anthropic + OpenAI; subset of input_tokens).
    pub cache_read_input_tokens: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FinishReason {
    Stop,
    ToolUse,
    MaxTokens,
    Error,
}

/// A single chunk from a streaming response.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Incremental text content.
    TextDelta { text: String },
    /// Incremental thinking content (Anthropic extended thinking).
    ThinkingDelta { thinking: String },
    /// Start of a tool use content block.
    ToolUseStart {
        index: usize,
        id: String,
        name: String,
    },
    /// Incremental JSON for tool input arguments.
    InputJsonDelta { index: usize, partial_json: String },
    /// Final usage statistics.
    Usage(Usage),
    /// Stream completed.
    Done { finish_reason: FinishReason },
    /// Stream error.
    Error { message: String },
}

/// A boxed stream of chat events.
pub type ChatStream = Pin<
    Box<dyn futures_util::Stream<Item = Result<StreamEvent, crate::error::LlmError>> + Send>,
>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_with_tools_propagates_parts() {
        let response = ChatResponse {
            content: "Here's the image".to_string(),
            parts: Some(vec![
                ContentPart::Text { text: "Here's the image".to_string() },
                ContentPart::Image {
                    source: ImageSource::Base64 {
                        media_type: "image/png".to_string(),
                        data: Arc::new("base64data".to_string()),
                    },
                    detail: None,
                },
            ]),
            tool_calls: vec![],
            model: "test".to_string(),
            usage: Usage::default(),
            finish_reason: FinishReason::Stop,
            thinking: None,
        };
        let msg = ChatMessage::assistant_with_tools(&response);
        assert_eq!(msg.parts.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn tool_result_with_parts_sets_fields() {
        let parts = vec![ContentPart::Text { text: "result".to_string() }];
        let msg = ChatMessage::tool_result_with_parts("call_1", "summary", parts);
        assert_eq!(msg.role, Role::Tool);
        assert_eq!(msg.tool_call_id.as_deref(), Some("call_1"));
        assert!(msg.parts.is_some());
    }

    #[test]
    fn effective_parts_fallback_to_content() {
        let msg = ChatMessage::assistant("plain text");
        let parts = msg.effective_parts();
        assert_eq!(parts.len(), 1);
        assert!(matches!(&parts[0], ContentPart::Text { text } if text == "plain text"));
    }

    #[test]
    fn test_cache_control_serialization() {
        // ephemeral() — no TTL
        let cc = CacheControl::ephemeral();
        let v = serde_json::to_value(&cc).unwrap();
        assert_eq!(v["type"], "ephemeral");
        assert!(v.get("ttl").is_none());

        // ephemeral_1h() — TTL = 3600
        let cc_1h = CacheControl::ephemeral_1h();
        let v_1h = serde_json::to_value(&cc_1h).unwrap();
        assert_eq!(v_1h["type"], "ephemeral");
        assert_eq!(v_1h["ttl"], 3600);
    }
}
