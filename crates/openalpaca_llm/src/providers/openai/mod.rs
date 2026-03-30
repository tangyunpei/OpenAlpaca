mod request;
mod response;
mod streaming;

use crate::LlmProvider;
use crate::error::LlmError;
use crate::types::*;
use async_trait::async_trait;
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

    pub(crate) fn build_request_body(&self, request: &ChatRequest) -> serde_json::Value {
        request::build_request_body(&self.model, self.max_tokens, request)
    }

    pub(crate) fn parse_response(
        &self,
        body: serde_json::Value,
    ) -> Result<ChatResponse, LlmError> {
        response::parse_response(&self.model, body)
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
        if self.base_url == DEFAULT_BASE_URL {
            body["stream_options"] = serde_json::json!({"include_usage": true});
        }
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
        Ok(Box::pin(streaming::parse_openai_sse(byte_stream)))
    }
}

#[cfg(test)]
use streaming::parse_openai_sse;

#[cfg(test)]
mod tests;
