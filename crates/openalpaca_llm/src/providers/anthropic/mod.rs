use crate::LlmProvider;
use crate::error::LlmError;
use crate::types::*;
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
        Self::with_client(reqwest::Client::new(), api_key, model, max_tokens)
    }

    /// Create with a shared `reqwest::Client` (for connection pool reuse).
    pub fn with_client(
        client: reqwest::Client,
        api_key: String,
        model: Option<String>,
        max_tokens: Option<u32>,
    ) -> Self {
        Self {
            client,
            api_key,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            max_tokens: max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        }
    }

    /// Build Anthropic content blocks from a ChatMessage.
    ///
    /// If the message has multimodal `parts`, builds an array of content blocks
    /// in Anthropic's format. If parts is None, returns a plain string value.
    fn build_message_content(msg: &ChatMessage) -> serde_json::Value {
        let parts = match &msg.parts {
            Some(parts) if !parts.is_empty() => parts,
            _ => return serde_json::Value::String(msg.content.clone()),
        };

        let blocks: Vec<serde_json::Value> = parts
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => {
                    if text.trim().is_empty() {
                        None
                    } else {
                        Some(serde_json::json!({ "type": "text", "text": text }))
                    }
                }
                ContentPart::Image { source, .. } => match source {
                    ImageSource::Base64 { media_type, data } => Some(serde_json::json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": media_type,
                            "data": data.as_str(),
                        }
                    })),
                    ImageSource::Url { url } => Some(serde_json::json!({
                        "type": "image",
                        "source": {
                            "type": "url",
                            "url": url,
                        }
                    })),
                    ImageSource::FileAsset { file_id, media_type } => {
                        // FileAsset should be resolved to Base64 before reaching the provider.
                        // If it hasn't been, emit a placeholder text block.
                        Some(serde_json::json!({
                            "type": "text",
                            "text": format!("[image file_id={} not resolved — media_type={}]", file_id, media_type),
                        }))
                    }
                },
                ContentPart::Audio { .. } => {
                    // Anthropic does not support audio input
                    Some(serde_json::json!({
                        "type": "text",
                        "text": "[audio content — not supported by this model]",
                    }))
                }
                ContentPart::Document {
                    filename,
                    mime_type,
                    extracted_text,
                    ..
                } => {
                    // Anthropic supports PDF via beta; fall back to extracted text for now
                    if let Some(text) = extracted_text {
                        Some(serde_json::json!({
                            "type": "text",
                            "text": format!("[Document: {} ({})]\n{}", filename, mime_type, text),
                        }))
                    } else {
                        Some(serde_json::json!({
                            "type": "text",
                            "text": format!("[Document: {} ({}) — no extracted text available]", filename, mime_type),
                        }))
                    }
                }
                ContentPart::FileRef {
                    file_id,
                    filename,
                    mime_type,
                } => Some(serde_json::json!({
                    "type": "text",
                    "text": format!("[File reference: {} ({}) id={}]", filename, mime_type, file_id),
                })),
            })
            .collect();

        if blocks.is_empty() {
            if !msg.content.trim().is_empty() {
                return serde_json::Value::String(msg.content.clone());
            }
            return serde_json::Value::Array(vec![serde_json::json!({
                "type": "text",
                "text": "[empty message]",
            })]);
        }

        serde_json::Value::Array(blocks)
    }

    fn build_request_body(&self, request: &ChatRequest) -> serde_json::Value {
        let model = request.model.as_deref().unwrap_or(&self.model);
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
                        "content": Self::build_message_content(msg),
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
        let model = body["model"].as_str().unwrap_or(&self.model).to_string();

        let base_input = body["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32;
        let cache_creation = body["usage"]["cache_creation_input_tokens"]
            .as_u64()
            .unwrap_or(0) as u32;
        let cache_read = body["usage"]["cache_read_input_tokens"]
            .as_u64()
            .unwrap_or(0) as u32;

        let usage = Usage {
            input_tokens: base_input + cache_creation + cache_read,
            output_tokens: body["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32,
            cache_creation_input_tokens: cache_creation,
            cache_read_input_tokens: cache_read,
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
                            id: block["id"].as_str().unwrap_or_default().to_string(),
                            name: block["name"].as_str().unwrap_or_default().to_string(),
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
        self.chat_with_key(&self.api_key, request).await
    }

    async fn list_models_with_key(&self, key: &str) -> Result<Vec<String>, LlmError> {
        let response = self
            .client
            .get("https://api.anthropic.com/v1/models")
            .header("x-api-key", key)
            .header("anthropic-version", API_VERSION)
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

        let response = self
            .client
            .post(API_URL)
            .header("x-api-key", key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        let status = response.status().as_u16();

        if status == 429 || status == 529 {
            let default_retry = if status == 529 { 30 } else { 1 };
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(default_retry);
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
mod tests;
