mod request;
mod response;
mod streaming;

use crate::LlmProvider;
use crate::error::LlmError;
use crate::types::*;
use async_trait::async_trait;
use reqwest::header::HeaderMap;

const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_MAX_TOKENS: u32 = 4096;
/// Where the Messages and Models APIs live; [`AnthropicProvider::with_api_base`]
/// points a test at a loopback server instead.
const API_BASE: &str = "https://api.anthropic.com/v1";
/// Ids asked for per page of the Models API's list: the most it allows
/// (1 to 1000, default 20 — its reference page, read 2026-11-12).
const MODELS_PAGE_LIMIT: u32 = 1000;
const API_VERSION: &str = "2023-06-01";

/// How a models-list page that failed in transit or did not decode ends the
/// walk: the first page's failure is the call's error, as before; a later
/// page's keeps the ids already read, with one WARN.
fn later_page_failed(
    after_id: Option<&str>,
    models: Vec<String>,
    error: LlmError,
) -> Result<Vec<String>, LlmError> {
    if after_id.is_none() {
        return Err(error);
    }
    tracing::warn!(
        provider = "anthropic",
        error = %error,
        read = models.len(),
        "A later page of the model list failed — keeping the ids read so far"
    );
    Ok(models)
}

/// Build the `anthropic-beta` header value by collecting all active beta features.
///
/// Returns an empty string when no beta features are needed; the caller should
/// skip setting the header in that case.
fn build_beta_header(request: &ChatRequest) -> String {
    let mut flags: Vec<&str> = Vec::new();

    if request.context_management.is_some() {
        flags.push("context-management-2025-01-15");
    }

    flags.join(",")
}

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
    /// Total deadline for one non-streaming call, from
    /// [`with_request_timeout`](Self::with_request_timeout). `None` leaves the
    /// bound to the client's connect and idle timeouts alone (L7).
    request_timeout: Option<std::time::Duration>,
    /// [`API_BASE`], unless a test points the provider at its own server.
    api_base: String,
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
            request_timeout: None,
            api_base: API_BASE.to_string(),
        }
    }

    /// Send every request to `api_base` (`http://127.0.0.1:<port>/v1`) — a
    /// test's loopback server, never a provider.
    #[cfg(test)]
    pub(crate) fn with_api_base(mut self, api_base: String) -> Self {
        self.api_base = api_base;
        self
    }

    fn messages_url(&self) -> String {
        format!("{}/messages", self.api_base)
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

    #[cfg(test)]
    pub(crate) fn build_request_body(&self, request: &ChatRequest) -> serde_json::Value {
        request::build_request_body(&self.model, self.max_tokens, request)
    }

    #[cfg(test)]
    pub(crate) fn parse_response(
        &self,
        body: serde_json::Value,
    ) -> Result<ChatResponse, LlmError> {
        response::parse_response(&self.model, body)
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

    /// Every page of the Models API's list, newest first.
    ///
    /// The list is paged — at most [`MODELS_PAGE_LIMIT`] ids a page, the next
    /// asked for `after_id` the page's `last_id` while `has_more` holds — so
    /// one request saw only the newest 20 (the API's default `limit`). A page
    /// after the first that fails — an error status, a failed request or a
    /// body that does not decode — ends the walk with the ids read, one WARN;
    /// a failed first page answers as before. A `last_id` that is missing or
    /// repeats the cursor just sent ends it too.
    async fn list_models_with_key(&self, key: &str) -> Result<Vec<String>, LlmError> {
        let mut models: Vec<String> = Vec::new();
        let mut after_id: Option<String> = None;
        loop {
            let mut url = reqwest::Url::parse(&format!("{}/models", self.api_base))
                .map_err(|e| LlmError::Config(format!("the models URL: {e}")))?;
            url.query_pairs_mut()
                .append_pair("limit", &MODELS_PAGE_LIMIT.to_string());
            if let Some(ref cursor) = after_id {
                url.query_pairs_mut().append_pair("after_id", cursor);
            }
            let response = self
                .deadline(
                    self.client
                        .get(url)
                        .header("x-api-key", key)
                        .header("anthropic-version", API_VERSION),
                )
                .send()
                .await
                .map_err(crate::providers::reqwest_error);
            let response = match response {
                Ok(response) => response,
                Err(e) => return later_page_failed(after_id.as_deref(), models, e),
            };

            if !response.status().is_success() {
                if after_id.is_some() {
                    tracing::warn!(
                        provider = "anthropic",
                        status = response.status().as_u16(),
                        read = models.len(),
                        "A later page of the model list failed — keeping the ids read so far"
                    );
                }
                return Ok(models);
            }

            let body: serde_json::Value = match response
                .json()
                .await
                .map_err(crate::providers::reqwest_error)
            {
                Ok(body) => body,
                Err(e) => return later_page_failed(after_id.as_deref(), models, e),
            };

            if let Some(page) = body["data"].as_array() {
                models.extend(
                    page.iter()
                        .filter_map(|m| m["id"].as_str().map(|s| s.to_string())),
                );
            }

            let next = body["last_id"].as_str().filter(|id| !id.is_empty());
            match next {
                Some(next)
                    if body["has_more"].as_bool() == Some(true)
                        && after_id.as_deref() != Some(next) =>
                {
                    after_id = Some(next.to_string());
                }
                _ => return Ok(models),
            }
        }
    }

    async fn chat_with_key(
        &self,
        key: &str,
        request: ChatRequest,
    ) -> Result<ChatResponse, LlmError> {
        let body = request::build_request_body(&self.model, self.max_tokens, &request);
        let model_id = request.model.as_deref().unwrap_or(&self.model);

        let mut req_builder = self.deadline(
            self.client
                .post(self.messages_url())
                .header("x-api-key", key)
                .header("anthropic-version", API_VERSION)
                .header("content-type", "application/json"),
        );

        // Add anthropic-beta header when beta features are in use
        let beta_flags = build_beta_header(&request);
        if !beta_flags.is_empty() {
            req_builder = req_builder.header("anthropic-beta", beta_flags);
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
            .map_err(crate::providers::reqwest_error)?;

        if status >= 400 {
            let message = response_body["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error")
                .to_string();
            return Err(LlmError::Api { status, message });
        }

        response::parse_response(&self.model, response_body)
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    async fn chat_streaming_with_key(
        &self,
        key: &str,
        request: ChatRequest,
    ) -> Result<ChatStream, LlmError> {
        let mut body = request::build_request_body(&self.model, self.max_tokens, &request);
        body["stream"] = serde_json::json!(true);
        let model_id = request.model.as_deref().unwrap_or(&self.model);

        let mut req_builder = self
            .client
            .post(self.messages_url())
            .header("x-api-key", key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json");

        // Add anthropic-beta header when beta features are in use
        let beta_flags = build_beta_header(&request);
        if !beta_flags.is_empty() {
            req_builder = req_builder.header("anthropic-beta", beta_flags);
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
                .map_err(crate::providers::reqwest_error)?;
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
        Ok(Box::pin(streaming::parse_anthropic_sse(byte_stream)))
    }
}

#[cfg(test)]
use streaming::parse_anthropic_sse;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod echo_tests;
