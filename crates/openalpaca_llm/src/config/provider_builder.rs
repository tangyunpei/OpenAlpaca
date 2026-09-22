use std::sync::Arc;

use super::LlmRuntimeConfig;
use crate::{LlmProvider, ProviderType};

#[derive(Debug)]
pub(crate) enum ProviderBuildError {
    #[cfg(any(feature = "anthropic", feature = "openai"))]
    MissingKey,
    Unavailable,
}

/// Shared construction for startup and runtime registration. Error policy stays
/// with the caller: startup skips missing credentials, runtime reports them.
#[allow(unused_variables)]
pub(crate) fn build_provider(
    provider_type: &ProviderType,
    base_url: Option<String>,
    runtime: &LlmRuntimeConfig,
    first_key: Option<String>,
) -> Result<Arc<dyn LlmProvider>, ProviderBuildError> {
    let name = provider_type.to_string();
    let defaults = runtime.provider_defaults.get(&name);
    // Boot has already folded file defaults into runtime. Registration uses
    // the router's current snapshot, even if the file was edited more recently.
    let model = defaults.map(|d| d.default_model.clone());
    let max_tokens = defaults.map(|d| d.default_max_tokens);
    #[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
    let request_timeout = runtime.request_timeout_for(&name);
    #[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
    let client = crate::providers::build_http_client(request_timeout);
    match provider_type {
        #[cfg(feature = "anthropic")]
        ProviderType::Anthropic => Ok(Arc::new(
            crate::providers::anthropic::AnthropicProvider::with_client(
                client,
                first_key.ok_or(ProviderBuildError::MissingKey)?,
                model,
                max_tokens,
            )
            .with_request_timeout(request_timeout),
        )),
        #[cfg(feature = "openai")]
        ProviderType::OpenAI => Ok(Arc::new(
            crate::providers::openai::OpenAiProvider::with_client(
                client,
                first_key.ok_or(ProviderBuildError::MissingKey)?,
                model,
                base_url,
                max_tokens,
            )
            .with_request_timeout(request_timeout),
        )),
        #[cfg(feature = "ollama")]
        ProviderType::Ollama => Ok(Arc::new(
            crate::providers::ollama::OllamaProvider::with_client(
                client,
                model
                    .filter(|m| !m.trim().is_empty())
                    .unwrap_or_else(|| "llama3".into()),
                base_url,
                max_tokens,
            )
            .with_request_timeout(request_timeout),
        )),
        _ => Err(ProviderBuildError::Unavailable),
    }
}

#[cfg(all(test, feature = "openai"))]
mod tests {
    use super::*;
    use crate::test_support::{MockHttpServer, MockResponse};
    use crate::{ChatMessage, ChatRequest};

    #[tokio::test]
    async fn factory_uses_runtime_defaults_and_the_callers_endpoint() {
        let server = MockHttpServer::start(|_| {
            MockResponse::json(r#"{"choices":[{"message":{"content":"ok"}}]}"#)
        })
        .await;
        let mut runtime = LlmRuntimeConfig::default();
        let defaults = runtime.provider_defaults.get_mut("openai").unwrap();
        defaults.default_model = "runtime-model".into();
        defaults.default_max_tokens = 4321;
        // An explicit caller endpoint must win over this unused runtime URL.
        defaults.base_url = Some("http://127.0.0.1:1/unused".into());
        let provider = build_provider(
            &ProviderType::OpenAI,
            Some(server.base_url.clone()),
            &runtime,
            Some("fixture-key".into()),
        )
        .unwrap();
        provider
            .chat(ChatRequest {
                messages: Arc::new(vec![ChatMessage::user("ping")]),
                tools: Arc::new(vec![]),
                model: None,
                temperature: None,
                max_tokens: None,
                tool_choice: None,
                enable_caching: false,
                thinking: None,
                context_management: None,
                ephemeral_system_notice: None,
            })
            .await
            .unwrap();
        let requests = server.requests().await;
        let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
        assert_eq!(body["model"], "runtime-model");
        assert_eq!(body["max_tokens"], 4321);
    }
}
