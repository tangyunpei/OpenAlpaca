use async_trait::async_trait;
use openalpaca_llm::LlmProvider;
use openalpaca_llm::error::LlmError;
use openalpaca_llm::types::*;
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
                LlmError::Http(format!("plugin {} provider/chat: {}", self.plugin_id, e))
            })?;

        openalpaca_llm::openai_compat::parse_response(
            request.model.as_deref().unwrap_or("unknown"),
            result,
        )
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
