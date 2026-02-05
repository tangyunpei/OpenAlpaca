use crate::error::LlmError;
use crate::types::*;
use crate::LlmProvider;
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
        let url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        #[cfg(feature = "openai")]
        {
            Self {
                inner: super::openai::OpenAiProvider::new_without_auth(model, url),
            }
        }
        #[cfg(not(feature = "openai"))]
        {
            Self {
                client: reqwest::Client::new(),
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
