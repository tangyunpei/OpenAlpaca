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

/// An OpenAI-shaped SSE stream — Ollama's `/v1` dialect: one frame per delta,
/// then the frame that carries `finish_reason` and, because
/// `stream_options.include_usage` was asked for, the token counts.
fn sse_frames(deltas: &[&str], prompt_tokens: u32, completion_tokens: u32) -> Vec<String> {
    let mut frames: Vec<String> = deltas
        .iter()
        .map(|text| {
            format!(
                "data: {}\n\n",
                serde_json::json!({
                    "choices": [{"index": 0, "delta": {"content": text}}]
                })
            )
        })
        .collect();
    frames.push(format!(
        "data: {}\n\n",
        serde_json::json!({
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens}
        })
    ));
    frames.push("data: [DONE]\n\n".to_string());
    frames
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
        supports_tools: true,
        declared: true,
    }
}

/// A router holding one Ollama provider pointed at `base_url`, with an **empty**
/// key pool — the shape the boot builder produces for a keyless provider.
fn keyless_router(base_url: &str, model_id: &str) -> LlmRouter {
    let provider = Arc::new(OllamaProvider::new(
        model_id.to_string(),
        Some(format!("{base_url}/v1")),
        None,
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
            MockResponse::sse(sse_frames(&["streamed ", "pong"], 11, 3))
        } else {
            MockResponse::not_found()
        }
    })
    .await;

    let router = keyless_router(&server.base_url, "local-model");
    let routed = router
        .complete_streaming(request("local-model"))
        .await
        .expect("a keyless provider must be able to stream");
    assert_eq!(routed.model, "local-model", "the router names what it called");

    let collected = crate::streaming::collect_stream(routed.stream, routed.model.clone())
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

// ── L5: tokens arrive as they are produced, with real usage ─────────────────

/// Drain a stream into the events it actually emitted, in order.
async fn drain(mut stream: ChatStream) -> Vec<StreamEvent> {
    use futures_util::StreamExt;
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.expect("no stream error"));
    }
    events
}

/// Before the fix `OllamaProvider` forwarded no streaming method, so the trait
/// default awaited the whole answer and replayed it as **one** `TextDelta` —
/// and the request never carried `"stream": true`.
#[tokio::test]
async fn a_streamed_local_turn_arrives_as_separate_events_with_usage() {
    let server = MockHttpServer::start(|req| {
        if req.path == "/v1/chat/completions" {
            MockResponse::sse(sse_frames(&["Hel", "lo", " there"], 11, 3))
                .every(std::time::Duration::from_millis(20))
        } else {
            MockResponse::not_found()
        }
    })
    .await;

    let router = keyless_router(&server.base_url, "local-model");
    let events = drain(
        router
            .complete_streaming(request("local-model"))
            .await
            .expect("a keyless provider must be able to stream")
            .stream,
    )
    .await;

    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        deltas,
        vec!["Hel", "lo", " there"],
        "each frame is its own event, not one burst at the end: {events:?}"
    );

    let usage = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::Usage(u) => Some(u),
            _ => None,
        })
        .expect("the stream reports usage");
    assert_eq!((usage.input_tokens, usage.output_tokens), (11, 3));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::Done { finish_reason: FinishReason::Stop })),
        "the stream finishes: {events:?}"
    );

    let seen = server.requests().await;
    assert_eq!(seen.len(), 1);
    let body: serde_json::Value = serde_json::from_str(&seen[0].body).expect("a JSON body");
    assert_eq!(body["stream"], serde_json::json!(true), "{}", seen[0].body);
    assert_eq!(
        body["stream_options"]["include_usage"],
        serde_json::json!(true),
        "usage is asked for on every OpenAI-compatible base, not just OpenAI's: {}",
        seen[0].body
    );
}

/// The other shape: OpenAI puts the totals in a trailing frame whose `choices`
/// is empty, after the one carrying `finish_reason`. Those tokens were dropped.
#[tokio::test]
async fn a_trailing_usage_frame_is_not_dropped() {
    let server = MockHttpServer::start(|req| {
        if req.path == "/v1/chat/completions" {
            MockResponse::sse(vec![
                format!(
                    "data: {}\n\n",
                    serde_json::json!({"choices": [{"index": 0, "delta": {"content": "hi"}}]})
                ),
                format!(
                    "data: {}\n\n",
                    serde_json::json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})
                ),
                format!(
                    "data: {}\n\n",
                    serde_json::json!({
                        "choices": [],
                        "usage": {"prompt_tokens": 42, "completion_tokens": 7}
                    })
                ),
                "data: [DONE]\n\n".to_string(),
            ])
        } else {
            MockResponse::not_found()
        }
    })
    .await;

    let router = keyless_router(&server.base_url, "local-model");
    let routed = router
        .complete_streaming(request("local-model"))
        .await
        .expect("the stream starts");
    let collected = crate::streaming::collect_stream(routed.stream, routed.model)
        .await
        .expect("the stream completes");

    assert_eq!(collected.content, "hi");
    assert_eq!(collected.usage.input_tokens, 42);
    assert_eq!(collected.usage.output_tokens, 7);
}

// ── L2: installed models come from Ollama's own API ─────────────────────────

/// `/api/tags` naming two models, `/api/show` describing them.
fn tags_body(names: &[&str]) -> String {
    serde_json::json!({
        "models": names.iter().map(|n| serde_json::json!({"name": n})).collect::<Vec<_>>()
    })
    .to_string()
}

fn show_body(arch: &str, context: u64, capabilities: &[&str]) -> String {
    serde_json::json!({
        "capabilities": capabilities,
        "model_info": {
            format!("{arch}.context_length"): context,
            format!("{arch}.embedding_length"): 4096,
        }
    })
    .to_string()
}

/// A router whose only provider is a keyless Ollama at `base_url`, with an
/// empty catalogue — exactly what a first boot with no `[models]` row has.
fn empty_catalogue_router(base_url: &str) -> LlmRouter {
    let provider = Arc::new(OllamaProvider::new(
        "unset".to_string(),
        Some(format!("{base_url}/v1")),
        None,
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
    LlmRouter::new(
        providers,
        ModelRegistry::new(HashMap::new()),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "unset".to_string(),
    )
}

/// The whole of L2 in one pass: no key is needed, the context length and the
/// capabilities come from `/api/show`, and an embedding-only model is not
/// offered as a chat model.
#[tokio::test]
async fn discovery_reads_tags_and_show_with_no_key() {
    let server = MockHttpServer::start(|req| match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/tags") => MockResponse::json(tags_body(&["chat-model:q8", "embed-model"])),
        ("POST", "/api/show") if req.body.contains("chat-model:q8") => MockResponse::json(
            show_body("qwen3", 262_144, &["completion", "tools", "vision", "thinking"]),
        ),
        ("POST", "/api/show") => MockResponse::json(show_body("bert", 512, &["embedding"])),
        _ => MockResponse::not_found(),
    })
    .await;

    let router = empty_catalogue_router(&server.base_url);
    let status = router.refresh_models_for(&ProviderType::Ollama).await;

    assert_eq!(status.models, 1, "the embedding-only model is not a chat model");
    assert_eq!(status.error, None);

    let models = router.available_models();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "chat-model:q8");
    assert_eq!(models[0].provider, "ollama");
    assert_eq!(models[0].context_window, 262_144);
    assert_eq!(models[0].input_price_per_million, 0.0);
    assert_eq!(models[0].output_price_per_million, 0.0);
    assert!(models[0].supports_tools);
    assert!(router.model_registry().supports_image("chat-model:q8"));
    assert!(
        router.model_registry().get_model_info("embed-model").is_none(),
        "an embedding-only model is never registered"
    );
    // Discovery is not gated on a key: the pool is empty and it still ran.
    assert_eq!(server.hits("/api/tags").await, 1);
    assert_eq!(server.hits("/api/show").await, 2);
}

/// `/api/show` that says nothing still yields a usable model.
#[tokio::test]
async fn a_model_without_a_reported_context_length_falls_back_to_8192() {
    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/api/tags" => MockResponse::json(tags_body(&["quiet-model"])),
        "/api/show" => MockResponse::json(r#"{}"#.to_string()),
        _ => MockResponse::not_found(),
    })
    .await;

    let router = empty_catalogue_router(&server.base_url);
    router.refresh_models_for(&ProviderType::Ollama).await;

    let info = router
        .model_registry()
        .get_model_info("quiet-model")
        .expect("a model with no metadata is still registered");
    assert_eq!(info.context_window, 8192);
    assert!(info.discovered);
}

/// A tag that is no longer installed cannot be served, so it goes.
#[tokio::test]
async fn a_model_that_disappears_from_tags_is_withdrawn() {
    let round = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&round);
    let server = MockHttpServer::start(move |req| match req.path.as_str() {
        "/api/tags" => {
            let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                MockResponse::json(tags_body(&["kept", "removed"]))
            } else {
                MockResponse::json(tags_body(&["kept"]))
            }
        }
        "/api/show" => MockResponse::json(show_body("llama", 8192, &["completion", "tools"])),
        _ => MockResponse::not_found(),
    })
    .await;

    let router = empty_catalogue_router(&server.base_url);
    router.refresh_models_for(&ProviderType::Ollama).await;
    assert_eq!(router.available_models().len(), 2);

    let status = router.refresh_models_for(&ProviderType::Ollama).await;
    assert_eq!(status.models, 1);
    let ids: Vec<String> = router.available_models().into_iter().map(|m| m.id).collect();
    assert_eq!(ids, vec!["kept".to_string()]);
}

/// A declared row survives a withdrawal — it only leaves the picker.
///
/// The settings service re-applies `[models]` on every provider enable, so a
/// refresh that deleted those rows would undo the enable's own work. It is
/// un-discovered instead: gone from the picker, still the owner's declaration.
#[tokio::test]
async fn a_declared_model_that_is_not_installed_is_un_discovered_not_deleted() {
    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/api/tags" => MockResponse::json(tags_body(&["installed"])),
        "/api/show" => MockResponse::json(show_body("llama", 8192, &["completion", "tools"])),
        _ => MockResponse::not_found(),
    })
    .await;

    let router = empty_catalogue_router(&server.base_url);
    let mut declared = std::collections::HashMap::new();
    declared.insert(
        "never-pulled".to_string(),
        crate::config::ModelConfigEntry {
            provider: "ollama".to_string(),
            input_price: Some(0.0),
            output_price: Some(0.0),
            context: Some(4096),
            supports_image: None,
            supports_audio: None,
            supports_document: None,
            supports_reasoning: None,
            supports_tools: None,
        },
    );
    router
        .model_registry()
        .reload_from_config(&declared, &std::collections::HashSet::new());

    router.refresh_models_for(&ProviderType::Ollama).await;

    assert!(
        router.model_registry().get_model_info("never-pulled").is_some(),
        "the owner's declaration is not deleted behind their back"
    );
    let offered: Vec<String> = router.available_models().into_iter().map(|m| m.id).collect();
    assert_eq!(
        offered,
        vec!["installed".to_string()],
        "but it is not offered as installed"
    );
    assert_eq!(
        router.effective_default_model().as_deref(),
        Some("installed"),
        "and the ladder picks the one that is really there"
    );
}

/// A `[models]` row wins over what discovery reports, and is never required.
#[tokio::test]
async fn a_declared_row_overrides_the_discovered_metadata() {
    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/api/tags" => MockResponse::json(tags_body(&["declared-model"])),
        "/api/show" => MockResponse::json(show_body("llama", 4096, &["completion"])),
        _ => MockResponse::not_found(),
    })
    .await;

    let router = empty_catalogue_router(&server.base_url);
    let mut declared = std::collections::HashMap::new();
    declared.insert(
        "declared-model".to_string(),
        crate::config::ModelConfigEntry {
            provider: "ollama".to_string(),
            input_price: Some(0.0),
            output_price: Some(0.0),
            context: Some(131_072),
            supports_image: Some(true),
            supports_audio: None,
            supports_document: None,
            supports_reasoning: None,
            supports_tools: None,
        },
    );
    router
        .model_registry()
        .reload_from_config(&declared, &std::collections::HashSet::new());

    router.refresh_models_for(&ProviderType::Ollama).await;

    let info = router
        .model_registry()
        .get_model_info("declared-model")
        .expect("declared and installed");
    assert_eq!(info.context_window, 131_072, "the row's context wins");
    assert!(info.supports_image, "the row's flags win");
    assert!(info.discovered, "and it is confirmed as installed");
}

/// Ollama down is a WARN and an empty list, never a failure — and the reason is
/// readable rather than looking like "nothing installed".
#[tokio::test]
async fn an_unreachable_ollama_leaves_the_provider_registered() {
    let server = MockHttpServer::start(|_| MockResponse::error(500, r#"{"error":"boom"}"#)).await;
    let base = server.base_url.clone();
    drop(server); // nothing is listening on that port any more

    let router = empty_catalogue_router(&base);
    let status = router.refresh_models_for(&ProviderType::Ollama).await;

    assert_eq!(status.models, 0);
    assert!(status.error.is_some(), "the failure is recorded, not swallowed");
    assert!(router.has_provider(&ProviderType::Ollama));
    assert!(router.available_models().is_empty());
    assert!(
        router
            .discovery_status(&ProviderType::Ollama)
            .and_then(|s| s.error)
            .is_some(),
        "the provider's status says why, so an empty list is not read as 'none installed'"
    );
}

// ── L3: a Claude-pinned template still runs on an Ollama-only machine ───────

/// The shipped templates all pin Claude ids. On a machine with only Ollama the
/// ladder ends at "the default model of the first enabled provider" — and for
/// Ollama that is `default_model` when installed, else the first installed
/// tools-capable model.
#[tokio::test]
async fn a_claude_pin_falls_through_to_an_installed_ollama_model() {
    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/api/tags" => MockResponse::json(tags_body(&["a-no-tools", "b-with-tools"])),
        "/api/show" if req.body.contains("b-with-tools") => {
            MockResponse::json(show_body("qwen3", 262_144, &["completion", "tools"]))
        }
        "/api/show" => MockResponse::json(show_body("gemma", 8192, &["completion"])),
        "/v1/chat/completions" => MockResponse::json(completion_body("b-with-tools", "local pong")),
        _ => MockResponse::not_found(),
    })
    .await;

    let router = empty_catalogue_router(&server.base_url);
    // The configured default is the seeded template's Claude id, and
    // `[providers.ollama] default_model` names a tag nobody pulled.
    router.set_default_model("claude-sonnet-4-6".to_string());
    router.refresh_models_for(&ProviderType::Ollama).await;

    assert_eq!(
        router.effective_default_model().as_deref(),
        Some("b-with-tools"),
        "the tools-capable installed model wins over the alphabetically first"
    );

    let response = router
        .complete(request("claude-sonnet-4-6"))
        .await
        .expect("a Claude pin still runs when only Ollama is enabled");
    assert_eq!(response.content, "local pong");
}

/// Ollama enabled but nothing pulled: one error that names the fix.
#[tokio::test]
async fn an_ollama_with_no_models_installed_says_what_to_do() {
    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/api/tags" => MockResponse::json(tags_body(&[])),
        _ => MockResponse::not_found(),
    })
    .await;

    let router = empty_catalogue_router(&server.base_url);
    router.refresh_models_for(&ProviderType::Ollama).await;

    assert_eq!(router.effective_default_model(), None);
    let err = router
        .complete(request("claude-sonnet-4-6"))
        .await
        .expect_err("nothing is installed");
    assert!(matches!(err, LlmRouterError::NoRoutableModel), "{err:?}");
    assert!(err.to_string().contains("ollama pull"), "{err}");
}

// ── L8: a local turn is billed at 0, end to end ─────────────────────────────

/// The whole path: discovery registers the model, the router calls it, and the
/// cost recorded against the turn is zero — not the $3/$15 Sonnet rate the
/// tracker's private registry used to fall back to.
#[tokio::test]
async fn a_discovered_local_model_is_recorded_at_zero_cost() {
    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/api/tags" => MockResponse::json(tags_body(&["free-model"])),
        "/api/show" => MockResponse::json(show_body("qwen3", 262_144, &["completion", "tools"])),
        "/v1/chat/completions" => MockResponse::json(completion_body("free-model", "cheap")),
        _ => MockResponse::not_found(),
    })
    .await;

    let router = empty_catalogue_router(&server.base_url);
    router.refresh_models_for(&ProviderType::Ollama).await;

    let mut req = request("free-model");
    req.context.agent_id = Some("agent1".to_string());
    router.complete(req).await.expect("the local model answers");

    let usage = router
        .cost_tracker
        .get_agent_usage("agent1")
        .await
        .expect("the turn was recorded");
    assert_eq!(usage.total_requests, 1);
    assert_eq!(usage.total_input_tokens, 11, "real tokens, not zero");
    assert_eq!(usage.total_cost_usd, 0.0, "a local model is free");
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

// ── M2: a small budget on a thinking model ──────────────────────────────────

/// The live failure, reproduced: `POST /v1/chat/completions` with
/// `max_tokens: 24` against a thinking model answers `content: ""`,
/// `reasoning: "…"`, `finish_reason: "length"` and `completion_tokens == 24`.
/// The caller then parsed `""` as JSON and reported "malformed JSON … EOF at
/// column 0", which names neither the cause nor the fix.
fn budget_exhausted_body(model: &str, cap: u32) -> String {
    serde_json::json!({
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "",
                "reasoning": "The user wants me to extract JSON traits from the statement. Let"
            },
            "finish_reason": "length"
        }],
        "usage": {"prompt_tokens": 24, "completion_tokens": cap}
    })
    .to_string()
}

/// M2, the request half: `ThinkingConfig::Disabled` must reach Ollama's wire as
/// `reasoning_effort = "none"` — the one value that actually stops the thinking
/// (verified live: 2 completion tokens against 25 for "low").
#[tokio::test]
async fn a_call_that_wants_no_reasoning_says_so_on_the_wire() {
    let server = MockHttpServer::start(|req| {
        if req.path == "/v1/chat/completions" {
            MockResponse::json(completion_body("local-model", "{\"traits\":[]}"))
        } else {
            MockResponse::not_found()
        }
    })
    .await;

    let router = keyless_router(&server.base_url, "local-model");
    let mut req = request("local-model");
    req.max_tokens = Some(256);
    req.thinking = Some(ThinkingConfig::Disabled);

    router.complete(req).await.expect("the local model answers");

    let seen = server.requests().await;
    assert_eq!(seen.len(), 1);
    let body: serde_json::Value = serde_json::from_str(&seen[0].body).expect("a JSON body");
    assert_eq!(
        body["reasoning_effort"], "none",
        "the utility call asked for no reasoning: {}",
        seen[0].body
    );
    assert_eq!(body["max_tokens"], 256);
}

/// M2, the honest-error half: an answer with nothing in it is reported as the
/// budget problem it is, naming the model and the tokens spent — not handed
/// back as an empty string for the caller to misdiagnose.
#[tokio::test]
async fn an_empty_completion_is_reported_as_a_spent_budget() {
    let server = MockHttpServer::start(|req| {
        if req.path == "/v1/chat/completions" {
            MockResponse::json(budget_exhausted_body("local-model", 24))
        } else {
            MockResponse::not_found()
        }
    })
    .await;

    let router = keyless_router(&server.base_url, "local-model");
    let mut req = request("local-model");
    req.max_tokens = Some(24);

    let err = router
        .complete(req)
        .await
        .expect_err("an empty answer is a failure, not an answer");

    match err {
        LlmRouterError::Llm(crate::error::LlmError::EmptyCompletion {
            ref model,
            output_tokens,
        }) => {
            assert_eq!(model, "local-model");
            assert_eq!(output_tokens, 24);
            let text = err.to_string();
            assert!(text.contains("max_tokens"), "the fix is named: {text}");
            assert!(text.contains("reasoning"), "the cause is named: {text}");
        }
        other => panic!("expected an empty-completion error, got {other:?}"),
    }

    // The tokens were really spent, so they are still booked.
    let usage = router
        .cost_tracker
        .get_agent_usage("unknown")
        .await
        .expect("the spent turn was recorded");
    assert_eq!(usage.total_output_tokens, 24);
}

/// The narrowness matters: a model that answers briefly, or stops at the cap
/// with text in hand, or calls a tool and says nothing, is not an empty
/// completion. Only "nothing at all" is.
#[tokio::test]
async fn a_terse_answer_is_not_an_empty_completion() {
    let server = MockHttpServer::start(|req| {
        if req.path == "/v1/chat/completions" {
            MockResponse::json(
                serde_json::json!({
                    "model": "local-model",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "ok"},
                        "finish_reason": "length"
                    }],
                    "usage": {"prompt_tokens": 9, "completion_tokens": 2}
                })
                .to_string(),
            )
        } else {
            MockResponse::not_found()
        }
    })
    .await;

    let router = keyless_router(&server.base_url, "local-model");
    let response = router
        .complete(request("local-model"))
        .await
        .expect("a short answer is still an answer");
    assert_eq!(response.content, "ok");
}

/// A turn that emits no prose but does call a tool is the normal shape of a
/// tool-using round, and must never be turned into an error.
#[tokio::test]
async fn a_tool_call_with_no_prose_is_not_an_empty_completion() {
    let server = MockHttpServer::start(|req| {
        if req.path == "/v1/chat/completions" {
            MockResponse::json(
                serde_json::json!({
                    "model": "local-model",
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning": "I should look this up.",
                            "tool_calls": [{
                                "id": "call_1",
                                "function": {"name": "search", "arguments": "{\"q\":\"rust\"}"}
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }],
                    "usage": {"prompt_tokens": 30, "completion_tokens": 12}
                })
                .to_string(),
            )
        } else {
            MockResponse::not_found()
        }
    })
    .await;

    let router = keyless_router(&server.base_url, "local-model");
    let response = router
        .complete(request("local-model"))
        .await
        .expect("a tool-calling round is not empty");
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].name, "search");
    assert_eq!(
        response.thinking.as_deref(),
        Some("I should look this up."),
        "M1: the local model's reasoning is kept, not dropped"
    );
}
