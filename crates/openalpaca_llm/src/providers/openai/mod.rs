use crate::LlmProvider;
use crate::error::LlmError;
use crate::types::*;
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::HeaderMap;

const DEFAULT_MODEL: &str = "gpt-4o";
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_MAX_TOKENS: u32 = 4096;

fn parse_retry_after_ms(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<f64>().ok())
        .map(|secs| ((secs.max(0.0)) * 1000.0).round() as u64)
        .filter(|ms| *ms > 0)
}

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

    /// Build OpenAI content value from a ChatMessage.
    ///
    /// If the message has multimodal `parts`, builds an array of content objects
    /// in OpenAI's format. If parts is None, returns a plain string value.
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
                ContentPart::Image { source, detail } => {
                    let url = match source {
                        ImageSource::Base64 { media_type, data } => {
                            format!("data:{};base64,{}", media_type, data.as_str())
                        }
                        ImageSource::Url { url } => url.clone(),
                        ImageSource::FileAsset { file_id, media_type } => {
                            // Should be resolved before reaching provider
                            return Some(serde_json::json!({
                                "type": "text",
                                "text": format!("[image file_id={} not resolved — media_type={}]", file_id, media_type),
                            }));
                        }
                    };
                    let detail_val = detail.as_deref().unwrap_or("auto");
                    Some(serde_json::json!({
                        "type": "image_url",
                        "image_url": {
                            "url": url,
                            "detail": detail_val,
                        }
                    }))
                }
                ContentPart::Audio { data, format } => Some(serde_json::json!({
                    "type": "input_audio",
                    "input_audio": {
                        "data": data.as_str(),
                        "format": format,
                    }
                })),
                ContentPart::Document {
                    filename,
                    mime_type,
                    extracted_text,
                    ..
                } => {
                    // OpenAI has no native document input; use extracted text fallback
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
            return serde_json::Value::String("[empty message]".to_string());
        }

        serde_json::Value::Array(blocks)
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

                let content = if matches!(msg.role, Role::User) {
                    Self::build_message_content(msg)
                } else {
                    serde_json::Value::String(msg.content.clone())
                };

                let mut obj = serde_json::json!({
                    "role": role,
                    "content": content,
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

        // OpenAI reasoning models (o-series): map ThinkingConfig → reasoning_effort.
        // When reasoning is active, temperature must be omitted (top_p is not currently
        // a ChatRequest field, so no stripping needed).
        if let Some(ref thinking) = request.thinking {
            match thinking {
                ThinkingConfig::Enabled { .. } => {
                    body["reasoning_effort"] = serde_json::json!("high");
                    body.as_object_mut().unwrap().remove("temperature");
                }
                ThinkingConfig::Adaptive => {
                    body["reasoning_effort"] = serde_json::json!("medium");
                    body.as_object_mut().unwrap().remove("temperature");
                }
                ThinkingConfig::Disabled => {
                    // Omit reasoning_effort — default behavior
                }
            }
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
                    // OpenAI has no `input_examples` equivalent (Anthropic-only feature for
                    // demonstrating expected tool usage patterns). The field is intentionally
                    // skipped — there is no OpenAI API parameter to map it to.
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

    pub(crate) fn parse_response(&self, body: serde_json::Value) -> Result<ChatResponse, LlmError> {
        let model = body["model"].as_str().unwrap_or(&self.model).to_string();

        let usage = Usage {
            input_tokens: body["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: body["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32,
            // OpenAI returns cached token count in prompt_tokens_details.cached_tokens
            // when automatic prompt caching is active. No cache_creation equivalent
            // (OpenAI caching is server-side and automatic, no write cost).
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

        Ok(ChatResponse {
            content,
            tool_calls,
            model,
            usage,
            finish_reason,
            // OpenAI o-series models return reasoning in message.reasoning_content
            thinking: message["reasoning_content"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
        })
    }
}

/// Parse an OpenAI SSE byte stream into a stream of `StreamEvent`s.
///
/// OpenAI SSE format:
///   data: {"choices":[{"delta":{"content":"text"}}]}\n
///   \n
///   data: [DONE]\n
fn parse_openai_sse(
    byte_stream: impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> impl futures_util::Stream<Item = Result<StreamEvent, LlmError>> + Send {
    futures_util::stream::unfold(
        (Box::pin(byte_stream), String::new()),
        |(mut stream, mut buffer)| async move {
            loop {
                // Try to extract complete lines from the buffer
                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim_end().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    // Skip empty lines (SSE frame separators)
                    if line.is_empty() {
                        continue;
                    }

                    let data = match line.strip_prefix("data: ") {
                        Some(d) => d,
                        None => continue,
                    };

                    // Terminal marker
                    if data == "[DONE]" {
                        return None;
                    }

                    let json: serde_json::Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let choice = &json["choices"][0];
                    let delta = &choice["delta"];

                    // Text content delta
                    if let Some(text) = delta["content"].as_str()
                        && !text.is_empty()
                    {
                        return Some((
                            Ok(StreamEvent::TextDelta {
                                text: text.to_string(),
                            }),
                            (stream, buffer),
                        ));
                    }

                    // Reasoning content delta (OpenAI o-series streaming)
                    if let Some(reasoning) = delta["reasoning_content"].as_str()
                        && !reasoning.is_empty()
                    {
                        return Some((
                            Ok(StreamEvent::ThinkingDelta {
                                thinking: reasoning.to_string(),
                            }),
                            (stream, buffer),
                        ));
                    }

                    // Tool calls delta
                    if let Some(tool_calls) = delta["tool_calls"].as_array() {
                        for tc in tool_calls {
                            let index = tc["index"].as_u64().unwrap_or(0) as usize;

                            // Tool call start (has id + function.name)
                            if let Some(id) = tc["id"].as_str() {
                                let name = tc["function"]["name"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string();
                                return Some((
                                    Ok(StreamEvent::ToolUseStart {
                                        index,
                                        id: id.to_string(),
                                        name,
                                    }),
                                    (stream, buffer),
                                ));
                            }

                            // Tool call arguments delta
                            if let Some(args) = tc["function"]["arguments"].as_str()
                                && !args.is_empty()
                            {
                                return Some((
                                    Ok(StreamEvent::InputJsonDelta {
                                        index,
                                        partial_json: args.to_string(),
                                    }),
                                    (stream, buffer),
                                ));
                            }
                        }
                    }

                    // Finish reason
                    if let Some(finish_reason_str) = choice["finish_reason"].as_str() {
                        // Extract usage if present in this chunk
                        let usage = if let Some(u) = json["usage"].as_object() {
                            Usage {
                                input_tokens: u
                                    .get("prompt_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0) as u32,
                                output_tokens: u
                                    .get("completion_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0) as u32,
                                cache_read_input_tokens: u
                                    .get("prompt_tokens_details")
                                    .and_then(|d| d.get("cached_tokens"))
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0) as u32,
                                ..Default::default()
                            }
                        } else {
                            Usage::default()
                        };

                        // Inject synthetic Done event after Usage
                        let done_line = format!(
                            "data: {}\n",
                            serde_json::json!({"_done": finish_reason_str})
                        );
                        buffer = done_line + &buffer;

                        return Some((
                            Ok(StreamEvent::Usage(usage)),
                            (stream, buffer),
                        ));
                    }

                    // Handle synthetic _done events
                    if let Some(fr_str) = json["_done"].as_str() {
                        let finish_reason = match fr_str {
                            "stop" => FinishReason::Stop,
                            "tool_calls" => FinishReason::ToolUse,
                            "length" => FinishReason::MaxTokens,
                            _ => FinishReason::Stop,
                        };
                        return Some((
                            Ok(StreamEvent::Done { finish_reason }),
                            (stream, buffer),
                        ));
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
                            (stream, buffer),
                        ));
                    }
                    None => {
                        return None;
                    }
                }
            }
        },
    )
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
        let model_id = request.model.as_deref().unwrap_or(&self.model);

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
            let retry_after_ms = parse_retry_after_ms(response.headers()).unwrap_or(1_000);
            tracing::warn!(
                provider = "openai",
                model = model_id,
                status,
                retry_after_ms,
                error_kind = "rate_limited",
                "Provider returned rate limit"
            );
            return Err(LlmError::RateLimited { retry_after_ms });
        }

        if status == 503 || status == 529 {
            let retry_after_ms = parse_retry_after_ms(response.headers());
            tracing::warn!(
                provider = "openai",
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
        // Request usage in stream chunks (OpenAI stream_options)
        body["stream_options"] = serde_json::json!({"include_usage": true});
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
            let retry_after_ms = parse_retry_after_ms(response.headers()).unwrap_or(1_000);
            return Err(LlmError::RateLimited { retry_after_ms });
        }

        if status == 503 || status == 529 {
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

        let byte_stream = response.bytes_stream();
        Ok(Box::pin(parse_openai_sse(byte_stream)))
    }
}

#[cfg(test)]
mod tests;
