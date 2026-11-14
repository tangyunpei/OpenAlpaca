//! What a model that discovery registered is counted at (O-18, taken as (b)
//! with (c)), and how much of Anthropic's model list discovery reads.
//!
//! Before the fix, keyed discovery registered every id Anthropic or OpenAI
//! listed that the catalogue lacked at 0 / 0, and the cost tracker counted it
//! at that row's price: on an OpenAI-only install the seeded default,
//! `gpt-4o-mini`, ran at $0 and no cost cap (N4) could trip. Every provider
//! here is answered by a loopback [`MockHttpServer`]; no test reaches one.

use super::*;
use crate::config::ModelConfigEntry;
use crate::keys::key_pool::ApiKey;
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::ollama::OllamaProvider;
use crate::providers::openai::OpenAiProvider;
use crate::test_support::{MockHttpServer, MockResponse, loopback_client};
use std::collections::HashSet;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);
/// A million tokens each way: the cost is then the two prices' sum.
const MILLION: u32 = 1_000_000;
/// `CostTracker::pricing_for`'s fallback for an id it holds no price for.
const FALLBACK: f64 = 3.0 + 15.0;
/// `gpt-4o-mini` at the rate its own page prints: $0.15 in, $0.60 out.
const GPT_4O_MINI: f64 = 0.15 + 0.60;

fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-9
}

/// OpenAI's `GET /models` listing `ids`.
fn models_body(ids: &[&str]) -> String {
    serde_json::json!({
        "object": "list",
        "data": ids
            .iter()
            .map(|id| serde_json::json!({"id": id, "object": "model", "owned_by": "openai"}))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

fn keyed_pool(provider: ProviderType) -> Arc<ArcSwap<KeyPool>> {
    Arc::new(ArcSwap::from_pointee(KeyPool::new(
        vec![ApiKey::new(
            "test".to_string(),
            provider,
            "sk-test-not-a-key".to_string(),
        )],
        SelectionStrategy::RoundRobin,
    )))
}

/// The registry an install with only `enabled` switched on builds: the
/// compiled defaults of that provider, no `[models]` row unless `config`.
fn registry_for(
    enabled: ProviderType,
    config: &HashMap<String, ModelConfigEntry>,
) -> ModelRegistry {
    let disabled: HashSet<ProviderType> = ProviderType::all()
        .iter()
        .filter(|p| **p != enabled)
        .cloned()
        .collect();
    ModelRegistry::with_defaults_and_config(config, &disabled)
}

/// A keyed OpenAI install, the provider at `base` (`…/v1`), as the seeded
/// `llm.toml` sets it up: `gpt-4o-mini` the default.
fn openai_router(base: &str, config: &HashMap<String, ModelConfigEntry>) -> LlmRouter {
    let provider = Arc::new(OpenAiProvider::with_client(
        loopback_client(TIMEOUT),
        "sk-test-not-a-key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(format!("{base}/v1")),
        None,
    ));
    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::OpenAI,
        ProviderEntry {
            provider,
            key_pool: keyed_pool(ProviderType::OpenAI),
        },
    );
    LlmRouter::new(
        providers,
        registry_for(ProviderType::OpenAI, config),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "gpt-4o-mini".to_string(),
    )
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

/// The warnings that name `id` as priced at the fallback.
fn unpriced_announcements(lines: &[&str], id: &str) -> usize {
    lines
        .iter()
        .filter(|l| l.contains("no price") && l.contains(id))
        .count()
}

/// The seeded OpenAI-only install: its default is discovered and, with the
/// catalogue row (c), counted at its own rate — no longer at $0.
#[tokio::test]
async fn the_seeded_openai_default_is_counted_at_its_own_rate() {
    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/v1/models" => MockResponse::json(models_body(&["gpt-4o-mini", "gpt-5-mini"])),
        _ => MockResponse::not_found(),
    })
    .await;
    let router = openai_router(&server.base_url, &HashMap::new());

    let status = router.refresh_models_for(&ProviderType::OpenAI).await;
    assert_eq!(status.error, None);
    assert!(router.is_routable("gpt-4o-mini"));

    let cost = router
        .cost_tracker
        .calculate_cost("gpt-4o-mini", MILLION, MILLION);
    assert!(
        close(cost, GPT_4O_MINI),
        "gpt-4o-mini is counted at $0.15 / $0.60, not {cost}"
    );
    let info = router
        .model_registry()
        .get_model_info("gpt-4o-mini")
        .expect("a catalogue row");
    assert_eq!(info.context_window, 128_000);
    assert!(info.supports_tools && info.supports_image && !info.supports_audio);
}

/// (b): an id discovery registered that no catalogue row and no `[models]`
/// row prices stays routable, is counted at the fallback, and is announced
/// once however often it is counted.
#[tokio::test]
#[tracing_test::traced_test]
async fn an_unpriced_discovered_openai_id_is_counted_at_the_fallback_and_announced_once() {
    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/v1/models" => MockResponse::json(models_body(&["gpt-unpriced-test"])),
        _ => MockResponse::not_found(),
    })
    .await;
    let router = openai_router(&server.base_url, &HashMap::new());
    router.refresh_models_for(&ProviderType::OpenAI).await;

    assert!(
        router.is_routable("gpt-unpriced-test"),
        "no new refusal: the id stays routable"
    );
    for _ in 0..3 {
        let cost = router
            .cost_tracker
            .calculate_cost("gpt-unpriced-test", MILLION, MILLION);
        assert!(close(cost, FALLBACK), "counted at $3 / $15, not {cost}");
    }
    logs_assert(
        |lines| match unpriced_announcements(lines, "gpt-unpriced-test") {
            1 => Ok(()),
            n => Err(format!("announced once, not {n} times")),
        },
    );
}

/// Design item (4): a non-streaming OpenAI answer is counted by the model its
/// body names — a dated snapshot of the id asked for. A snapshot discovery
/// registered with no price meets the same rule.
#[tokio::test]
async fn a_non_streaming_answer_naming_a_discovered_snapshot_is_counted_at_the_fallback() {
    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/v1/models" => MockResponse::json(models_body(&["gpt-4o-mini", "gpt-4o-mini-2024-07-18"])),
        "/v1/chat/completions" => MockResponse::json(
            serde_json::json!({
                "model": "gpt-4o-mini-2024-07-18",
                "choices": [{
                    "message": {"role": "assistant", "content": "pong"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": MILLION, "completion_tokens": MILLION}
            })
            .to_string(),
        ),
        _ => MockResponse::not_found(),
    })
    .await;
    let router = openai_router(&server.base_url, &HashMap::new());
    router.refresh_models_for(&ProviderType::OpenAI).await;

    let mut req = request("gpt-4o-mini");
    req.context.agent_id = Some("agent1".to_string());
    let response = router.complete(req).await.expect("the mock answers");
    assert_eq!(response.model, "gpt-4o-mini-2024-07-18");

    let usage = router
        .cost_tracker
        .get_agent_usage("agent1")
        .await
        .expect("the call was recorded");
    assert!(
        close(usage.total_cost_usd, FALLBACK),
        "the snapshot is counted at $3 / $15, not {}",
        usage.total_cost_usd
    );
}

/// A `[models]` row at $0 is the owner's price, discovered or not: it is never
/// repriced and never announced.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_declared_zero_price_row_is_not_repriced() {
    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/v1/models" => MockResponse::json(models_body(&["gpt-free-test"])),
        _ => MockResponse::not_found(),
    })
    .await;
    let config = HashMap::from([(
        "gpt-free-test".to_string(),
        ModelConfigEntry {
            provider: "openai".to_string(),
            input_price: Some(0.0),
            output_price: Some(0.0),
            context: Some(32_000),
            supports_image: None,
            supports_audio: None,
            supports_document: None,
            supports_reasoning: None,
            supports_tools: None,
        },
    )]);
    let router = openai_router(&server.base_url, &config);
    router.refresh_models_for(&ProviderType::OpenAI).await;

    assert_eq!(
        router
            .cost_tracker
            .calculate_cost("gpt-free-test", MILLION, MILLION),
        0.0
    );
    assert!(!logs_contain("no price"));
}

/// A provider that needs no key keeps price 0 for what it discovers — on a
/// registry that also holds cloud rows, so the 0 is not the local-only rule's.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_discovered_ollama_model_still_costs_nothing() {
    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/api/tags" => MockResponse::json(r#"{"models":[{"name":"local-test:8b"}]}"#),
        "/api/show" => MockResponse::json(
            r#"{"capabilities":["completion","tools"],"model_info":{"qwen3.context_length":32768}}"#,
        ),
        _ => MockResponse::not_found(),
    })
    .await;
    let provider = Arc::new(OllamaProvider::new(
        "local-test:8b".to_string(),
        Some(format!("{}/v1", server.base_url)),
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
    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "local-test:8b".to_string(),
    );
    router.refresh_models_for(&ProviderType::Ollama).await;

    assert!(!router.model_registry().is_local_only());
    assert!(router.is_routable("local-test:8b"));
    assert_eq!(
        router
            .cost_tracker
            .calculate_cost("local-test:8b", MILLION, MILLION),
        0.0
    );
    assert!(!logs_contain("no price"));
}

/// One page of Anthropic's model list.
fn anthropic_page(ids: &[&str], has_more: bool) -> String {
    serde_json::json!({
        "data": ids
            .iter()
            .map(|id| serde_json::json!({"id": id, "type": "model", "display_name": id}))
            .collect::<Vec<_>>(),
        "first_id": ids.first(),
        "has_more": has_more,
        "last_id": ids.last(),
    })
    .to_string()
}

/// Discovery reads every page of Anthropic's model list, not only the first:
/// the second page's ids are registered too, each asked for after the first
/// page's `last_id`.
#[tokio::test]
async fn anthropic_discovery_reads_every_page_of_the_model_list() {
    let server = MockHttpServer::start(|req| {
        if !req.path.starts_with("/v1/models") {
            return MockResponse::not_found();
        }
        if req.path.contains("after_id=claude-page-one-b") {
            MockResponse::json(anthropic_page(&["claude-page-two-a"], false))
        } else if req.path.contains("after_id=") {
            MockResponse::not_found()
        } else {
            MockResponse::json(anthropic_page(
                &["claude-page-one-a", "claude-page-one-b"],
                true,
            ))
        }
    })
    .await;
    let provider = Arc::new(
        AnthropicProvider::with_client(
            loopback_client(TIMEOUT),
            "sk-test-not-a-key".to_string(),
            None,
            None,
        )
        .with_api_base(format!("{}/v1", server.base_url)),
    );
    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Anthropic,
        ProviderEntry {
            provider,
            key_pool: keyed_pool(ProviderType::Anthropic),
        },
    );
    let router = LlmRouter::new(
        providers,
        registry_for(ProviderType::Anthropic, &HashMap::new()),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "claude-sonnet-4-6".to_string(),
    );

    let status = router.refresh_models_for(&ProviderType::Anthropic).await;

    assert_eq!(status.error, None);
    assert_eq!(status.models, 3, "both pages are read");
    let discovered: HashSet<String> = router
        .available_models()
        .into_iter()
        .map(|m| m.id)
        .collect();
    for id in [
        "claude-page-one-a",
        "claude-page-one-b",
        "claude-page-two-a",
    ] {
        assert!(
            discovered.contains(id),
            "{id} is registered: {discovered:?}"
        );
    }
    assert_eq!(server.requests().await.len(), 2, "one request per page");
}

/// A later page that does not decode ends the walk with the ids already
/// read: the first page's ids are still registered. An error status and a
/// failure in transit are tested below.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_later_page_that_does_not_decode_keeps_the_ids_already_read() {
    let server = MockHttpServer::start(|req| {
        if !req.path.starts_with("/v1/models") {
            MockResponse::not_found()
        } else if req.path.contains("after_id=") {
            MockResponse::json("{not json")
        } else {
            MockResponse::json(anthropic_page(
                &["claude-page-one-a", "claude-page-one-b"],
                true,
            ))
        }
    })
    .await;
    let provider = Arc::new(
        AnthropicProvider::with_client(
            loopback_client(TIMEOUT),
            "sk-test-not-a-key".to_string(),
            None,
            None,
        )
        .with_api_base(format!("{}/v1", server.base_url)),
    );
    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Anthropic,
        ProviderEntry {
            provider,
            key_pool: keyed_pool(ProviderType::Anthropic),
        },
    );
    let router = LlmRouter::new(
        providers,
        registry_for(ProviderType::Anthropic, &HashMap::new()),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "claude-sonnet-4-6".to_string(),
    );

    let status = router.refresh_models_for(&ProviderType::Anthropic).await;

    assert_eq!(status.error, None, "the first page's ids are kept");
    assert_eq!(status.models, 2);
    let discovered: HashSet<String> = router
        .available_models()
        .into_iter()
        .map(|m| m.id)
        .collect();
    for id in ["claude-page-one-a", "claude-page-one-b"] {
        assert!(
            discovered.contains(id),
            "{id} is registered: {discovered:?}"
        );
    }
    assert_eq!(server.requests().await.len(), 2, "one request per page");
    assert!(logs_contain("A later page of the model list failed"));
}

/// A router over one keyed Anthropic provider at `server`, as the two tests
/// above build it.
fn anthropic_router(server: &MockHttpServer) -> LlmRouter {
    let provider = Arc::new(
        AnthropicProvider::with_client(
            loopback_client(TIMEOUT),
            "sk-test-not-a-key".to_string(),
            None,
            None,
        )
        .with_api_base(format!("{}/v1", server.base_url)),
    );
    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Anthropic,
        ProviderEntry {
            provider,
            key_pool: keyed_pool(ProviderType::Anthropic),
        },
    );
    LlmRouter::new(
        providers,
        registry_for(ProviderType::Anthropic, &HashMap::new()),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "claude-sonnet-4-6".to_string(),
    )
}

/// A list that answers every page with the same page — `has_more` set and
/// the same `last_id` — is read twice and no more: the walk has no page cap,
/// so the `last_id` that repeats the cursor just sent is what ends it. The
/// mock stops saying `has_more` after a few requests, so a walk that ignores
/// the repeat fails this test rather than hanging it.
#[tokio::test]
async fn a_last_id_that_repeats_the_cursor_ends_the_walk() {
    const BOUND: usize = 6;
    let served = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&served);
    let server = MockHttpServer::start(move |req| {
        if !req.path.starts_with("/v1/models") {
            return MockResponse::not_found();
        }
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        MockResponse::json(anthropic_page(
            &["claude-stuck-a", "claude-stuck-b"],
            n < BOUND,
        ))
    })
    .await;
    let router = anthropic_router(&server);

    let status = router.refresh_models_for(&ProviderType::Anthropic).await;

    assert_eq!(status.error, None);
    assert_eq!(
        server.requests().await.len(),
        2,
        "the first page, then the one whose last_id repeats the cursor"
    );
    let discovered: HashSet<String> = router
        .available_models()
        .into_iter()
        .map(|m| m.id)
        .collect();
    for id in ["claude-stuck-a", "claude-stuck-b"] {
        assert!(
            discovered.contains(id),
            "{id} is registered: {discovered:?}"
        );
    }
}

/// A later page answered with an error status, or lost in transit, ends the
/// walk with the ids already read, as one that does not decode does (above):
/// the first page's ids are registered, discovery reports no error, and the
/// failure is one WARN.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_later_page_with_an_error_status_or_lost_in_transit_keeps_the_ids_already_read() {
    for (label, later) in [
        ("500", MockResponse::error(500, "{}")),
        ("404", MockResponse::not_found()),
        ("hung up", MockResponse::hang_up()),
    ] {
        let server = MockHttpServer::start(move |req| {
            if !req.path.starts_with("/v1/models") {
                MockResponse::not_found()
            } else if req.path.contains("after_id=") {
                later.clone()
            } else {
                MockResponse::json(anthropic_page(
                    &["claude-page-one-a", "claude-page-one-b"],
                    true,
                ))
            }
        })
        .await;
        let router = anthropic_router(&server);

        let status = router.refresh_models_for(&ProviderType::Anthropic).await;

        assert_eq!(status.error, None, "{label}: the first page's ids are kept");
        assert_eq!(status.models, 2, "{label}");
        let discovered: HashSet<String> = router
            .available_models()
            .into_iter()
            .map(|m| m.id)
            .collect();
        for id in ["claude-page-one-a", "claude-page-one-b"] {
            assert!(
                discovered.contains(id),
                "{label}: {id} is registered: {discovered:?}"
            );
        }
        assert!(
            server
                .requests()
                .await
                .iter()
                .any(|r| r.path.contains("after_id=claude-page-one-b")),
            "{label}: the later page was asked for"
        );
    }
    logs_assert(|lines: &[&str]| {
        let warned = lines
            .iter()
            .filter(|l| l.contains("A later page of the model list failed"))
            .count();
        if warned == 3 {
            Ok(())
        } else {
            Err(format!("one WARN per case, 3 in all; found {warned}"))
        }
    });
}

/// M-3c: the two flagships discovery already lists on a keyed install keep
/// their catalogue rows when it lists them again — price and window — so a
/// run on either is counted at its page's rate. Red on the parent, where
/// discovery registered both with no price (counted at the $3 / $15
/// fallback since O-18's fix) and no window.
#[tokio::test]
async fn a_discovered_flagship_keeps_its_catalogue_price() {
    let server = MockHttpServer::start(|req| {
        if req.path.starts_with("/v1/models") {
            MockResponse::json(anthropic_page(&["claude-opus-5-5"], false))
        } else {
            MockResponse::not_found()
        }
    })
    .await;
    let router = anthropic_router(&server);
    let status = router.refresh_models_for(&ProviderType::Anthropic).await;
    assert_eq!(status.error, None);
    assert!(router.is_routable("claude-opus-5-5"));
    let cost = router
        .cost_tracker
        .calculate_cost("claude-opus-5-5", MILLION, MILLION);
    assert!(
        close(cost, 4.0 + 20.0),
        "claude-opus-5-5 is counted at $4 / $20, not {cost}"
    );
    let info = router
        .model_registry()
        .get_model_info("claude-opus-5-5")
        .expect("registered");
    assert!(info.discovered, "discovery marked it");
    assert_eq!(info.context_window, 1_000_000);

    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/v1/models" => MockResponse::json(models_body(&["gpt-6.1-sol"])),
        _ => MockResponse::not_found(),
    })
    .await;
    let router = openai_router(&server.base_url, &HashMap::new());
    let status = router.refresh_models_for(&ProviderType::OpenAI).await;
    assert_eq!(status.error, None);
    // A prompt within its standard rate's 272K input tokens.
    let cost = router
        .cost_tracker
        .calculate_cost("gpt-6.1-sol", 100_000, MILLION);
    assert!(
        close(cost, 0.2 + 10.0),
        "gpt-6.1-sol is counted at $2 / $10, not {cost}"
    );
    let info = router
        .model_registry()
        .get_model_info("gpt-6.1-sol")
        .expect("registered");
    assert_eq!(info.context_window, 1_050_000);
}

// ── C7b: which id prices a Responses API call ───────────────────────────────

/// A keyed OpenAI install whose provider stands in for OpenAI's own API at
/// `base`, so a request with tools goes through the Responses API (C7b).
fn responses_router(base: &str) -> LlmRouter {
    let provider = Arc::new(
        OpenAiProvider::with_client(
            loopback_client(TIMEOUT),
            "sk-test-not-a-key".to_string(),
            Some("gpt-4o-mini".to_string()),
            Some(format!("{base}/v1")),
            None,
        )
        .with_responses_api(true),
    );
    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::OpenAI,
        ProviderEntry {
            provider,
            key_pool: keyed_pool(ProviderType::OpenAI),
        },
    );
    LlmRouter::new(
        providers,
        registry_for(ProviderType::OpenAI, &HashMap::new()),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "gpt-4o-mini".to_string(),
    )
}

/// A request for `model` that carries a tool, so it goes to `/responses`.
fn tool_request(model: &str) -> RouterRequest {
    RouterRequest {
        tools: Arc::new(vec![ToolDefinition {
            name: "search".to_string(),
            description: "search".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            strict: None,
            input_examples: None,
        }]),
        ..request(model)
    }
}

/// A whole Responses answer naming `model`, with these token counts.
fn responses_answer(model: &str, input: u32, output: u32) -> String {
    serde_json::json!({
        "id": "resp_1",
        "object": "response",
        "model": model,
        "status": "completed",
        "output": [{
            "type": "message", "id": "msg_1", "role": "assistant", "status": "completed",
            "content": [{"type": "output_text", "text": "pong", "annotations": []}]
        }],
        "usage": {"input_tokens": input, "output_tokens": output,
                  "input_tokens_details": {"cached_tokens": 0},
                  "output_tokens_details": {"reasoning_tokens": 0}}
    })
    .to_string()
}

/// C7b's pin, no change made: a non-streaming Responses answer is counted by
/// the model its body names, as a Chat Completions one is (design item (4)).
/// `gpt-6.1-sol`'s page (read 2026-11-12) lists one snapshot, the id
/// itself, so its answer is counted at its row; `gpt-5.2`'s names the
/// default snapshot `gpt-5.2-2025-12-11`, which no row prices, so an answer
/// naming it is counted at $3 / $15 (pending-decisions §1.14, *Rev 42*).
#[tokio::test]
async fn a_responses_answer_is_counted_by_the_model_its_body_names() {
    let server = MockHttpServer::start(|req| {
        let asked: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
        match (req.path.as_str(), asked["model"].as_str()) {
            ("/v1/responses", Some("gpt-6.1-sol")) => {
                MockResponse::json(responses_answer("gpt-6.1-sol", 100_000, 100_000))
            }
            ("/v1/responses", Some("gpt-5.2")) => {
                MockResponse::json(responses_answer("gpt-5.2-2025-12-11", MILLION, MILLION))
            }
            _ => MockResponse::not_found(),
        }
    })
    .await;
    let router = responses_router(&server.base_url);

    let mut sol = tool_request("gpt-6.1-sol");
    sol.context.agent_id = Some("sol".to_string());
    let answered = router.complete(sol).await.expect("the mock answers");
    assert_eq!(answered.model, "gpt-6.1-sol");
    let usage = router
        .cost_tracker
        .get_agent_usage("sol")
        .await
        .expect("recorded");
    // 100,000 × $2 + 100,000 × $10 per million, under the long-prompt line.
    assert!(
        close(usage.total_cost_usd, 0.2 + 1.0),
        "{}",
        usage.total_cost_usd
    );

    let mut previous = tool_request("gpt-5.2");
    previous.context.agent_id = Some("snapshot".to_string());
    let answered = router.complete(previous).await.expect("the mock answers");
    assert_eq!(answered.model, "gpt-5.2-2025-12-11");
    let usage = router
        .cost_tracker
        .get_agent_usage("snapshot")
        .await
        .expect("recorded");
    assert!(
        close(usage.total_cost_usd, FALLBACK),
        "the snapshot is counted at $3 / $15, not {}",
        usage.total_cost_usd
    );
    assert_eq!(server.hits("/v1/chat/completions").await, 0);
}

/// The streamed site, no change made. What this pins is the router's half:
/// `RoutedStream.model`, the id the router routed the round to, is the id
/// asked for, whatever model the completion event names. The cost itself is
/// recorded by core's loop, which collects the stream under that id and
/// prices the round by it (`runner/agentic_loop/backend.rs`), outside this
/// crate.
#[tokio::test]
async fn a_streamed_responses_round_is_routed_under_the_asked_id() {
    let completed = serde_json::json!({
        "type": "response.completed",
        "response": serde_json::from_str::<serde_json::Value>(
            &responses_answer("gpt-5.2-2025-12-11", 1_000, 1_000)
        ).unwrap()
    });
    let frame = format!("event: response.completed\ndata: {completed}\n\n");
    let server = MockHttpServer::start(move |req| match req.path.as_str() {
        "/v1/responses" => MockResponse::sse(vec![frame.clone()]),
        _ => MockResponse::not_found(),
    })
    .await;
    let router = responses_router(&server.base_url);

    let routed = router
        .complete_streaming(tool_request("gpt-5.2"))
        .await
        .expect("streams");
    assert_eq!(routed.model, "gpt-5.2");
    let response = crate::collect_stream(routed.stream, routed.model.clone())
        .await
        .expect("collects");
    assert_eq!(response.content, "pong");
    assert_eq!(server.hits("/v1/responses").await, 1);
}

// ── C7b: a Responses turn cut before any text ───────────────────────────────

/// A Responses answer cut at `max_output_tokens` before any text or call,
/// a reasoning item alone, as the arm's 4096 default can end a
/// `gpt-6.1-sol` turn. No change made; what this pins (pending-decisions
/// §1, *Rev 63*): asked without streaming, as every lead and subagent
/// round is, it is the router's `EmptyCompletion`, not transient, asked
/// once, so nothing retries it and the loop ends the run on it; streamed,
/// the same answer collects as a `MaxTokens` turn, which the loop
/// continues.
#[tokio::test]
async fn a_responses_turn_cut_before_any_text_ends_unless_streamed() {
    let cut = serde_json::json!({
        "id": "resp_1",
        "object": "response",
        "model": "gpt-6.1-sol",
        "status": "incomplete",
        "incomplete_details": {"reason": "max_output_tokens"},
        "output": [{
            "type": "reasoning", "id": "rs_1", "summary": [],
            "encrypted_content": "gAAAA-encrypted"
        }],
        "usage": {"input_tokens": 1_000, "output_tokens": 4_096,
                  "input_tokens_details": {"cached_tokens": 0},
                  "output_tokens_details": {"reasoning_tokens": 4_096}}
    });
    let frame = format!(
        "event: response.incomplete\ndata: {}\n\n",
        serde_json::json!({"type": "response.incomplete", "response": cut})
    );
    let server = MockHttpServer::start(move |req| {
        let asked: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
        match (req.path.as_str(), asked["stream"].as_bool()) {
            ("/v1/responses", Some(true)) => MockResponse::sse(vec![frame.clone()]),
            ("/v1/responses", _) => MockResponse::json(cut.to_string()),
            _ => MockResponse::not_found(),
        }
    })
    .await;
    let router = responses_router(&server.base_url);

    let refused = router
        .complete(tool_request("gpt-6.1-sol"))
        .await
        .expect_err("an answer with nothing in it is not an answer");
    match &refused {
        LlmRouterError::Llm(
            inner @ crate::error::LlmError::EmptyCompletion {
                model,
                output_tokens,
            },
        ) => {
            assert_eq!(model, "gpt-6.1-sol");
            assert_eq!(*output_tokens, 4_096);
            assert!(!inner.is_transient());
        }
        other => panic!("expected EmptyCompletion, got {other:?}"),
    }
    assert_eq!(server.hits("/v1/responses").await, 1);

    let routed = router
        .complete_streaming(tool_request("gpt-6.1-sol"))
        .await
        .expect("streams");
    let streamed = crate::collect_stream(routed.stream, routed.model.clone())
        .await
        .expect("a cut stream still collects");
    assert_eq!(streamed.finish_reason, FinishReason::MaxTokens);
    assert_eq!(streamed.content, "");
    assert!(streamed.tool_calls.is_empty());
    assert_eq!(server.hits("/v1/responses").await, 2);
}
