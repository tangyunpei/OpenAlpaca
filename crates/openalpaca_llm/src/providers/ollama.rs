use crate::LlmProvider;
use crate::error::LlmError;
use crate::routing::model_registry::DiscoveredModel;
use crate::types::*;
use async_trait::async_trait;

const DEFAULT_BASE_URL: &str = "http://localhost:11434/v1";

/// Ollama provider that delegates to OpenAI-compatible endpoint.
pub struct OllamaProvider {
    #[cfg(feature = "openai")]
    inner: super::openai::OpenAiProvider,
    #[cfg(not(feature = "openai"))]
    client: reqwest::Client,
    #[cfg(not(feature = "openai"))]
    model: String,
    #[cfg(not(feature = "openai"))]
    base_url: String,
    /// Deadline for the native `/api/*` calls; the chat calls carry the inner
    /// provider's copy of it (L7).
    request_timeout: Option<std::time::Duration>,
}

impl OllamaProvider {
    pub fn new(model: String, base_url: Option<String>, max_tokens: Option<u32>) -> Self {
        Self::with_client(reqwest::Client::new(), model, base_url, max_tokens)
    }

    /// Create with a shared `reqwest::Client` (for connection pool reuse).
    ///
    /// `max_tokens` is `[providers.ollama] default_max_tokens`. Both
    /// construction paths pass it: it was dropped on the floor here, so a local
    /// model was capped at 4096 output tokens whatever the file said (L6).
    pub fn with_client(
        client: reqwest::Client,
        model: String,
        base_url: Option<String>,
        max_tokens: Option<u32>,
    ) -> Self {
        let url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        #[cfg(feature = "openai")]
        {
            Self {
                inner: super::openai::OpenAiProvider::new_without_auth_with_client(
                    client, model, url, max_tokens,
                )
                .with_flavour(super::openai::OpenAiFlavour::Ollama),
                request_timeout: None,
            }
        }
        #[cfg(not(feature = "openai"))]
        {
            let _ = max_tokens;
            Self {
                client,
                model,
                base_url: url,
                request_timeout: None,
            }
        }
    }

    /// Give one non-streaming call — a chat completion, `/api/tags`,
    /// `/api/show` — this much wall clock, and no more.
    ///
    /// A streamed completion deliberately does not carry it: see
    /// [`crate::providers::build_http_client`] (L7).
    pub fn with_request_timeout(mut self, request_timeout: std::time::Duration) -> Self {
        self.request_timeout = Some(request_timeout);
        #[cfg(feature = "openai")]
        {
            self.inner = self.inner.with_request_timeout(request_timeout);
        }
        self
    }

    /// The per-request deadline, applied to the native discovery calls.
    fn deadline(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.request_timeout {
            Some(timeout) => builder.timeout(timeout),
            None => builder,
        }
    }

    pub fn base_url(&self) -> &str {
        #[cfg(feature = "openai")]
        {
            &self.inner.base_url
        }
        #[cfg(not(feature = "openai"))]
        {
            &self.base_url
        }
    }

    /// The root Ollama serves its own API from — `base_url` without the
    /// OpenAI-compatibility `/v1` suffix.
    fn native_base(&self) -> &str {
        self.base_url().trim_end_matches('/').trim_end_matches("/v1")
    }

    /// The shared HTTP client, so discovery reuses the router's connection
    /// pool instead of standing up a new one per call.
    fn client(&self) -> &reqwest::Client {
        #[cfg(feature = "openai")]
        {
            self.inner.http_client()
        }
        #[cfg(not(feature = "openai"))]
        {
            &self.client
        }
    }

    /// `POST /api/show` for one installed tag.
    ///
    /// `Ok(None)` means the tag is not a chat model: an embedding-only model
    /// reports capabilities without `completion`, and registering it would
    /// offer the owner a model no turn can use. A build old enough to report no
    /// capabilities at all is taken at face value as a chat model rather than
    /// hidden.
    async fn show_model(&self, id: &str) -> Result<Option<DiscoveredModel>, LlmError> {
        let url = format!("{}/api/show", self.native_base());
        let response = self
            .deadline(self.client().post(&url))
            .json(&serde_json::json!({ "model": id }))
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Err(LlmError::Api {
                status,
                message: format!("/api/show refused '{id}'"),
            });
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| LlmError::Serialization(e.to_string()))?;

        let capabilities: Vec<&str> = body["capabilities"]
            .as_array()
            .map(|a| a.iter().filter_map(|c| c.as_str()).collect())
            .unwrap_or_default();
        if !capabilities.is_empty() && !capabilities.contains(&"completion") {
            return Ok(None);
        }

        // Ollama namespaces the key by architecture — "qwen3.context_length",
        // "llama.context_length" — so match on the suffix, not a fixed name.
        let context_window = body["model_info"]
            .as_object()
            .and_then(|info| {
                info.iter()
                    .find(|(k, _)| k.ends_with(".context_length"))
                    .and_then(|(_, v)| v.as_u64())
            })
            .and_then(|v| u32::try_from(v).ok())
            .filter(|w| *w > 0);

        Ok(Some(DiscoveredModel {
            id: id.to_string(),
            context_window,
            supports_image: capabilities.contains(&"vision"),
            supports_tools: capabilities.contains(&"tools"),
        }))
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    /// Ollama runs on the owner's machine and authenticates nothing (L1).
    fn requires_key(&self) -> bool {
        false
    }

    async fn list_models_with_key(&self, _key: &str) -> Result<Vec<String>, LlmError> {
        // Ollama uses native /api/tags endpoint (no auth needed)
        let url = format!("{}/api/tags", self.native_base());

        let response = self
            .deadline(self.client().get(&url))
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

        let models = body["models"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m["name"].as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        Ok(models)
    }

    /// Every installed chat model, described by Ollama's own API (L2).
    ///
    /// `/api/tags` names what is installed; one `/api/show` per tag gives the
    /// context length and the capability list. Both are unauthenticated —
    /// `key` is ignored, and discovery works with no key configured.
    async fn discover_models(&self, _key: &str) -> Result<Vec<DiscoveredModel>, LlmError> {
        let tags = self.list_models_with_key("").await?;
        let mut models = Vec::with_capacity(tags.len());
        for id in tags {
            match self.show_model(&id).await {
                Ok(Some(model)) => models.push(model),
                Ok(None) => {
                    tracing::info!(
                        model = %id,
                        "Ollama model cannot complete (embedding-only) — not registered as a chat model"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        model = %id,
                        error = %e,
                        "Ollama /api/show failed — registering the model without its context length or capabilities"
                    );
                    models.push(DiscoveredModel::bare(id));
                }
            }
        }
        Ok(models)
    }

    /// Ollama's `/v1` surface streams like any OpenAI-compatible one (L5).
    ///
    /// Without these three forwards the trait defaults applied: the whole
    /// answer was awaited and then replayed as one event, so a 27B model's
    /// reply landed in a single burst after a long silence.
    #[cfg(feature = "openai")]
    fn supports_streaming(&self) -> bool {
        self.inner.supports_streaming()
    }

    #[cfg(feature = "openai")]
    async fn chat_streaming(&self, request: ChatRequest) -> Result<ChatStream, LlmError> {
        self.inner.chat_streaming(request).await
    }

    #[cfg(feature = "openai")]
    async fn chat_streaming_with_key(
        &self,
        key: &str,
        request: ChatRequest,
    ) -> Result<ChatStream, LlmError> {
        self.inner.chat_streaming_with_key(key, request).await
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        #[cfg(feature = "openai")]
        {
            self.inner.chat(request).await
        }
        #[cfg(not(feature = "openai"))]
        {
            // Standalone implementation (minimal OpenAI-compatible request)
            let url = format!("{}/chat/completions", self.base_url);
            let mut messages: Vec<serde_json::Value> = request
                .messages
                .iter()
                .map(|m| {
                    let role = match m.role {
                        Role::System => "system",
                        Role::User => "user",
                        Role::Assistant => "assistant",
                        Role::Tool => "tool",
                    };
                    serde_json::json!({ "role": role, "content": m.content })
                })
                .collect();

            // Ephemeral system notice: append as tail system-role message (spec P0).
            if let Some(ref notice) = request.ephemeral_system_notice {
                messages.push(serde_json::json!({
                    "role": "system",
                    "content": notice,
                }));
            }

            let body = serde_json::json!({
                "model": request.model.as_deref().unwrap_or(&self.model),
                "messages": messages,
            });

            let response = self
                .client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| LlmError::Http(e.to_string()))?;

            let status = response.status().as_u16();
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

            // The shared OpenAI-compatible decoder, which is feature-independent
            // for exactly this branch: hand-decoding here dropped tool calls,
            // token usage and reasoning text on the floor.
            crate::openai_compat::parse_response(&self.model, response_body)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_base_url() {
        let provider = OllamaProvider::new("llama3".to_string(), None, None);
        assert_eq!(provider.base_url(), DEFAULT_BASE_URL);
    }

    #[test]
    fn test_custom_base_url() {
        let provider = OllamaProvider::new(
            "codellama".to_string(),
            Some("http://192.168.1.100:11434/v1".to_string()),
            None,
        );
        assert_eq!(provider.base_url(), "http://192.168.1.100:11434/v1");
    }

    /// Ollama delegates to the OpenAI provider's request builder when the
    /// `openai` feature is enabled (the default in the workspace). Exercise
    /// placement through that same builder: a tail system-role message carrying
    /// the notice when `ephemeral_system_notice.is_some()`.
    #[cfg(feature = "openai")]
    #[test]
    fn test_ollama_ephemeral_notice_placement() {
        use crate::providers::openai::request::build_request_body;
        use crate::types::{ChatMessage, ChatRequest};
        use std::sync::Arc;

        let request = ChatRequest {
            messages: Arc::new(vec![
                ChatMessage::system("real system"),
                ChatMessage::user("hello"),
            ]),
            tools: Arc::new(vec![]),
            model: None,
            temperature: None,
            max_tokens: None,
            tool_choice: None,
            enable_caching: false,
            thinking: None,
            context_management: None,
            ephemeral_system_notice: Some("[budget_notice]\nollama path\n[/budget_notice]".to_string()),
        };

        let body = build_request_body(
            "llama3",
            1024,
            crate::providers::openai::OpenAiFlavour::Ollama,
            &request,
        );
        let messages = body["messages"].as_array().unwrap();
        let last = messages.last().unwrap();
        assert_eq!(last["role"], "system");
        assert!(last["content"].as_str().unwrap().contains("budget_notice"));
    }
}
