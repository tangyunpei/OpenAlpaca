//! Router tests for the local-model path, against a loopback mock server.
//!
//! Never the real Ollama: every request in here is answered by
//! [`crate::test_support::MockHttpServer`].

use super::*;
use crate::providers::ollama::OllamaProvider;
use crate::routing::model_registry::ModelInfo;
use crate::test_support::{MockHttpServer, MockResponse};

/// An OpenAI-shaped completion, which is what Ollama's `/v1` surface returns.
fn completion_body(model: &str, content: &str) -> String {
    serde_json::json!({
        "model": model,
        "choices": [{
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 11, "completion_tokens": 3}
    })
    .to_string()
}

fn request(model: &str) -> RouterRequest {
    RouterRequest {
        model: Some(model.to_string()),
        messages: Arc::new(vec![ChatMessage::user("ping")]),
        tools: Arc::new(vec![]),
        temperature: None,
        max_tokens: None,
        context: RequestContext::default(),
        tool_choice: None,
        tools_token_estimate: None,
        enable_caching: false,
        thinking: None,
        context_management: None,
        fallback_models: Vec::new(),
        ephemeral_system_notice: None,
    }
}

fn local_model_info() -> ModelInfo {
    ModelInfo {
        provider: ProviderType::Ollama,
        input_price_per_million: 0.0,
        output_price_per_million: 0.0,
        context_window: 262_144,
        discovered: true,
        supports_image: false,
        supports_audio: false,
        supports_document: false,
        supports_reasoning: false,
    }
}

/// A router holding one Ollama provider pointed at `base_url`, with an **empty**
/// key pool — the shape the boot builder produces for a keyless provider.
fn keyless_router(base_url: &str, model_id: &str) -> LlmRouter {
    let provider = Arc::new(OllamaProvider::new(
        model_id.to_string(),
        Some(format!("{base_url}/v1")),
    ));

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Ollama,
        ProviderEntry {
            provider,
            key_pool: Arc::new(ArcSwap::from_pointee(KeyPool::new(
                vec![],
                SelectionStrategy::RoundRobin,
            ))),
        },
    );

    let registry = ModelRegistry::new(HashMap::new());
    registry.register(model_id.to_string(), local_model_info());

    LlmRouter::new(
        providers,
        registry,
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        model_id.to_string(),
    )
}

// ── L1: a provider that needs no key is served by both router paths ─────────

/// Before the fix the empty pool answered `NoKeys`, the retry ladder broke to
/// `MaxRetriesExceeded` and no HTTP request was ever made.
#[tokio::test]
async fn a_keyless_provider_completes_through_the_non_streaming_path() {
    let server = MockHttpServer::start(|req| {
        if req.path == "/v1/chat/completions" {
            MockResponse::json(completion_body("local-model", "pong"))
        } else {
            MockResponse::not_found()
        }
    })
    .await;

    let router = keyless_router(&server.base_url, "local-model");
    let response = router
        .complete(request("local-model"))
        .await
        .expect("a keyless provider must be able to answer");

    assert_eq!(response.content, "pong");
    let seen = server.requests().await;
    assert_eq!(seen.len(), 1, "one call reached the provider");
    assert_eq!(seen[0].method, "POST");
    assert!(
        seen[0].body.contains("local-model"),
        "the router names the model on the wire: {}",
        seen[0].body
    );
}

/// Before the fix `max_attempts = pool.len().min(3)` was 0, so the loop body
/// never ran and the caller got "All keys are rate-limited".
#[tokio::test]
async fn a_keyless_provider_completes_through_the_streaming_path() {
    let server = MockHttpServer::start(|req| {
        if req.path == "/v1/chat/completions" {
            MockResponse::json(completion_body("local-model", "streamed pong"))
        } else {
            MockResponse::not_found()
        }
    })
    .await;

    let router = keyless_router(&server.base_url, "local-model");
    let stream = router
        .complete_streaming(request("local-model"))
        .await
        .expect("a keyless provider must be able to stream");

    let collected = crate::streaming::collect_stream(stream, "local-model".to_string())
        .await
        .expect("the stream completes");
    assert_eq!(collected.content, "streamed pong");
    assert_eq!(server.hits("/v1/chat/completions").await, 1);
}

/// The slot is for providers that need no key — not a way around a missing one.
#[tokio::test]
async fn an_empty_pool_still_refuses_a_provider_that_needs_a_key() {
    let server = MockHttpServer::start(|_| MockResponse::json(completion_body("gpt-5.2", "hi"))).await;

    let provider = Arc::new(crate::providers::openai::OpenAiProvider::new(
        "sk-test".to_string(),
        Some("gpt-5.2".to_string()),
        Some(format!("{}/v1", server.base_url)),
        None,
    ));
    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::OpenAI,
        ProviderEntry {
            provider,
            key_pool: Arc::new(ArcSwap::from_pointee(KeyPool::new(
                vec![],
                SelectionStrategy::RoundRobin,
            ))),
        },
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "gpt-5.2".to_string(),
    );

    let err = router
        .complete(request("gpt-5.2"))
        .await
        .expect_err("a provider that needs a key and has none cannot be served");
    assert!(
        matches!(err, LlmRouterError::AllFallbacksFailed),
        "{err:?}"
    );
    assert_eq!(server.hits("/v1/chat/completions").await, 0);
}

/// The fact the CLI and GUI render as "no key needed".
#[tokio::test]
async fn the_router_reports_a_local_provider_as_needing_no_key() {
    let server = MockHttpServer::start(|_| MockResponse::not_found()).await;
    let router = keyless_router(&server.base_url, "local-model");

    assert_eq!(router.provider_requires_key(&ProviderType::Ollama), Some(false));
    assert_eq!(router.provider_requires_key(&ProviderType::Anthropic), None);
    // And no key was invented to make the call work.
    assert!(
        router
            .key_statuses(&ProviderType::Ollama)
            .await
            .expect("the provider is loaded")
            .is_empty()
    );
}
