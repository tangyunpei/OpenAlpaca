use crate::LlmProvider;
use crate::error::LlmError;
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
}

impl OllamaProvider {
    pub fn new(model: String, base_url: Option<String>) -> Self {
        Self::with_client(reqwest::Client::new(), model, base_url)
    }

    /// Create with a shared `reqwest::Client` (for connection pool reuse).
    pub fn with_client(client: reqwest::Client, model: String, base_url: Option<String>) -> Self {
        let url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        #[cfg(feature = "openai")]
        {
            Self {
                inner: super::openai::OpenAiProvider::new_without_auth_with_client(
                    client, model, url,
                ),
            }
        }
        #[cfg(not(feature = "openai"))]
        {
            Self {
                client,
                model,
                base_url: url,
            }
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
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn list_models_with_key(&self, _key: &str) -> Result<Vec<String>, LlmError> {
        // Ollama uses native /api/tags endpoint (no auth needed)
        // Strip /v1 suffix from base_url to get native base
        let base = self.base_url();
        let native_base = base.trim_end_matches("/v1");
        let url = format!("{}/api/tags", native_base);

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
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

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        #[cfg(feature = "openai")]
        {
            self.inner.chat(request).await
        }
        #[cfg(not(feature = "openai"))]
        {
            // Standalone implementation (minimal OpenAI-compatible request)
            let url = format!("{}/chat/completions", self.base_url);
            let messages: Vec<serde_json::Value> = request
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

            let choice = &response_body["choices"][0];
            let content = choice["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let model = response_body["model"]
                .as_str()
                .unwrap_or(&self.model)
                .to_string();

            Ok(ChatResponse {
                content,
                tool_calls: vec![],
                model,
                usage: Usage::default(),
                finish_reason: FinishReason::Stop,
                thinking: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_base_url() {
        let provider = OllamaProvider::new("llama3".to_string(), None);
        assert_eq!(provider.base_url(), DEFAULT_BASE_URL);
    }

    #[test]
    fn test_custom_base_url() {
        let provider = OllamaProvider::new(
            "codellama".to_string(),
            Some("http://192.168.1.100:11434/v1".to_string()),
        );
        assert_eq!(provider.base_url(), "http://192.168.1.100:11434/v1");
    }
}
