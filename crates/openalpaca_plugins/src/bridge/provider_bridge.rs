use async_trait::async_trait;
use openalpaca_llm::error::LlmError;
use openalpaca_llm::types::*;
use openalpaca_llm::LlmProvider;
use tracing::{debug, warn};

use crate::stdio_channel::StdioChannel;

/// Bridges a plugin process to the [`LlmProvider`] trait.
///
/// Converts [`ChatRequest`] to OpenAI-compatible JSON, sends a `provider/chat`
/// JSON-RPC call to the plugin, and parses the OpenAI-compatible response back
/// into a [`ChatResponse`].
pub struct PluginLlmProvider {
    plugin_id: String,
    provider_name: String,
    tool_support: bool,
    streaming_support: bool,
    channel: StdioChannel,
}

impl PluginLlmProvider {
    pub fn new(
        plugin_id: String,
        provider_name: String,
        tool_support: bool,
        streaming_support: bool,
        channel: StdioChannel,
    ) -> Self {
        Self {
            plugin_id,
            provider_name,
            tool_support,
            streaming_support,
            channel,
        }
    }
}

#[async_trait]
impl LlmProvider for PluginLlmProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn supports_tools(&self) -> bool {
        self.tool_support
    }

    fn supports_streaming(&self) -> bool {
        self.streaming_support
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let body = build_request_body(&request);

        debug!(
            plugin_id = %self.plugin_id,
            provider = %self.provider_name,
            model = ?request.model,
            "sending provider/chat to plugin"
        );

        let result = self
            .channel
            .call("provider/chat", body)
            .await
            .map_err(|e| {
                warn!(
                    plugin_id = %self.plugin_id,
                    error = %e,
                    "plugin provider/chat failed"
                );
                LlmError::Http(format!(
                    "plugin {} provider/chat: {}",
                    self.plugin_id, e
                ))
            })?;

        parse_response(&request, result)
    }
}

// ── Request serialization ──────────────────────────────────────────

/// Serialize a [`ChatRequest`] into OpenAI-compatible chat completion JSON.
fn build_request_body(request: &ChatRequest) -> serde_json::Value {
    let messages: Vec<serde_json::Value> = request
        .messages
        .iter()
        .map(|msg| {
            let role = match msg.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };

            let mut obj = serde_json::json!({
                "role": role,
                "content": msg.content,
            });

            if let Some(ref tool_calls) = msg.tool_calls {
                let tcs: Vec<serde_json::Value> = tool_calls
                    .iter()
                    .map(|tc| {
                        serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": tc.arguments.to_string(),
                            }
                        })
                    })
                    .collect();
                obj["tool_calls"] = serde_json::Value::Array(tcs);
            }

            if let Some(ref tool_call_id) = msg.tool_call_id {
                obj["tool_call_id"] = serde_json::Value::String(tool_call_id.clone());
            }

            obj
        })
        .collect();

    let mut body = serde_json::json!({
        "messages": messages,
    });

    if let Some(ref model) = request.model {
        body["model"] = serde_json::Value::String(model.clone());
    }

    if let Some(temp) = request.temperature {
        body["temperature"] = serde_json::json!(temp);
    }

    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = serde_json::json!(max_tokens);
    }

    if !request.tools.is_empty() {
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|t| {
                let mut function = serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                });
                if let Some(true) = t.strict {
                    function["strict"] = serde_json::json!(true);
                }
                serde_json::json!({
                    "type": "function",
                    "function": function,
                })
            })
            .collect();
        body["tools"] = serde_json::Value::Array(tools);

        if let Some(ref choice) = request.tool_choice {
            body["tool_choice"] = match choice {
                ToolChoice::Auto => serde_json::json!("auto"),
                ToolChoice::Any => serde_json::json!("required"),
                ToolChoice::Tool(name) => serde_json::json!({
                    "type": "function",
                    "function": {"name": name}
                }),
            };
        }
    }

    body
}

// ── Response parsing ───────────────────────────────────────────────

/// Parse an OpenAI-compatible chat completion response into [`ChatResponse`].
fn parse_response(
    request: &ChatRequest,
    body: serde_json::Value,
) -> Result<ChatResponse, LlmError> {
    let model = body["model"]
        .as_str()
        .or(request.model.as_deref())
        .unwrap_or("unknown")
        .to_string();

    let usage = Usage {
        input_tokens: body["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32,
        output_tokens: body["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32,
        cache_read_input_tokens: body["usage"]["prompt_tokens_details"]["cached_tokens"]
            .as_u64()
            .unwrap_or(0) as u32,
        ..Default::default()
    };

    let choice = &body["choices"][0];
    let message = &choice["message"];

    let content = message["content"].as_str().unwrap_or("").to_string();

    let finish_reason_str = choice["finish_reason"].as_str().unwrap_or("stop");
    let finish_reason = match finish_reason_str {
        "stop" => FinishReason::Stop,
        "tool_calls" => FinishReason::ToolUse,
        "length" => FinishReason::MaxTokens,
        _ => FinishReason::Stop,
    };

    let mut tool_calls = Vec::new();
    if let Some(tcs) = message["tool_calls"].as_array() {
        for tc in tcs {
            let id = tc["id"].as_str().unwrap_or_default().to_string();
            let name = tc["function"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
            let arguments: serde_json::Value = serde_json::from_str(args_str)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
    }

    let parts = if content.is_empty() {
        None
    } else {
        Some(vec![ContentPart::Text {
            text: content.clone(),
        }])
    };

    Ok(ChatResponse {
        content,
        tool_calls,
        model,
        usage,
        finish_reason,
        thinking: message["reasoning_content"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        parts,
    })
}
