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
