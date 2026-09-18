use super::*;

#[test]
fn test_parse_hierarchical_provider_keys() {
    let toml_str = r#"
[orchestrator]
model = "claude-sonnet-4-5-20250929"

[providers.anthropic]
enabled = true

[[providers.anthropic.keys]]
id = "key1"
secret_env = "ANTHROPIC_API_KEY"
"#;
    let config: LlmRouterConfig = toml::from_str(toml_str).unwrap();
    let key = &config.providers.as_ref().unwrap()["anthropic"]
        .keys
        .as_ref()
        .unwrap()[0];
    assert_eq!(key.secret_env.as_deref(), Some("ANTHROPIC_API_KEY"));
}

#[test]
fn test_parse_router_config() {
    let toml_str = r#"
[orchestrator]
model = "claude-sonnet-4-5-20250929"
fallback_models = ["gpt-4o"]

[providers.anthropic]
enabled = true
strategy = "round_robin"

[[providers.anthropic.keys]]
id = "key1"
secret_env = "ANTHROPIC_API_KEY"
tier = "tier1"

[models.custom-model]
provider = "anthropic"
input_price = 5.0
output_price = 25.0
context = 100000

[fallback_chains]
"claude-sonnet-4-5-20250929" = ["gpt-4o"]
"#;
    let config: LlmRouterConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(
        config.orchestrator.as_ref().unwrap().model,
        "claude-sonnet-4-5-20250929"
    );
    assert!(config.providers.as_ref().unwrap().contains_key("anthropic"));
    assert!(config.models.as_ref().unwrap().contains_key("custom-model"));

    let key = &config.providers.as_ref().unwrap()["anthropic"]
        .keys
        .as_ref()
        .unwrap()[0];
    assert_eq!(key.id, "key1");
    assert_eq!(key.tier.as_deref(), Some("tier1"));
}

#[test]
fn test_parse_provider_type_fn() {
    assert_eq!(
        parse_provider_type("anthropic"),
        Some(ProviderType::Anthropic)
    );
    assert_eq!(parse_provider_type("openai"), Some(ProviderType::OpenAI));
    assert_eq!(parse_provider_type("ollama"), Some(ProviderType::Ollama));
    assert_eq!(parse_provider_type("unknown"), None);
}

// ── R58(b): a disabled provider contributes no models ───────────────────────

/// A provider the owner turned off must not be in the catalogue after a
/// restart either — neither its `[models]` rows nor its compiled defaults.
/// Otherwise `GET /v1/models` re-lists it, the picker offers it, and the call
/// fails with `ProviderNotConfigured` (or worse, reaches the provider's CLI
/// backend).
#[test]
fn a_disabled_provider_contributes_no_models_at_boot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("llm.toml");
    std::fs::write(
        &path,
        r#"[orchestrator]
model = "claude-haiku-4-5-20251001"

[providers.openai]
enabled = false

[models."gpt-hand-written"]
provider = "openai"
context = 128000
"#,
    )
    .unwrap();

    let router = build_router(&path).expect("router");
    let registry = router.model_registry();

    assert_eq!(
        registry.resolve_provider("gpt-hand-written"),
        None,
        "the disabled provider's own [models] row must stay out"
    );
    assert_eq!(
        registry.resolve_provider("gpt-5.2"),
        None,
        "and so must its compiled defaults"
    );
    assert!(
        registry.resolve_provider("claude-haiku-4-5-20251001").is_some(),
        "the enabled providers are untouched"
    );
}

// ── L4: the seeded template ────────────────────────────────────────────────

/// The file a fresh install parses before anything else. Its `[providers.ollama]`
/// section is the whole local-model story: turn it on and nothing else is
/// needed — no key, no `[models]` row, no hand-picked tag.
#[test]
fn the_seeded_template_describes_a_discoverable_ollama() {
    const TEMPLATE: &str =
        include_str!("../../../../../scripts/release/templates/config/llm.toml");

    let config: LlmRouterConfig = toml::from_str(TEMPLATE).expect("the seeded template parses");
    let ollama = &config.providers.as_ref().expect("providers")["ollama"];

    assert_eq!(
        ollama.enabled,
        Some(false),
        "enabling stays the owner's one action (auto-enable is owner decision T20, not adopted)"
    );
    assert_eq!(
        ollama.default_model.as_deref(),
        Some(""),
        "empty means 'whatever is installed', not a tag nobody pulled"
    );
    assert_eq!(ollama.default_max_tokens, Some(8192));
    assert_eq!(ollama.request_timeout_secs, Some(600));
    assert!(
        config.models.is_none(),
        "discovery writes the catalogue; the owner writes no [models] rows"
    );
    assert!(
        ollama.keys.is_none(),
        "and no key, placeholder or otherwise"
    );

    let runtime = LlmRuntimeConfig::from(&config);
    assert_eq!(
        runtime.request_timeout_for("ollama"),
        std::time::Duration::from_secs(600),
        "the local provider's own budget"
    );
    assert_eq!(
        runtime.request_timeout_for("anthropic"),
        std::time::Duration::from_secs(120),
        "and the cloud default is untouched"
    );
}

// ── L7: the request timeout is configuration, not a constant ───────────────

/// The default is the number the HTTP client used to be hard-coded with, so a
/// file that says nothing behaves exactly as it did.
#[test]
fn the_request_timeout_defaults_to_two_minutes() {
    let config: LlmRouterConfig = toml::from_str("[orchestrator]\nmodel = \"m\"\n").unwrap();
    let runtime = LlmRuntimeConfig::from(&config);

    assert_eq!(runtime.timeouts.llm_request_timeout_secs, 120);
    assert_eq!(
        runtime.request_timeout_for("anthropic"),
        std::time::Duration::from_secs(120)
    );
}

/// `[timeouts]` moves every provider; `[providers.<name>] request_timeout_secs`
/// moves one — a local model that generates for ten minutes without the cloud
/// providers waiting that long for a dead socket.
#[test]
fn a_per_provider_timeout_outranks_the_global_one() {
    let config: LlmRouterConfig = toml::from_str(
        r#"[orchestrator]
model = "m"

[timeouts]
llm_request_timeout_secs = 45

[providers.ollama]
enabled = true
request_timeout_secs = 600
"#,
    )
    .unwrap();
    let runtime = LlmRuntimeConfig::from(&config);

    assert_eq!(
        runtime.request_timeout_for("ollama"),
        std::time::Duration::from_secs(600),
        "the provider's own value wins"
    );
    assert_eq!(
        runtime.request_timeout_for("openai"),
        std::time::Duration::from_secs(45),
        "and everyone else takes the [timeouts] value"
    );
}

/// A nonsense value is clamped into the range and said out loud, never taken
/// as meant: zero seconds would fail every call before it started.
#[test]
fn an_out_of_range_timeout_is_clamped() {
    let config: LlmRouterConfig = toml::from_str(
        r#"[orchestrator]
model = "m"

[providers.ollama]
request_timeout_secs = 0

[providers.openai]
request_timeout_secs = 999999999
"#,
    )
    .unwrap();
    let runtime = LlmRuntimeConfig::from(&config);

    assert_eq!(
        runtime.request_timeout_for("ollama"),
        std::time::Duration::from_secs(1)
    );
    assert_eq!(
        runtime.request_timeout_for("openai"),
        std::time::Duration::from_secs(86_400)
    );
}

// ── The boot builder's local-provider arm (L6, L7) ──────────────────────────
//
// Compiled only where the provider features are: `cargo test -p openalpaca_llm`
// builds the crate with none of them, and these tests drive a real
// `OllamaProvider` against a loopback mock server. `cargo test --workspace`
// runs them, because `openalpacad` asks for all three features.
#[cfg(all(feature = "ollama", feature = "openai"))]
mod local_provider_boot {
    use crate::routing::router::{RequestContext, RouterRequest};
    use crate::test_support::{MockHttpServer, MockResponse};
    use crate::types::ChatMessage;
    use std::sync::Arc;

    /// A config whose only provider is a keyless Ollama at `base_url`, with one
    /// declared model so the request is routable without discovery.
    fn config_with(base_url: &str, provider_lines: &str) -> String {
        format!(
            r#"[orchestrator]
model = "local-model"

[providers.ollama]
enabled = true
base_url = "{base_url}/v1"
{provider_lines}

[models."local-model"]
provider = "ollama"
input_price = 0.0
output_price = 0.0
context = 262144
"#
        )
    }

    fn write_config(dir: &std::path::Path, contents: &str) -> std::path::PathBuf {
        let path = dir.join("llm.toml");
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn request(max_tokens: Option<u32>) -> RouterRequest {
        RouterRequest {
            model: Some("local-model".to_string()),
            messages: Arc::new(vec![ChatMessage::user("ping")]),
            tools: Arc::new(vec![]),
            temperature: None,
            max_tokens,
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

    fn completion_body() -> String {
        serde_json::json!({
            "model": "local-model",
            "choices": [{"message": {"role": "assistant", "content": "pong"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 11, "completion_tokens": 3}
        })
        .to_string()
    }

    /// L6: the Ollama arm read `default_model` and `base_url` and dropped
    /// `default_max_tokens`, so every local answer was cut at the OpenAI
    /// provider's hard-coded 4096 whatever the file said.
    #[tokio::test]
    async fn the_configured_output_ceiling_reaches_the_request_body() {
        let server =
            MockHttpServer::start(|_| MockResponse::json(completion_body())).await;
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            &config_with(&server.base_url, "default_max_tokens = 8192"),
        );

        let router = super::build_router(&path).expect("router");
        router.complete(request(None)).await.expect("a local answer");

        let seen = server.requests().await;
        let body: serde_json::Value = serde_json::from_str(&seen[0].body).unwrap();
        assert_eq!(
            body["max_tokens"],
            serde_json::json!(8192),
            "[providers.ollama] default_max_tokens is what goes on the wire: {}",
            seen[0].body
        );

        // And the request still outranks the provider default.
        router
            .complete(request(Some(64)))
            .await
            .expect("a local answer");
        let seen = server.requests().await;
        let body: serde_json::Value = serde_json::from_str(&seen[1].body).unwrap();
        assert_eq!(body["max_tokens"], serde_json::json!(64), "{}", seen[1].body);
    }

    /// L7, first half: the total is the file's, and it is real. Before the fix
    /// every call carried the same hard-coded 120 s, so a provider told to give
    /// up after a second waited two minutes — and this call simply succeeded.
    #[tokio::test]
    async fn a_non_streaming_call_is_cut_at_the_configured_timeout() {
        let server = MockHttpServer::start(|_| {
            MockResponse::json(completion_body()).after(std::time::Duration::from_secs(5))
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            &config_with(&server.base_url, "request_timeout_secs = 1"),
        );

        let router = super::build_router(&path).expect("router");
        let result = router.complete(request(None)).await;

        assert!(
            result.is_err(),
            "a provider slower than its own budget is given up on"
        );
        assert!(
            server.hits("/v1/chat/completions").await >= 1,
            "and the failure is the deadline, not a routing miss"
        );
    }

    /// L7, second half: that same total must not reach the streaming path. A
    /// stream whose frames keep coming is healthy however long it runs — the
    /// wall clock belongs to the loop's `max_stream_duration`, not to reqwest.
    #[tokio::test]
    async fn a_healthy_stream_outlives_the_non_streaming_timeout() {
        let server = MockHttpServer::start(|_| {
            MockResponse::sse(vec![
                format!(
                    "data: {}\n\n",
                    serde_json::json!({"choices": [{"index": 0, "delta": {"content": "tick "}}]})
                ),
                format!(
                    "data: {}\n\n",
                    serde_json::json!({"choices": [{"index": 0, "delta": {"content": "tock "}}]})
                ),
                format!(
                    "data: {}\n\n",
                    serde_json::json!({"choices": [{"index": 0, "delta": {"content": "done"}}]})
                ),
                format!(
                    "data: {}\n\n",
                    serde_json::json!({
                        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 11, "completion_tokens": 3}
                    })
                ),
                "data: [DONE]\n\n".to_string(),
            ])
            .every(std::time::Duration::from_millis(400))
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            &config_with(&server.base_url, "request_timeout_secs = 1"),
        );

        let router = super::build_router(&path).expect("router");
        let stream = router
            .complete_streaming(request(None))
            .await
            .expect("the stream starts");
        let collected = crate::streaming::collect_stream(stream, "local-model".to_string())
            .await
            .expect("a stream that keeps producing is never cut");

        assert_eq!(collected.content, "tick tock done");
        assert_eq!(collected.usage.output_tokens, 3);
    }
}
