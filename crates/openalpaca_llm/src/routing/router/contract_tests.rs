//! C7a: a request meets the request contract of the model it names.
//!
//! Anthropic's pages, read 2026-11-12: a request to `claude-opus-5-5` must
//! "Omit `temperature`, `top_p`, and `top_k`", sends no `thinking` of type
//! `disabled` or `enabled` (both "rejected"), and uses `tool_choice` `auto`
//! or `none` — "Forcing a tool call with `{"type": "any"}` or
//! `{"type": "tool", "name": "..."}` is rejected"
//! (platform.claude.com/docs/en/models/opus-5-5/migration-guide). The arm's
//! API version, `2023-06-01`, is one of the "Earlier API versions" that
//! return a 400 for a sampling parameter "set to a non-default value on
//! Claude 4.7 and later models" (.../about-claude/model-deprecations).
//! Every provider here is answered by a loopback [`MockHttpServer`].

use super::*;
use crate::config::ModelConfigEntry;
use crate::keys::key_pool::ApiKey;
use crate::providers::anthropic::AnthropicProvider;
use crate::test_support::{MockHttpServer, MockResponse, loopback_client};
use std::collections::HashSet;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

/// A whole Messages API answer from `model`: one text block.
fn answer(model: &str) -> String {
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

/// The same answer as Server-Sent Events.
fn answer_frames(model: &str) -> Vec<String> {
    let frame = |event: &str, data: serde_json::Value| format!("event: {event}\ndata: {data}\n\n");
    vec![
        frame(
            "message_start",
            serde_json::json!({"type": "message_start", "message": {
                "id": "msg_1", "type": "message", "role": "assistant", "content": [],
                "model": model, "usage": {"input_tokens": 3}}}),
        ),
        frame(
            "content_block_start",
            serde_json::json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "text", "text": ""}}),
        ),
        frame(
            "content_block_delta",
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": "ok"}}),
        ),
        frame(
            "message_delta",
            serde_json::json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"},
                "usage": {"output_tokens": 1}}),
        ),
        frame("message_stop", serde_json::json!({"type": "message_stop"})),
    ]
}

/// A keyed Anthropic install whose provider is the loopback `server`, with
/// the compiled Anthropic rows and any `[models]` rows in `config`.
fn anthropic_router(
    server: &MockHttpServer,
    config: &HashMap<String, ModelConfigEntry>,
) -> LlmRouter {
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
            key_pool: Arc::new(ArcSwap::from_pointee(KeyPool::new(
                vec![ApiKey::new(
                    "test".to_string(),
                    ProviderType::Anthropic,
                    "sk-test-not-a-key".to_string(),
                )],
                SelectionStrategy::RoundRobin,
            ))),
        },
    );
    let disabled: HashSet<ProviderType> = ProviderType::all()
        .iter()
        .filter(|p| **p != ProviderType::Anthropic)
        .cloned()
        .collect();
    LlmRouter::new(
        providers,
        ModelRegistry::with_defaults_and_config(config, &disabled),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "claude-sonnet-4-6".to_string(),
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
/// thinking disabled.
fn utility_request(model: &str) -> RouterRequest {
    RouterRequest {
        model: Some(model.to_string()),
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
        ..utility_request(model)
    }
}

async fn bodies(server: &MockHttpServer) -> Vec<serde_json::Value> {
    server
        .requests()
        .await
        .iter()
        .filter(|r| r.path == "/v1/messages")
        .map(|r| serde_json::from_str(&r.body).expect("a JSON body"))
        .collect()
}

/// A utility call to `claude-opus-5-5` (temperature 0, thinking disabled)
/// goes with no sampling parameter and no `thinking` field at all.
#[tokio::test]
async fn a_utility_request_to_opus_5_5_goes_without_sampling_parameters_or_thinking() {
    let server = MockHttpServer::start(|_| MockResponse::json(answer("claude-opus-5-5"))).await;
    let router = anthropic_router(&server, &HashMap::new());

    router
        .complete(utility_request("claude-opus-5-5"))
        .await
        .expect("the mock answers");

    let sent = bodies(&server).await;
    assert_eq!(sent.len(), 1);
    let body = &sent[0];
    assert_eq!(body["model"], "claude-opus-5-5");
    for field in ["temperature", "top_p", "top_k", "thinking"] {
        assert!(
            body.get(field).is_none(),
            "{field} is not sent to claude-opus-5-5: {body}"
        );
    }
}

/// A model whose row carries no contract gets the request its caller asked
/// for (characterization: the parent's request, unchanged).
#[tokio::test]
async fn a_utility_request_to_a_model_without_a_contract_is_sent_as_asked() {
    for model in ["claude-opus-4-6", "claude-sonnet-4-6"] {
        let server = MockHttpServer::start(move |_| MockResponse::json(answer(model))).await;
        let router = anthropic_router(&server, &HashMap::new());

        router
            .complete(utility_request(model))
            .await
            .expect("the mock answers");

        let body = &bodies(&server).await[0];
        assert_eq!(body["model"], model);
        assert_eq!(
            body["temperature"],
            serde_json::json!(0.0),
            "{model}: {body}"
        );
        assert_eq!(
            body["thinking"],
            serde_json::json!({"type": "disabled"}),
            "{model}: {body}"
        );
    }
}

/// A round-0 forced `send` to `claude-opus-5-5` goes as `auto`, on the
/// streamed path the main loop and a skill take (`completion.rs`) and on the
/// non-streaming one (`retry.rs`), and `daemon.log` says so once for the
/// model across both.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_forced_send_to_opus_5_5_goes_as_auto_and_is_said_once() {
    let server = MockHttpServer::start(|req| {
        if req.body.contains("\"stream\":true") {
            MockResponse::sse(answer_frames("claude-opus-5-5")).every(Duration::ZERO)
        } else {
            MockResponse::json(answer("claude-opus-5-5"))
        }
    })
    .await;
    let router = anthropic_router(&server, &HashMap::new());

    let routed = router
        .complete_streaming(forced_send_request("claude-opus-5-5"))
        .await
        .expect("the stream opens");
    crate::collect_stream(routed.stream, routed.model)
        .await
        .expect("the stream completes");
    router
        .complete(forced_send_request("claude-opus-5-5"))
        .await
        .expect("the mock answers");

    let sent = bodies(&server).await;
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0]["stream"], true, "the first went streamed");
    assert!(sent[1].get("stream").is_none(), "the second did not");
    for body in &sent {
        assert_eq!(
            body["tool_choice"],
            serde_json::json!({"type": "auto"}),
            "{body}"
        );
    }
    logs_assert(|lines: &[&str]| {
        let said = lines
            .iter()
            .filter(|l| l.contains("refuses a forced tool choice") && l.contains("claude-opus-5-5"))
            .count();
        if said == 1 {
            Ok(())
        } else {
            Err(format!("said {said} times, not once"))
        }
    });
}

/// A model without the contract is still forced (characterization).
#[tokio::test]
async fn a_forced_send_to_a_model_without_a_contract_stays_forced() {
    for model in ["claude-opus-4-6", "claude-sonnet-4-6"] {
        let server = MockHttpServer::start(move |_| MockResponse::json(answer(model))).await;
        let router = anthropic_router(&server, &HashMap::new());

        router
            .complete(forced_send_request(model))
            .await
            .expect("the mock answers");

        let body = &bodies(&server).await[0];
        assert_eq!(body["model"], model);
        assert_eq!(
            body["tool_choice"],
            serde_json::json!({"type": "tool", "name": "send"}),
            "{model}: {body}"
        );
    }
}

/// A manual thinking budget (`Enabled`) to `claude-opus-5-5`, which takes
/// adaptive thinking only, goes as adaptive thinking — `enabled` is
/// "rejected" (the migration guide, read 2026-11-12). No shipped caller
/// sends a budget today; the contract still holds a request that would.
#[tokio::test]
async fn a_thinking_budget_to_opus_5_5_goes_as_adaptive_thinking() {
    let server = MockHttpServer::start(|_| MockResponse::json(answer("claude-opus-5-5"))).await;
    let router = anthropic_router(&server, &HashMap::new());

    router
        .complete(RouterRequest {
            temperature: None,
            max_tokens: Some(4096),
            thinking: Some(ThinkingConfig::Enabled {
                budget_tokens: 2048,
            }),
            ..utility_request("claude-opus-5-5")
        })
        .await
        .expect("the mock answers");

    let body = &bodies(&server).await[0];
    assert_eq!(
        body["thinking"],
        serde_json::json!({"type": "adaptive"}),
        "{body}"
    );
}

/// A `[models."claude-opus-5-5"]` row a person writes for its price keeps
/// the model's contract: the request still goes without `temperature`.
#[tokio::test]
async fn a_models_row_for_opus_5_5_keeps_its_contract() {
    let server = MockHttpServer::start(|_| MockResponse::json(answer("claude-opus-5-5"))).await;
    let mut config = HashMap::new();
    config.insert(
        "claude-opus-5-5".to_string(),
        ModelConfigEntry {
            provider: "anthropic".to_string(),
            input_price: Some(4.0),
            output_price: Some(20.0),
            context: Some(1_000_000),
            supports_image: Some(true),
            supports_audio: None,
            supports_document: Some(true),
            supports_reasoning: None,
            supports_tools: Some(true),
        },
    );
    let router = anthropic_router(&server, &config);

    router
        .complete(utility_request("claude-opus-5-5"))
        .await
        .expect("the mock answers");

    let body = &bodies(&server).await[0];
    assert!(body.get("temperature").is_none(), "{body}");
    assert!(body.get("thinking").is_none(), "{body}");
}

// ── C7a: an Anthropic turn of thinking alone, cut at `max_tokens` ───────────

/// The blocks of an Anthropic answer cut at `max_tokens` while it was
/// still thinking: one thinking block with its signature, no text, no
/// `tool_use`.
fn thinking_alone_blocks() -> serde_json::Value {
    serde_json::json!([
        {"type": "thinking", "thinking": "Plan the search.", "signature": "EqQBsig-A=="},
    ])
}

/// A lead's or subagent's round: a tool offered, no forced choice.
fn tool_round_request(model: &str) -> RouterRequest {
    RouterRequest {
        tools: Arc::new(vec![send_tool()]),
        temperature: None,
        max_tokens: None,
        thinking: None,
        ..utility_request(model)
    }
}

/// An Anthropic answer cut at `max_tokens` before any text or call, its
/// thinking alone, as the arm's 4096 default can end a `claude-opus-5-5`
/// round. No change made; what this pins: asked without streaming, as
/// every lead and subagent round is, it is the router's `EmptyCompletion`
/// (`crate::error::empty_completion_error`, called in `retry.rs`), not
/// transient, asked once, so nothing retries it and the loop ends the run
/// on it; streamed, the same answer collects as a `MaxTokens` turn with no
/// text whose kept thinking `ChatMessage::assistant_answer` carries into
/// the loop's continuation (the Responses wire's twin is
/// `pricing_tests::a_responses_turn_cut_before_any_text_ends_unless_streamed`).
#[tokio::test]
async fn an_anthropic_turn_of_thinking_alone_cut_at_max_tokens_ends_unless_streamed() {
    let whole = serde_json::json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-5-5",
        "content": thinking_alone_blocks(),
        "stop_reason": "max_tokens",
        "usage": {"input_tokens": 1_000, "output_tokens": 4_096},
    });
    let frame = |event: &str, data: serde_json::Value| format!("event: {event}\ndata: {data}\n\n");
    let frames = vec![
        frame(
            "message_start",
            serde_json::json!({"type": "message_start", "message": {
                "id": "msg_1", "type": "message", "role": "assistant", "content": [],
                "model": "claude-opus-5-5", "usage": {"input_tokens": 1_000}}}),
        ),
        frame(
            "content_block_start",
            serde_json::json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
        ),
        frame(
            "content_block_delta",
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "Plan the search."}}),
        ),
        frame(
            "content_block_delta",
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "signature_delta", "signature": "EqQBsig-A=="}}),
        ),
        frame(
            "content_block_stop",
            serde_json::json!({"type": "content_block_stop", "index": 0}),
        ),
        frame(
            "message_delta",
            serde_json::json!({"type": "message_delta", "delta": {"stop_reason": "max_tokens"},
                "usage": {"output_tokens": 4_096}}),
        ),
        frame("message_stop", serde_json::json!({"type": "message_stop"})),
    ];
    let server = MockHttpServer::start(move |req| {
        let asked: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
        match (req.path.as_str(), asked["stream"].as_bool()) {
            ("/v1/messages", Some(true)) => MockResponse::sse(frames.clone()),
            ("/v1/messages", _) => MockResponse::json(whole.to_string()),
            _ => MockResponse::not_found(),
        }
    })
    .await;
    let router = anthropic_router(&server, &HashMap::new());

    let refused = router
        .complete(tool_round_request("claude-opus-5-5"))
        .await
        .expect_err("an answer with nothing in it is not an answer");
    match &refused {
        LlmRouterError::Llm(
            inner @ LlmError::EmptyCompletion {
                model,
                output_tokens,
            },
        ) => {
            assert_eq!(model, "claude-opus-5-5");
            assert_eq!(*output_tokens, 4_096);
            assert!(!inner.is_transient());
        }
        other => panic!("expected EmptyCompletion, got {other:?}"),
    }
    assert_eq!(server.hits("/v1/messages").await, 1);

    let routed = router
        .complete_streaming(tool_round_request("claude-opus-5-5"))
        .await
        .expect("streams");
    let streamed = crate::collect_stream(routed.stream, routed.model.clone())
        .await
        .expect("a cut stream still collects");
    assert_eq!(streamed.finish_reason, FinishReason::MaxTokens);
    assert_eq!(streamed.content, "");
    assert!(streamed.tool_calls.is_empty());
    let kept = streamed.echo.as_ref().expect("the thinking is kept");
    assert_eq!(
        serde_json::Value::Array(kept.blocks().to_vec()),
        thinking_alone_blocks()
    );
    let continued = ChatMessage::assistant_answer(&streamed);
    assert_eq!(continued.echo.as_ref(), Some(kept));
    assert_eq!(server.hits("/v1/messages").await, 2);
}
