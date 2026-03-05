use crate::LlmProvider;
use crate::error::LlmError;
use crate::types::*;
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::HeaderMap;

const DEFAULT_MODEL: &str = "claude-sonnet-4-5-20250929";
const DEFAULT_MAX_TOKENS: u32 = 4096;
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

fn parse_retry_after_ms(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<f64>().ok())
        .map(|secs| ((secs.max(0.0)) * 1000.0).round() as u64)
        .filter(|ms| *ms > 0)
}

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

        for msg in request.messages.iter() {
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
            if request.enable_caching {
                body["system"] = serde_json::json!([{
                    "type": "text",
                    "text": system_text,
                    "cache_control": CacheControl::ephemeral()
                }]);
            } else {
                body["system"] = serde_json::Value::String(system_text);
            }
        }

        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        if !request.tools.is_empty() {
            let tools: Vec<serde_json::Value> = request
                .tools
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let mut tool = serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    });
                    if let Some(true) = t.strict {
                        tool["strict"] = serde_json::json!(true);
                    }
                    if let Some(ref examples) = t.input_examples {
                        tool["input_examples"] = serde_json::json!(examples);
                    }
                    // Cache breakpoint on the last tool
                    if request.enable_caching && i == request.tools.len() - 1 {
                        tool["cache_control"] = serde_json::json!(CacheControl::ephemeral());
                    }
                    tool
                })
                .collect();
            body["tools"] = serde_json::Value::Array(tools);

            if let Some(ref choice) = request.tool_choice {
                body["tool_choice"] = match choice {
                    ToolChoice::Auto => serde_json::json!({"type": "auto"}),
                    ToolChoice::Any => serde_json::json!({"type": "any"}),
                    ToolChoice::Tool(name) => serde_json::json!({"type": "tool", "name": name}),
                };
            }
        }

        // Extended thinking
        if let Some(ref thinking) = request.thinking {
            match thinking {
                ThinkingConfig::Enabled { budget_tokens } => {
                    if *budget_tokens < 1024 {
                        tracing::warn!(
                            budget_tokens,
                            "budget_tokens < 1024 may produce poor thinking results; Anthropic minimum is 1024"
                        );
                    }
                    body["thinking"] = serde_json::json!({
                        "type": "enabled",
                        "budget_tokens": budget_tokens,
                    });
                    // Anthropic requires temperature=1.0 (or unset) for thinking
                    body.as_object_mut().unwrap().remove("temperature");
                }
                ThinkingConfig::Adaptive => {
                    body["thinking"] = serde_json::json!({ "type": "adaptive" });
                    body.as_object_mut().unwrap().remove("temperature");
                }
                ThinkingConfig::Disabled => {
                    body["thinking"] = serde_json::json!({ "type": "disabled" });
                }
            }
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
        let mut thinking = None;

        if let Some(content_blocks) = body["content"].as_array() {
            for block in content_blocks {
                match block["type"].as_str() {
                    Some("thinking") => {
                        if let Some(thought) = block["thinking"].as_str() {
                            let existing = thinking.get_or_insert_with(String::new);
                            if !existing.is_empty() {
                                existing.push('\n');
                            }
                            existing.push_str(thought);
                        }
                    }
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
            thinking,
        })
    }
}

/// Parse an Anthropic SSE byte stream into a stream of `StreamEvent`s.
///
/// Anthropic SSE format:
///   event: <event_type>\n
///   data: <json>\n
///   \n
fn parse_anthropic_sse(
    byte_stream: impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> impl futures_util::Stream<Item = Result<StreamEvent, LlmError>> + Send {
    futures_util::stream::unfold(
        (Box::pin(byte_stream), String::new(), 0_usize),
        |(mut stream, mut buffer, mut block_index)| async move {
            loop {
                // Try to extract a complete SSE frame from the buffer
                while let Some(frame_end) = buffer.find("\n\n") {
                    let frame = buffer[..frame_end].to_string();
                    buffer = buffer[frame_end + 2..].to_string();

                    let mut event_type = None;
                    let mut data = None;
                    for line in frame.lines() {
                        if let Some(val) = line.strip_prefix("event: ") {
                            event_type = Some(val.to_string());
                        } else if let Some(val) = line.strip_prefix("data: ") {
                            data = Some(val.to_string());
                        }
                    }

                    let event_type = match event_type {
                        Some(t) => t,
                        None => continue,
                    };

                    // Terminal event
                    if event_type == "message_stop" {
                        return None;
                    }

                    let data = match data {
                        Some(d) => d,
                        None => continue,
                    };

                    let json: serde_json::Value = match serde_json::from_str(&data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    match event_type.as_str() {
                        "content_block_start" => {
                            let block = &json["content_block"];
                            let block_type = block["type"].as_str().unwrap_or("");
                            if block_type == "tool_use" {
                                let id = block["id"].as_str().unwrap_or_default().to_string();
                                let name = block["name"].as_str().unwrap_or_default().to_string();
                                let idx = json["index"].as_u64().unwrap_or(block_index as u64) as usize;
                                block_index = idx + 1;
                                return Some((
                                    Ok(StreamEvent::ToolUseStart {
                                        index: idx,
                                        id,
                                        name,
                                    }),
                                    (stream, buffer, block_index),
                                ));
                            }
                            // text and thinking blocks don't need start events
                        }
                        "content_block_delta" => {
                            let delta = &json["delta"];
                            let delta_type = delta["type"].as_str().unwrap_or("");
                            match delta_type {
                                "text_delta" => {
                                    if let Some(text) = delta["text"].as_str() {
                                        return Some((
                                            Ok(StreamEvent::TextDelta {
                                                text: text.to_string(),
                                            }),
                                            (stream, buffer, block_index),
                                        ));
                                    }
                                }
                                "thinking_delta" => {
                                    if let Some(thinking) = delta["thinking"].as_str() {
                                        return Some((
                                            Ok(StreamEvent::ThinkingDelta {
                                                thinking: thinking.to_string(),
                                            }),
                                            (stream, buffer, block_index),
                                        ));
                                    }
                                }
                                "input_json_delta" => {
                                    if let Some(pj) = delta["partial_json"].as_str() {
                                        let idx = json["index"].as_u64().unwrap_or(0) as usize;
                                        return Some((
                                            Ok(StreamEvent::InputJsonDelta {
                                                index: idx,
                                                partial_json: pj.to_string(),
                                            }),
                                            (stream, buffer, block_index),
                                        ));
                                    }
                                }
                                _ => {}
                            }
                        }
                        "message_delta" => {
                            let stop_reason = json["delta"]["stop_reason"].as_str().unwrap_or("end_turn");
                            let usage = if let Some(u) = json["usage"].as_object() {
                                Usage {
                                    input_tokens: 0, // Input tokens come from message_start, not delta
                                    output_tokens: u.get("output_tokens")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0) as u32,
                                    ..Default::default()
                                }
                            } else {
                                Usage::default()
                            };
                            // Emit Usage then Done
                            // We buffer the Done event in the buffer to emit after Usage
                            let done_frame = format!(
                                "event: _done\ndata: {{\"finish_reason\":\"{}\"}}\n\n",
                                stop_reason
                            );
                            buffer = done_frame + &buffer;
                            return Some((
                                Ok(StreamEvent::Usage(usage)),
                                (stream, buffer, block_index),
                            ));
                        }
                        "_done" => {
                            // Synthetic event we injected after message_delta
                            let stop_reason = json["finish_reason"].as_str().unwrap_or("end_turn");
                            let finish_reason = match stop_reason {
                                "end_turn" => FinishReason::Stop,
                                "tool_use" => FinishReason::ToolUse,
                                "max_tokens" => FinishReason::MaxTokens,
                                _ => FinishReason::Stop,
                            };
                            return Some((
                                Ok(StreamEvent::Done { finish_reason }),
                                (stream, buffer, block_index),
                            ));
                        }
                        "message_start" => {
                            // Extract input token usage from the initial message
                            if let Some(usage) = json["message"]["usage"].as_object() {
                                let input_tokens = usage.get("input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0) as u32;
                                let cache_creation = usage.get("cache_creation_input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0) as u32;
                                let cache_read = usage.get("cache_read_input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0) as u32;
                                if input_tokens > 0 || cache_creation > 0 || cache_read > 0 {
                                    return Some((
                                        Ok(StreamEvent::Usage(Usage {
                                            input_tokens: input_tokens + cache_creation + cache_read,
                                            output_tokens: 0,
                                            cache_creation_input_tokens: cache_creation,
                                            cache_read_input_tokens: cache_read,
                                        })),
                                        (stream, buffer, block_index),
                                    ));
                                }
                            }
                        }
                        "error" => {
                            let message = json["error"]["message"]
                                .as_str()
                                .unwrap_or("Unknown streaming error")
                                .to_string();
                            return Some((
                                Ok(StreamEvent::Error { message }),
                                (stream, buffer, block_index),
                            ));
                        }
                        // ping, content_block_stop, etc. — ignore
                        _ => {}
                    }
                }

                // Need more data from the byte stream
                match stream.next().await {
                    Some(Ok(bytes)) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                    }
                    Some(Err(e)) => {
                        return Some((
                            Err(LlmError::Stream(e.to_string())),
                            (stream, buffer, block_index),
                        ));
                    }
                    None => {
                        // Stream ended — nothing more to yield
                        return None;
                    }
                }
            }
        },
    )
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
        let model_id = request.model.as_deref().unwrap_or(&self.model);

        // Prompt caching is GA as of 2024-10. No beta header required.
        // If needed for older API versions: .header("anthropic-beta", "prompt-caching-2024-07-31")
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

        if status == 429 {
            let retry_after_ms = parse_retry_after_ms(response.headers()).unwrap_or(1_000);
            tracing::warn!(
                provider = "anthropic",
                model = model_id,
                status,
                retry_after_ms,
                error_kind = "rate_limited",
                "Provider returned rate limit"
            );
            return Err(LlmError::RateLimited { retry_after_ms });
        }

        if status == 529 {
            let retry_after_ms = parse_retry_after_ms(response.headers());
            tracing::warn!(
                provider = "anthropic",
                model = model_id,
                status,
                retry_after_ms = ?retry_after_ms,
                error_kind = "overloaded",
                "Provider returned transient overload"
            );
            return Err(LlmError::Overloaded {
                status,
                retry_after_ms,
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

    fn supports_streaming(&self) -> bool {
        true
    }

    async fn chat_streaming_with_key(
        &self,
        key: &str,
        request: ChatRequest,
    ) -> Result<ChatStream, LlmError> {
        let mut body = self.build_request_body(&request);
        body["stream"] = serde_json::json!(true);
        let model_id = request.model.as_deref().unwrap_or(&self.model);

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

        if status == 429 {
            let retry_after_ms = parse_retry_after_ms(response.headers()).unwrap_or(1_000);
            return Err(LlmError::RateLimited { retry_after_ms });
        }

        if status == 529 {
            let retry_after_ms = parse_retry_after_ms(response.headers());
            return Err(LlmError::Overloaded {
                status,
                retry_after_ms,
            });
        }

        if status >= 400 {
            let error_body: serde_json::Value = response
                .json()
                .await
                .map_err(|e| LlmError::Serialization(e.to_string()))?;
            let message = error_body["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error")
                .to_string();
            return Err(LlmError::Api { status, message });
        }

        tracing::debug!(
            provider = "anthropic",
            model = model_id,
            "Streaming response started"
        );

        let byte_stream = response.bytes_stream();
        Ok(Box::pin(parse_anthropic_sse(byte_stream)))
    }
}

#[cfg(test)]
mod tests;
