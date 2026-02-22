use crate::LlmProvider;
use crate::error::LlmError;
use crate::types::*;
use async_trait::async_trait;

const DEFAULT_MODEL: &str = "gpt-4o";
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_MAX_TOKENS: u32 = 4096;

pub struct OpenAiProvider {
    client: reqwest::Client,
    api_key: Option<String>,
    model: String,
    pub(crate) base_url: String,
    max_tokens: u32,
}

impl OpenAiProvider {
    pub fn new(
        api_key: String,
        model: Option<String>,
        base_url: Option<String>,
        max_tokens: Option<u32>,
    ) -> Self {
        Self::with_client(reqwest::Client::new(), api_key, model, base_url, max_tokens)
    }

    /// Create with a shared `reqwest::Client` (for connection pool reuse).
    pub fn with_client(
        client: reqwest::Client,
        api_key: String,
        model: Option<String>,
        base_url: Option<String>,
        max_tokens: Option<u32>,
    ) -> Self {
        Self {
            client,
            api_key: Some(api_key),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            max_tokens: max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        }
    }

    /// Create a provider without auth (for OpenAI-compatible APIs like Ollama).
    pub fn new_without_auth(model: String, base_url: String) -> Self {
        Self::new_without_auth_with_client(reqwest::Client::new(), model, base_url)
    }

    /// Create a provider without auth, using a shared client.
    pub fn new_without_auth_with_client(
        client: reqwest::Client,
        model: String,
        base_url: String,
    ) -> Self {
        Self {
            client,
            api_key: None,
            model,
            base_url,
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    pub(crate) fn build_request_body(&self, request: &ChatRequest) -> serde_json::Value {
        let model = request.model.as_deref().unwrap_or(&self.model);
        let max_tokens = request.max_tokens.unwrap_or(self.max_tokens);

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
            "model": model,
            "max_tokens": max_tokens,
            "messages": messages,
        });

        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        if !request.tools.is_empty() {
            let tools: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(tools);
        }

        body
    }

    pub(crate) fn parse_response(&self, body: serde_json::Value) -> Result<ChatResponse, LlmError> {
        let model = body["model"].as_str().unwrap_or(&self.model).to_string();

        let usage = Usage {
            input_tokens: body["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: body["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32,
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
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        match self.api_key {
            Some(ref key) => self.chat_with_key(key, request).await,
            None => self.chat_with_key("", request).await,
        }
    }

    async fn list_models_with_key(&self, key: &str) -> Result<Vec<String>, LlmError> {
        let url = format!("{}/models", self.base_url);
        let mut req_builder = self.client.get(&url);

        if !key.is_empty() {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", key));
        }

        let response = req_builder
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        if !response.status().is_success() {
            return Ok(vec![]);
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| LlmError::Serialization(e.to_string()))?;

        let models = body["data"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        Ok(models)
    }

    async fn chat_with_key(
        &self,
        key: &str,
        request: ChatRequest,
    ) -> Result<ChatResponse, LlmError> {
        let body = self.build_request_body(&request);
        let url = format!("{}/chat/completions", self.base_url);

        let mut req_builder = self
            .client
            .post(&url)
            .header("content-type", "application/json");

        if !key.is_empty() {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", key));
        } else if let Some(ref api_key) = self.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = req_builder
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
                .unwrap_or(1);
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
    fn test_request_format() {
        let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);
        let request = ChatRequest {
            messages: vec![
                ChatMessage::system("You are helpful."),
                ChatMessage::user("Hello"),
            ],
            tools: vec![],
            model: None,
            temperature: None,
            max_tokens: None,
        };

        let body = provider.build_request_body(&request);
        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2); // system + user (inline, unlike Anthropic)
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are helpful.");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Hello");
    }

    #[test]
    fn test_response_parsing() {
        let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);
        let response_json = serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "model": "gpt-4o-2024-05-13",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello! How can I help you?"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 8,
                "total_tokens": 28
            }
        });

        let response = provider.parse_response(response_json).unwrap();
        assert_eq!(response.content, "Hello! How can I help you?");
        assert!(response.tool_calls.is_empty());
        assert_eq!(response.model, "gpt-4o-2024-05-13");
        assert_eq!(response.usage.input_tokens, 20);
        assert_eq!(response.usage.output_tokens, 8);
        assert_eq!(response.finish_reason, FinishReason::Stop);
    }

    #[test]
    fn test_tool_calls_response() {
        let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);
        let response_json = serde_json::json!({
            "id": "chatcmpl-456",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"location\": \"Paris\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 20,
                "total_tokens": 70
            }
        });

        let response = provider.parse_response(response_json).unwrap();
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "call_abc");
        assert_eq!(response.tool_calls[0].name, "get_weather");
        assert_eq!(response.tool_calls[0].arguments["location"], "Paris");
        assert_eq!(response.finish_reason, FinishReason::ToolUse);
    }

    #[test]
    fn test_base_url_custom() {
        let provider = OpenAiProvider::new(
            "key".to_string(),
            Some("custom-model".to_string()),
            Some("http://localhost:8080/v1".to_string()),
            None,
        );
        assert_eq!(provider.base_url, "http://localhost:8080/v1");
        assert_eq!(provider.model, "custom-model");
    }
}
