pub(crate) mod request;
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

/// Which OpenAI-compatible server is on the other end.
///
/// The wire is the same but for the few places it is not, and the only one
/// today is how a call says it wants no reasoning (M2). Ollama's `/v1` takes
/// `reasoning_effort = "none"` — verified against a live 0.34 server, which
/// accepts `minimal|low|medium|high|xhigh|ultra|max|none` and answers `400` to
/// anything else. OpenAI's own API rejects the key outright on a model that
/// does not reason, so the default flavour keeps saying nothing and cloud
/// behaviour is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum OpenAiFlavour {
    #[default]
    OpenAi,
    Ollama,
}

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
    /// Total deadline for one non-streaming call, from
    /// [`with_request_timeout`](Self::with_request_timeout). `None` leaves the
    /// bound to the client's connect and idle timeouts alone (L7).
    request_timeout: Option<std::time::Duration>,
    /// Whose OpenAI-compatible wire this is — see [`OpenAiFlavour`].
    flavour: OpenAiFlavour,
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
            request_timeout: None,
            flavour: OpenAiFlavour::OpenAi,
        }
    }

    /// Create a provider without auth (for OpenAI-compatible APIs like Ollama).
    pub fn new_without_auth(model: String, base_url: String, max_tokens: Option<u32>) -> Self {
        Self::new_without_auth_with_client(reqwest::Client::new(), model, base_url, max_tokens)
    }

    /// Create a provider without auth, using a shared client.
    ///
    /// `max_tokens` is the output ceiling the caller's config asked for; `None`
    /// keeps the 4096 default. It used to be hard-coded here, which is how
    /// `[providers.ollama] default_max_tokens` came to be silently ignored (L6).
    pub fn new_without_auth_with_client(
        client: reqwest::Client,
        model: String,
        base_url: String,
        max_tokens: Option<u32>,
    ) -> Self {
        Self {
            client,
            api_key: None,
            model,
            base_url,
            max_tokens: max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            request_timeout: None,
            flavour: OpenAiFlavour::OpenAi,
        }
    }

    /// Declare whose OpenAI-compatible wire this is (M2).
    ///
    /// Only Ollama's wrapper calls this; everything else keeps the default,
    /// which behaves exactly as this provider always has.
    pub(crate) fn with_flavour(mut self, flavour: OpenAiFlavour) -> Self {
        self.flavour = flavour;
        self
    }

    /// Give one non-streaming call this much wall clock, and no more.
    ///
    /// Streaming deliberately does not carry it — see
    /// [`crate::providers::build_http_client`] (L7).
    pub fn with_request_timeout(mut self, request_timeout: std::time::Duration) -> Self {
        self.request_timeout = Some(request_timeout);
        self
    }

    /// The per-request deadline, applied to every non-streaming request.
    fn deadline(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.request_timeout {
            Some(timeout) => builder.timeout(timeout),
            None => builder,
        }
    }

    /// The shared `reqwest::Client` this provider was built with.
    ///
    /// Ollama wraps this provider and needs the same connection pool for its
    /// native `/api/*` discovery calls.
    pub(crate) fn http_client(&self) -> &reqwest::Client {
        &self.client
    }

    pub(crate) fn build_request_body(&self, request: &ChatRequest) -> serde_json::Value {
        request::build_request_body(&self.model, self.max_tokens, self.flavour, request)
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
        let mut req_builder = self.deadline(self.client.get(&url));

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

        let mut req_builder = self.deadline(
            self.client
                .post(&url)
                .header("content-type", "application/json"),
        );

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
        // Asked of every OpenAI-compatible base, not just api.openai.com: a
        // streamed turn that reports no usage is booked at zero tokens and zero
        // cost, so the caps and the usage screen see nothing (L5). Ollama
        // honours the flag; a base that does not simply ignores it.
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
        Ok(Box::pin(streaming::parse_openai_sse(byte_stream)))
    }
}

#[cfg(test)]
use streaming::parse_openai_sse;

#[cfg(test)]
mod tests;
