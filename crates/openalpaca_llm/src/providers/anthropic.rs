#![cfg(feature = "anthropic")]

use crate::error::LlmError;
use crate::types::*;
use crate::LlmProvider;
use async_trait::async_trait;

const DEFAULT_MODEL: &str = "claude-sonnet-4-5-20250929";
const DEFAULT_MAX_TOKENS: u32 = 4096;
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
    max_tokens: u32,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: Option<String>, max_tokens: Option<u32>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            max_tokens: max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        }
    }

    fn build_request_body(
        &self,
        request: &ChatRequest,
    ) -> serde_json::Value {
        let model = request
            .model
            .as_deref()
            .unwrap_or(&self.model);
        let max_tokens = request.max_tokens.unwrap_or(self.max_tokens);

        // Extract system message (Anthropic uses top-level system field)
        let mut system_text = String::new();
        let mut messages = Vec::new();

        for msg in &request.messages {
            match msg.role {
                Role::System => {
                    if !system_text.is_empty() {
                        system_text.push('\n');
                    }
                    system_text.push_str(&msg.content);
                }
                Role::User => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": msg.content,
                    }));
                }
                Role::Assistant => {
                    if let Some(ref tool_calls) = msg.tool_calls {
                        // Assistant message with tool use
                        let mut content = Vec::new();
                        if !msg.content.is_empty() {
                            content.push(serde_json::json!({
                                "type": "text",
                                "text": msg.content,
                            }));
                        }
                        for tc in tool_calls {
                            content.push(serde_json::json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": tc.arguments,
                            }));
                        }
                        messages.push(serde_json::json!({
                            "role": "assistant",
                            "content": content,
                        }));
                    } else {
                        messages.push(serde_json::json!({
                            "role": "assistant",
                            "content": msg.content,
                        }));
                    }
                }
                Role::Tool => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": msg.tool_call_id,
                            "content": msg.content,
                        }],
                    }));
                }
            }
        }

        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": messages,
        });

        if !system_text.is_empty() {
            body["system"] = serde_json::Value::String(system_text);
        }

        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        if !request.tools.is_empty() {
            let tools: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(tools);
        }

        body
    }

    fn parse_response(&self, body: serde_json::Value) -> Result<ChatResponse, LlmError> {
        let model = body["model"]
            .as_str()
            .unwrap_or(&self.model)
            .to_string();

        let usage = Usage {
            input_tokens: body["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: body["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32,
        };

        let stop_reason = body["stop_reason"].as_str().unwrap_or("end_turn");
        let finish_reason = match stop_reason {
            "end_turn" => FinishReason::Stop,
            "tool_use" => FinishReason::ToolUse,
            "max_tokens" => FinishReason::MaxTokens,
            _ => FinishReason::Stop,
        };

        let mut content = String::new();
        let mut tool_calls = Vec::new();

        if let Some(content_blocks) = body["content"].as_array() {
            for block in content_blocks {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(text) = block["text"].as_str() {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(text);
                        }
                    }
                    Some("tool_use") => {
                        tool_calls.push(ToolCall {
                            id: block["id"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            name: block["name"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            arguments: block["input"].clone(),
                        });
                    }
                    _ => {}
                }
            }
        }

        Ok(ChatResponse {
            content,
            tool_calls,
            model,
            usage,
            finish_reason,
        })
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let body = self.build_request_body(&request);

        let response = self
            .client
            .post(API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        let status = response.status().as_u16();

        if status == 429 {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(1000);
            return Err(LlmError::RateLimited {
                retry_after_ms: retry_after * 1000,
            });
        }

        let response_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| LlmError::Serialization(e.to_string()))?;

        if status >= 400 {
            let message = response_body["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error")
                .to_string();
            return Err(LlmError::Api { status, message });
        }

        self.parse_response(response_body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_serialization() {
        let provider = AnthropicProvider::new(
            "test-key".to_string(),
            None,
            None,
        );
        let request = ChatRequest {
            messages: vec![
                ChatMessage::system("You are helpful."),
                ChatMessage::user("Hello"),
            ],
            tools: vec![],
            model: None,
            temperature: Some(0.7),
            max_tokens: None,
        };

        let body = provider.build_request_body(&request);
        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(body["system"], "You are helpful.");
        let temp = body["temperature"].as_f64().unwrap();
        assert!((temp - 0.7).abs() < 0.01);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1); // system extracted, only user remains
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Hello");
    }

    #[test]
    fn test_response_parsing() {
        let provider = AnthropicProvider::new("test-key".to_string(), None, None);
        let response_json = serde_json::json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-5-20250929",
            "content": [
                {"type": "text", "text": "Hello! How can I help?"}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 25,
                "output_tokens": 10
            }
        });

        let response = provider.parse_response(response_json).unwrap();
        assert_eq!(response.content, "Hello! How can I help?");
        assert!(response.tool_calls.is_empty());
        assert_eq!(response.model, "claude-sonnet-4-5-20250929");
        assert_eq!(response.usage.input_tokens, 25);
        assert_eq!(response.usage.output_tokens, 10);
        assert_eq!(response.finish_reason, FinishReason::Stop);
    }

    #[test]
    fn test_tool_use_response() {
        let provider = AnthropicProvider::new("test-key".to_string(), None, None);
        let response_json = serde_json::json!({
            "id": "msg_456",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-5-20250929",
            "content": [
                {"type": "text", "text": "Let me search that."},
                {
                    "type": "tool_use",
                    "id": "toolu_01",
                    "name": "web_search",
                    "input": {"query": "Rust programming"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 50,
                "output_tokens": 30
            }
        });

        let response = provider.parse_response(response_json).unwrap();
        assert_eq!(response.content, "Let me search that.");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "toolu_01");
        assert_eq!(response.tool_calls[0].name, "web_search");
        assert_eq!(response.tool_calls[0].arguments["query"], "Rust programming");
        assert_eq!(response.finish_reason, FinishReason::ToolUse);
    }

    #[test]
    fn test_error_handling() {
        let provider = AnthropicProvider::new("test-key".to_string(), None, None);
        let error_json = serde_json::json!({
            "error": {
                "type": "invalid_request_error",
                "message": "Invalid API key"
            }
        });
        // parse_response works on success payloads; API errors are handled before parsing
        // Just verify parse_response doesn't panic on unexpected structure
        let result = provider.parse_response(error_json);
        assert!(result.is_ok()); // It parses but fields are empty/default
    }
}
