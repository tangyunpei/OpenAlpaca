pub(crate) mod request;
mod response;
mod responses;
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
    /// Whether a request with tools goes to the Responses API (C7b): set
    /// when `base_url` is OpenAI's own API ([`is_openai_api`]), never for a
    /// keyless or Ollama provider; see [`Self::uses_responses`].
    responses_api: bool,
}

/// The hosts of OpenAI's own API: `api.openai.com` and the regional hosts
/// of its data-residency table, each of which serves `/v1/responses`
/// (developers.openai.com/api/docs/guides/your-data, "Support by region",
/// read 2026-11-12).
const OPENAI_API_HOSTS: &[&str] = &[
    "api.openai.com",
    "us.api.openai.com",
    "eu.api.openai.com",
    "au.api.openai.com",
    "ca.api.openai.com",
    "jp.api.openai.com",
    "in.api.openai.com",
    "sg.api.openai.com",
    "kr.api.openai.com",
    "gb.api.openai.com",
    "ae.api.openai.com",
];

/// Whether `base_url` is OpenAI's own API: `https` to one of
/// [`OPENAI_API_HOSTS`], whatever the path. Every other OpenAI-compatible
/// server — Ollama, llama.cpp, LM Studio, a gateway — keeps Chat
/// Completions (C7b).
pub(crate) fn is_openai_api(base_url: &str) -> bool {
    reqwest::Url::parse(base_url).is_ok_and(|url| {
        url.scheme() == "https"
            && url
                .host_str()
                .is_some_and(|host| OPENAI_API_HOSTS.contains(&host))
    })
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
        let base_url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Self {
            client,
            api_key: Some(api_key),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            responses_api: is_openai_api(&base_url),
            base_url,
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
            responses_api: false,
        }
    }

    /// Send a request with tools through the Responses API, or not, whatever
    /// the `base_url` (C7b). Only tests call it: they point the provider at a
    /// loopback mock that stands in for OpenAI's own API.
    #[cfg(test)]
    pub(crate) fn with_responses_api(mut self, on: bool) -> Self {
        self.responses_api = on;
        self
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

    /// Whether `request` goes to `/responses` rather than
    /// `/chat/completions` (C7b): on OpenAI's own API, a request that calls
    /// functions — it offers tools, or its history holds a tool call or a
    /// tool result. OpenAI's current reasoning models take tool calls with
    /// reasoning only there: "Tool calling requires the Responses API"
    /// (gpt-6.1-sol's page), and "Starting with GPT-5.4, Chat Completions
    /// does not support tool calling with `reasoning_effort` values other
    /// than `none`" (the migration guide); "GPT-6 Sol
    /// and GPT-6 Luna support function calling in Chat Completions only with
    /// `reasoning_effort: "none"`" (the GPT-6 guide; all three read
    /// 2026-11-12). Every other request
    /// keeps Chat Completions, which every catalogued OpenAI model supports
    /// (their pages, the same day), as does every other OpenAI-compatible
    /// server.
    pub(crate) fn uses_responses(&self, request: &ChatRequest) -> bool {
        self.responses_api
            && self.flavour == OpenAiFlavour::OpenAi
            && (!request.tools.is_empty()
                || request
                    .messages
                    .iter()
                    .any(|m| m.tool_calls.is_some() || m.tool_call_id.is_some()))
    }

    /// The route and body for `request`, on whichever wire it takes.
    fn route(&self, request: &ChatRequest) -> (String, serde_json::Value) {
        if self.uses_responses(request) {
            (
                format!("{}/responses", self.base_url),
                self.build_responses_body(request),
            )
        } else {
            (
                format!("{}/chat/completions", self.base_url),
                self.build_request_body(request),
            )
        }
    }

    /// The Responses API body for `request` (C7b).
    pub(crate) fn build_responses_body(&self, request: &ChatRequest) -> serde_json::Value {
        responses::build_request_body(&self.model, self.max_tokens, request)
    }

    /// A whole Responses API answer, read (C7b).
    pub(crate) fn parse_responses_response(
        &self,
        body: serde_json::Value,
    ) -> Result<ChatResponse, LlmError> {
        responses::parse_response(&self.model, body)
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
            .map_err(crate::providers::reqwest_error)?;

        if !response.status().is_success() {
            return Ok(vec![]);
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(crate::providers::reqwest_error)?;

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
        let responses = self.uses_responses(&request);
        let (url, body) = self.route(&request);
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
            .map_err(crate::providers::reqwest_error)?;

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
            .map_err(crate::providers::reqwest_error)?;

        if status >= 400 {
            let message = response_body["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error")
                .to_string();
            return Err(LlmError::Api { status, message });
        }

        if responses {
            self.parse_responses_response(response_body)
        } else {
            self.parse_response(response_body)
        }
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    async fn chat_streaming_with_key(
        &self,
        key: &str,
        request: ChatRequest,
    ) -> Result<ChatStream, LlmError> {
        let responses = self.uses_responses(&request);
        let (url, mut body) = self.route(&request);
        body["stream"] = serde_json::json!(true);
        if !responses {
            // Asked of every OpenAI-compatible base, not just api.openai.com:
            // a streamed turn that reports no usage is booked at zero tokens
            // and zero cost, so the caps and the usage screen see nothing
            // (L5). Ollama honours the flag; a base that does not simply
            // ignores it. The Responses API's `stream_options` is another
            // object, and its `response.completed` event always carries the
            // usage (the reference, read 2026-11-12).
            body["stream_options"] = serde_json::json!({"include_usage": true});
        }

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
            .map_err(crate::providers::reqwest_error)?;

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
                .map_err(crate::providers::reqwest_error)?;
            let message = error_body["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error")
                .to_string();
            return Err(LlmError::Api { status, message });
        }

        let byte_stream = response.bytes_stream();
        if responses {
            Ok(Box::pin(responses::parse_responses_sse(byte_stream)))
        } else {
            Ok(Box::pin(streaming::parse_openai_sse(byte_stream)))
        }
    }
}

#[cfg(test)]
use responses::parse_responses_sse;
#[cfg(test)]
use streaming::parse_openai_sse;

#[cfg(test)]
mod tests;
