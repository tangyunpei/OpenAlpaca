//! MODELS-LATEST: what a request to each of the latest models is sent as,
//! and what a stock install with one provider switched on runs on (the
//! subagent design's §4.9, the owner's models directive of 2026-11-14 and
//! his agreement, "I agree on the new models").
//!
//! The pages, read 2026-11-14: Anthropic's `.../build-with-claude/thinking`
//! ("Sampling parameters", "Response prefill and forced tool use"),
//! `.../thinking-troubleshooting` (the per-model table) and the Haiku 5.5
//! migration guide; OpenAI's `.../guides/latest-model` ("When reasoning
//! effort is not `none`, remove `temperature`, `top_p`, and
//! `top_logprobs`") and the deprecations page. Every provider here is
//! answered by a loopback [`MockHttpServer`]; no test reaches one, and no
//! CLI backend is registered.

use super::*;
use crate::config::{ModelConfigEntry, build_router};
use crate::keys::key_pool::ApiKey;
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::openai::OpenAiProvider;
use crate::test_support::{MockHttpServer, MockResponse, loopback_client};
use std::collections::HashSet;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);
const TEMPLATE: &str = include_str!("../../../../../scripts/release/templates/config/llm.toml");

/// A whole Messages API answer from `model`: one text block.
fn anthropic_answer(model: &str) -> String {
    serde_json::json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [{"type": "text", "text": "ok"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 3, "output_tokens": 1},
    })
    .to_string()
}

/// A whole Chat Completions answer from `model`.
fn chat_answer(model: &str) -> String {
    serde_json::json!({
        "model": model,
        "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 3, "completion_tokens": 1}
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

fn only(enabled: ProviderType) -> HashSet<ProviderType> {
    ProviderType::all()
        .iter()
        .filter(|p| **p != enabled)
        .cloned()
        .collect()
}

/// A keyed Anthropic install answered by `server`, its default `default`.
fn anthropic_router(server: &MockHttpServer, default: &str) -> LlmRouter {
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
        ModelRegistry::with_defaults_and_config(&HashMap::new(), &only(ProviderType::Anthropic)),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        default.to_string(),
    )
}

/// A keyed install answered by `server`, standing in for OpenAI's own API,
/// whose compiled rows are routable only there (COMPAT-SERVER): a request
/// with tools takes the Responses API, a tool-less one Chat Completions.
fn openai_router(server: &MockHttpServer, config: &HashMap<String, ModelConfigEntry>) -> LlmRouter {
    let provider = Arc::new(
        OpenAiProvider::with_client(
            loopback_client(TIMEOUT),
            "sk-test-not-a-key".to_string(),
            None,
            Some(format!("{}/v1", server.base_url)),
            None,
        )
        .with_openai_api(true),
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
        ModelRegistry::with_defaults_and_config(config, &only(ProviderType::OpenAI)),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "gpt-6-luna".to_string(),
    )
}

fn send_tool() -> ToolDefinition {
    ToolDefinition {
        name: "send".to_string(),
        description: "Send the reply.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {"text": {"type": "string"}},
        }),
        ..Default::default()
    }
}

/// What one of the four utility calls sends (`working_state.rs`,
/// `episode.rs`, `extractor.rs`, `task_extraction`): `temperature` 0 with
/// thinking disabled; `model` the summary or extraction model, `None` for
/// the router's default.
fn utility_request(model: Option<&str>) -> RouterRequest {
    RouterRequest {
        model: model.map(str::to_string),
        messages: Arc::new(vec![ChatMessage::user("summarise this")]),
        tools: Arc::new(vec![]),
        temperature: Some(0.0),
        max_tokens: Some(512),
        context: RequestContext::default(),
        tool_choice: None,
        tools_token_estimate: None,
        enable_caching: false,
        thinking: Some(ThinkingConfig::Disabled),
        context_management: None,
        fallback_models: vec![],
        ephemeral_system_notice: None,
    }
}

/// The main loop's or a skill's round 0 with its send hint: `send` forced.
fn forced_send_request(model: &str) -> RouterRequest {
    RouterRequest {
        tools: Arc::new(vec![send_tool()]),
        tool_choice: Some(ToolChoice::Tool("send".to_string())),
        temperature: None,
        thinking: None,
        ..utility_request(Some(model))
    }
}

async fn bodies(server: &MockHttpServer, path: &str) -> Vec<serde_json::Value> {
    server
        .requests()
        .await
        .iter()
        .filter(|r| r.path == path)
        .map(|r| serde_json::from_str(&r.body).expect("a JSON body"))
        .collect()
}

fn no_sampling(body: &serde_json::Value, model: &str) {
    for field in ["temperature", "top_p", "top_k"] {
        assert!(
            body.get(field).is_none(),
            "{field} is not sent to {model}: {body}"
        );
    }
}

fn summarized() -> serde_json::Value {
    serde_json::json!({"type": "adaptive", "display": "summarized"})
}

/// §4.9.3: a utility call to Haiku 5.5 goes with no sampling parameter and
/// its `disabled` thinking kept ("`disabled` accepted at effort `high` or
/// below"; the arm sends no effort, so the model's default `medium` holds);
/// to Sonnet 5.5 and Fable 5.2, which refuse `disabled`, the thinking goes
/// adaptive, asked for summarized (ML-5). Red on the parent, which has no
/// row for any of the three.
#[tokio::test]
async fn a_utility_request_meets_each_anthropic_rows_contract() {
    for (model, thinking) in [
        ("claude-haiku-5-5", serde_json::json!({"type": "disabled"})),
        ("claude-sonnet-5-5", summarized()),
        ("claude-fable-5-2", summarized()),
    ] {
        let server =
            MockHttpServer::start(move |_| MockResponse::json(anthropic_answer(model))).await;
        let router = anthropic_router(&server, model);
        router
            .complete(utility_request(Some(model)))
            .await
            .expect("the mock answers");
        let sent = bodies(&server, "/v1/messages").await;
        assert_eq!(sent.len(), 1, "{model}");
        assert_eq!(sent[0]["model"], model);
        no_sampling(&sent[0], model);
        assert_eq!(sent[0]["thinking"], thinking, "{model}: {}", sent[0]);
    }
}

/// ML-6: Haiku 5.5 "accepts a forced `tool_choice`" (its migration guide),
/// so the main loop's forced `send` reaches it as asked; Sonnet 5.5
/// "rejects both with a 400 error", so it goes as `auto`. Red on the
/// parent, which has neither row.
#[tokio::test]
async fn a_forced_send_stays_forced_on_haiku_5_5_and_goes_auto_on_sonnet_5_5() {
    for (model, choice) in [
        (
            "claude-haiku-5-5",
            serde_json::json!({"type": "tool", "name": "send"}),
        ),
        ("claude-sonnet-5-5", serde_json::json!({"type": "auto"})),
    ] {
        let server =
            MockHttpServer::start(move |_| MockResponse::json(anthropic_answer(model))).await;
        let router = anthropic_router(&server, model);
        router
            .complete(forced_send_request(model))
            .await
            .expect("the mock answers");
        let sent = bodies(&server, "/v1/messages").await;
        assert_eq!(sent[0]["tool_choice"], choice, "{model}: {}", sent[0]);
    }
}

/// Item 4 (taken: `no_sampling_parameters` on `gpt-6.1-sol` "and on the
/// other OpenAI rows whose pages say so"): a utility call's `temperature`
/// 0 is not sent to any of the four, which reason at their default effort
/// since the arm sends none for `Disabled`. Red on the parent, which sends
/// it to `gpt-6.1-sol` and has no row for the other three.
#[tokio::test]
async fn a_utility_request_to_an_openai_row_goes_without_temperature() {
    for model in ["gpt-6-luna", "gpt-6.1-terra", "gpt-6.1-sol", "gpt-6-astra"] {
        let server = MockHttpServer::start(move |_| MockResponse::json(chat_answer(model))).await;
        let router = openai_router(&server, &HashMap::new());
        router
            .complete(utility_request(Some(model)))
            .await
            .expect("the mock answers");
        let sent = bodies(&server, "/v1/chat/completions").await;
        assert_eq!(sent.len(), 1, "{model}");
        assert_eq!(sent[0]["model"], model);
        no_sampling(&sent[0], model);
        assert!(
            sent[0].get("reasoning_effort").is_none(),
            "no effort is sent to {model} (ML-10): {}",
            sent[0]
        );
    }
}

/// The seeded `llm.toml` with only `provider` switched on, a key for it,
/// `base_url` at `base` when given, and both CLI backends off (so nothing
/// on the machine is detected).
fn seeded_with(provider: &str, base: Option<&str>) -> String {
    let switch = format!("[providers.{provider}]\nenabled = false\n");
    assert_eq!(TEMPLATE.matches(&switch).count(), 1, "{switch}");
    let mut text = TEMPLATE.replace(
        &switch,
        &format!("[providers.{provider}]\nenabled = true\n"),
    );
    if let Some(base) = base {
        let seeded = match provider {
            "openai" => "base_url = \"https://api.openai.com/v1\"\n",
            _ => "base_url = \"http://localhost:11434/v1\"\n",
        };
        assert_eq!(text.matches(seeded).count(), 1, "{seeded}");
        text = text.replace(seeded, &format!("base_url = \"{base}/v1\"\n"));
    }
    if provider != "ollama" {
        text.push_str(&format!(
            "\n[[providers.{provider}.keys]]\nid = \"test\"\nsecret_encrypted = \"sk-test-not-a-key\"\n"
        ));
    }
    text.push_str(
        "\n[cli_backends.claude_code]\nenabled = false\n\n[cli_backends.codex]\nenabled = false\n",
    );
    text
}

fn router_from(text: &str) -> (tempfile::TempDir, LlmRouter) {
    let dir = tempfile::tempdir().expect("a temporary dir");
    let path = dir.path().join("llm.toml");
    std::fs::write(&path, text).expect("written");
    let router = build_router(&path).expect("the seeded template builds a router");
    (dir, router)
}

/// A stock Anthropic-only install: the seeded `[orchestrator] model` is
/// Haiku 5.5, routable with no discovery, and a utility call naming no
/// model reaches it with no `temperature` and `disabled` kept. Red on the
/// parent, whose seed names `claude-haiku-4-5-20251001`.
#[tokio::test]
async fn a_stock_anthropic_install_runs_on_haiku_5_5_and_its_utility_calls_are_accepted() {
    let (_dir, router) = router_from(&seeded_with("anthropic", None));
    assert_eq!(
        router.effective_default_model().as_deref(),
        Some("claude-haiku-5-5")
    );

    // The arm reads no `base_url`, so the wire is a router like the seeded
    // one, its default the template's, answered by the mock.
    let server =
        MockHttpServer::start(|_| MockResponse::json(anthropic_answer("claude-haiku-5-5"))).await;
    let wire = anthropic_router(&server, &router.default_model());
    wire.complete(utility_request(None))
        .await
        .expect("the mock answers");
    let sent = bodies(&server, "/v1/messages").await;
    assert_eq!(sent[0]["model"], "claude-haiku-5-5");
    no_sampling(&sent[0], "claude-haiku-5-5");
    assert_eq!(sent[0]["thinking"], serde_json::json!({"type": "disabled"}));
}

/// A stock OpenAI-only install: the seeded `[orchestrator] model` is
/// Anthropic's, so the default is the provider's seeded `gpt-6-luna`, priced
/// at its own $0.10 / $0.50, and a utility call naming no model reaches it
/// on Chat Completions with no `temperature` and no effort. Red on the
/// parent, whose seed names `gpt-4o-mini`.
#[tokio::test]
async fn a_stock_openai_install_runs_on_luna_and_its_utility_calls_are_accepted() {
    // The seeded `base_url`, OpenAI's own API: nothing is sent there.
    let (_dir, router) = router_from(&seeded_with("openai", None));
    assert_eq!(
        router.effective_default_model().as_deref(),
        Some("gpt-6-luna")
    );
    // 100,000 tokens each way, below the 272,000 long-prompt threshold.
    let cost = router
        .cost_tracker
        .calculate_cost("gpt-6-luna", 100_000, 100_000);
    assert!((cost - 0.06).abs() < 1e-9, "Luna at $0.10 / $0.50: {cost}");

    // The wire is a router standing in for OpenAI's own API, its default
    // the template's, answered by the mock.
    let server = MockHttpServer::start(|_| MockResponse::json(chat_answer("gpt-6-luna"))).await;
    let wire = openai_router(&server, &HashMap::new());
    assert_eq!(
        wire.effective_default_model(),
        router.effective_default_model()
    );
    wire.complete(utility_request(None))
        .await
        .expect("the mock answers");
    let sent = bodies(&server, "/v1/chat/completions").await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["model"], "gpt-6-luna");
    no_sampling(&sent[0], "gpt-6-luna");
    assert!(sent[0].get("reasoning_effort").is_none(), "{}", sent[0]);
}

/// A stock Ollama-only install (characterization, green on both): the
/// default is the installed model discovery found, and a utility call
/// reaches it as asked — `temperature` 0 and `reasoning_effort: "none"`
/// (M2) — since no compiled row, and so no contract, is Ollama's.
#[tokio::test]
async fn a_stock_ollama_install_runs_on_its_installed_model_as_before() {
    let server = MockHttpServer::start(|req| match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/tags") => {
            MockResponse::json(serde_json::json!({"models": [{"name": "qwen3:8b"}]}).to_string())
        }
        ("POST", "/api/show") => MockResponse::json(
            serde_json::json!({
                "capabilities": ["completion", "tools"],
                "model_info": {"qwen3.context_length": 40960}
            })
            .to_string(),
        ),
        ("POST", "/v1/chat/completions") => MockResponse::json(chat_answer("qwen3:8b")),
        _ => MockResponse::not_found(),
    })
    .await;
    let (_dir, router) = router_from(&seeded_with("ollama", Some(&server.base_url)));
    let status = router.refresh_models_for(&ProviderType::Ollama).await;
    assert_eq!(status.error, None);
    assert_eq!(
        router.effective_default_model().as_deref(),
        Some("qwen3:8b")
    );

    router
        .complete(utility_request(None))
        .await
        .expect("the mock answers");
    let sent = bodies(&server, "/v1/chat/completions").await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["model"], "qwen3:8b");
    assert_eq!(sent[0]["temperature"], 0.0);
    assert_eq!(sent[0]["reasoning_effort"], "none");
}
