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
    /// A tool result whose call failed (N-TOOL-4, Z1-13): the call's own
    /// `Err`, never read off the text. The Anthropic provider sends it as the
    /// `tool_result` block's `is_error`; the OpenAI-compatible wire has no
    /// such field, so there the text's `[tool_error]` marker says it. `false`
    /// on every other message, and absent from the serialized form.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    /// The assistant turn this message rebuilds, as the provider returned
    /// it, when that turn carried thinking (C7a): the Anthropic arm sends it
    /// back block for block, and the OpenAI arm's Responses wire item for
    /// item (C7b), so the model keeps its reasoning across a tool loop.
    /// Lives only within one loop — never serialized, so no session log,
    /// database row or history line holds thinking.
    #[serde(skip)]
    pub echo: Option<EchoedTurn>,
}

/// An assistant turn's content blocks exactly as the provider returned them,
/// in their order, kept only for a turn that carried thinking (C7a).
///
/// Anthropic's pages ask that a tool loop pass each assistant turn back
/// "complete and unmodified": its `thinking` blocks with their `signature`s
/// (empty `thinking` text included), its `redacted_thinking` blocks, its
/// text and its `tool_use` blocks, in the order received — "the API rejects
/// edited, reordered, or partially dropped thinking blocks with a 400 error"
/// (platform.claude.com/docs/en/models/opus-5-5/migration-guide and
/// .../build-with-claude/thinking, read 2026-11-12). A whole response keeps
/// the API's own blocks; a streamed one is put back together by block index.
///
/// OpenAI's Responses API asks the same of a reasoning model's tool loop:
/// "any reasoning items returned in model responses with tool calls must
/// also be passed back with tool call outputs", every output item "in their
/// original order" (developers.openai.com/api/docs/guides/function-calling
/// and .../guides/reasoning, read 2026-11-12; C7b). Such a turn is kept as
/// its output items, tagged [`EchoWire::OpenAiResponses`], and each arm sends
/// back only a turn of its own wire.
#[derive(Debug, Clone, PartialEq)]
pub struct EchoedTurn {
    wire: EchoWire,
    items: Arc<Vec<serde_json::Value>>,
}

/// Whose wire an [`EchoedTurn`]'s items are written for (C7b): a turn goes
/// back only to the arm that returned it, so a request that falls back to
/// another provider mid-loop rebuilds the turn from its text and tool calls
/// instead of sending one API's items to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoWire {
    /// Anthropic's Messages API: the turn's content blocks.
    AnthropicMessages,
    /// OpenAI's Responses API: the response's output items.
    OpenAiResponses,
}

impl EchoedTurn {
    /// The turn's blocks, when at least one is a `thinking` or
    /// `redacted_thinking` block and each can go back: every `thinking`
    /// block has its `signature`, every `redacted_thinking` block its
    /// `data` ("sent back empty", a signature is refused, the thinking
    /// troubleshooting page). `None` otherwise — a turn without thinking is
    /// rebuilt exactly as before, and one that cannot go back whole goes
    /// back with none of its thinking, never some.
    pub fn from_blocks(blocks: Vec<serde_json::Value>) -> Option<Self> {
        let complete = blocks.iter().all(|b| match b["type"].as_str() {
            Some("thinking") => {
                b["thinking"].is_string() && b["signature"].as_str().is_some_and(|s| !s.is_empty())
            }
            Some("redacted_thinking") => b["data"].as_str().is_some_and(|d| !d.is_empty()),
            _ => true,
        });
        (complete && blocks.iter().any(is_thinking_block)).then(|| Self {
            wire: EchoWire::AnthropicMessages,
            items: Arc::new(blocks),
        })
    }

    /// A Responses API turn's output items (C7b), when at least one is a
    /// `reasoning` item and each can go back: every item is a `reasoning`,
    /// `message` or `function_call` item, and every `reasoning` item carries
    /// its `encrypted_content`. The arm asks the API to store nothing
    /// (`store: false`), and in that mode a reasoning item goes back by its
    /// encrypted content, "populated by default" (the Responses API
    /// reference, read 2026-11-12). `None` otherwise: the turn is rebuilt
    /// from its text and tool calls, as before.
    pub fn from_response_items(items: Vec<serde_json::Value>) -> Option<Self> {
        let complete = items.iter().all(|item| match item["type"].as_str() {
            Some("reasoning") => item["encrypted_content"]
                .as_str()
                .is_some_and(|e| !e.is_empty()),
            Some("message" | "function_call") => true,
            _ => false,
        });
        (complete && items.iter().any(|item| item["type"] == "reasoning")).then(|| Self {
            wire: EchoWire::OpenAiResponses,
            items: Arc::new(items),
        })
    }

    /// Whose wire the items are written for.
    pub fn wire(&self) -> EchoWire {
        self.wire
    }

    /// The blocks or items, in the order the provider returned them.
    pub fn blocks(&self) -> &[serde_json::Value] {
        &self.items
    }

    /// The ids of the turn's `tool_use` blocks, in order.
    pub fn tool_use_ids(&self) -> Vec<&str> {
        self.items
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .filter_map(|b| b["id"].as_str())
            .collect()
    }

    /// The `call_id`s of the turn's `function_call` items, in order (C7b):
    /// what a tool result names on the Responses API, not the item's `id`.
    pub fn function_call_ids(&self) -> Vec<&str> {
        self.items
            .iter()
            .filter(|b| b["type"] == "function_call")
            .filter_map(|b| b["call_id"].as_str())
            .collect()
    }
}

/// A `thinking` or `redacted_thinking` content block.
pub fn is_thinking_block(block: &serde_json::Value) -> bool {
    matches!(
        block["type"].as_str(),
        Some("thinking" | "redacted_thinking")
    )
}

impl ChatMessage {
    pub fn system(content: &str) -> Self {
        Self {
            role: Role::System,
            content: content.to_string(),
            parts: None,
            tool_calls: None,
            tool_call_id: None,
            is_error: false,
            echo: None,
        }
    }

    pub fn user(content: &str) -> Self {
        Self {
            role: Role::User,
            content: content.to_string(),
            parts: None,
            tool_calls: None,
            tool_call_id: None,
            is_error: false,
            echo: None,
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
            is_error: false,
            echo: None,
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: Role::Assistant,
            content: content.to_string(),
            parts: None,
            tool_calls: None,
            tool_call_id: None,
            is_error: false,
            echo: None,
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
            is_error: false,
            echo: response.echo.clone(),
        }
    }

    /// The text a response answered with, as the next request's assistant
    /// turn: the `MaxTokens` continuation, the steering drain at the
    /// completion guard and the answer guard's corrective round (C7a).
    ///
    /// Like [`Self::assistant`], text only, with no parts, so a model without
    /// thinking is sent what it always was; a turn that carried thinking and
    /// called no tool goes back whole, all its thinking with it. A turn that
    /// called a tool is not echoed here, since the text alone leaves its
    /// `tool_use` blocks out.
    pub fn assistant_answer(response: &super::ChatResponse) -> Self {
        Self {
            echo: if response.tool_calls.is_empty() {
                response.echo.clone()
            } else {
                None
            },
            ..Self::assistant(&response.content)
        }
    }

    /// Leave this message's thinking out of every later request (C7a): what
    /// it was produced after has changed, so the API's prefix check would
    /// fail it, and once out it stays out. Returns whether there was any.
    pub fn forget_thinking(&mut self) -> bool {
        self.echo.take().is_some()
    }

    pub fn tool_result(tool_call_id: &str, content: &str) -> Self {
        Self {
            role: Role::Tool,
            content: content.to_string(),
            parts: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
            is_error: false,
            echo: None,
        }
    }

    /// The result of a tool call that failed (N-TOOL-4): a tool result with
    /// [`is_error`](Self::is_error) set. `content` is the text every wire
    /// shows — marked `[tool_error]` by the loop, since the OpenAI-compatible
    /// wire has no field to carry the flag.
    pub fn tool_error(tool_call_id: &str, content: &str) -> Self {
        Self {
            is_error: true,
            ..Self::tool_result(tool_call_id, content)
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
            is_error: false,
            echo: None,
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
    /// Asks for OpenAI's strict mode (arguments that match the schema
    /// exactly). Only the OpenAI-compatible providers send it, and only when
    /// the schema is one strict mode accepts
    /// ([`crate::providers::openai_tools::strict_mode_compatible`]); the
    /// Anthropic provider never serializes it.
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
    /// How much the caller wants this model to reason — the one request field
    /// each provider maps to its own wire (M2). Anthropic reads it as extended
    /// thinking; the OpenAI-compatible providers map it to `reasoning_effort`,
    /// and `Disabled` reaches Ollama as `"none"` so a small-budget utility call
    /// is not spent on thought before the answer. A provider with no such
    /// control ignores it.
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
    /// The turn's content blocks as the provider returned them, when it
    /// carried thinking (C7a), or its output items, when a Responses API
    /// turn carried reasoning (C7b); `None` from every other provider and
    /// wire, and for a turn without thinking.
    /// [`ChatMessage::assistant_with_tools`] and
    /// [`ChatMessage::assistant_answer`] carry it into the loop.
    pub echo: Option<EchoedTurn>,
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
    /// The whole turn's content blocks, in order, once the stream has
    /// delivered them all — sent only for a turn that carried thinking
    /// (C7a; Anthropic's `thinking`, `signature_delta` and
    /// `redacted_thinking`; C7b, a Responses API turn's output items with
    /// a reasoning item), so the collected response can be echoed.
    Turn(EchoedTurn),
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
            echo: None,
        };
        let msg = ChatMessage::assistant_with_tools(&response);
        assert_eq!(msg.parts.as_ref().unwrap().len(), 2);
    }

    /// `assistant_answer` (C7a) carries a thinking turn that called no tool
    /// whole, and leaves out the thinking of one that called a tool: the
    /// answer's text alone would drop its `tool_use` blocks, so its thinking
    /// goes back none, never some.
    #[test]
    fn assistant_answer_echoes_only_a_turn_that_called_no_tool() {
        let blocks = |with_tool: bool| {
            let mut blocks = vec![
                serde_json::json!({"type": "thinking", "thinking": "", "signature": "sig-A"}),
                serde_json::json!({"type": "text", "text": "Answer."}),
            ];
            if with_tool {
                blocks.push(serde_json::json!(
                    {"type": "tool_use", "id": "toolu_1", "name": "search", "input": {}}
                ));
            }
            blocks
        };
        let response = |with_tool: bool| ChatResponse {
            content: "Answer.".to_string(),
            parts: None,
            tool_calls: if with_tool {
                vec![ToolCall {
                    id: "toolu_1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({}),
                }]
            } else {
                vec![]
            },
            model: "claude-opus-5-5".to_string(),
            usage: Usage::default(),
            finish_reason: FinishReason::MaxTokens,
            thinking: None,
            echo: EchoedTurn::from_blocks(blocks(with_tool)),
        };

        let answered = ChatMessage::assistant_answer(&response(false));
        assert_eq!(answered.content, "Answer.");
        assert_eq!(answered.echo, response(false).echo);
        assert!(answered.echo.is_some());

        let called = ChatMessage::assistant_answer(&response(true));
        assert_eq!(called.content, "Answer.");
        assert!(called.tool_calls.is_none());
        assert!(
            called.echo.is_none(),
            "a turn that called a tool is not echoed"
        );
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
