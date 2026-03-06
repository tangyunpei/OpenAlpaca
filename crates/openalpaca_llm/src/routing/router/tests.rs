use super::*;
use crate::keys::key_pool::{ApiKey, SelectionStrategy};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

struct MockProvider {
    name_str: String,
    responses: Vec<Result<ChatResponse, LlmError>>,
    call_count: AtomicUsize,
}

impl MockProvider {
    fn new(name: &str, responses: Vec<Result<ChatResponse, LlmError>>) -> Self {
        Self {
            name_str: name.to_string(),
            responses,
            call_count: AtomicUsize::new(0),
        }
    }

    fn ok_response(model: &str) -> ChatResponse {
        ChatResponse {
            content: format!("Response from {}", model),
            tool_calls: vec![],
            model: model.to_string(),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            },
            finish_reason: FinishReason::Stop,
            thinking: None,
            parts: None,
        }
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    fn name(&self) -> &str {
        &self.name_str
    }
    fn supports_tools(&self) -> bool {
        false
    }
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        if idx < self.responses.len() {
            self.responses[idx].clone()
        } else {
            self.responses
                .last()
                .cloned()
                .unwrap_or(Err(LlmError::NotConfigured))
        }
    }
    async fn chat_with_key(
        &self,
        _key: &str,
        request: ChatRequest,
    ) -> Result<ChatResponse, LlmError> {
        self.chat(request).await
    }
}

fn make_request(model: Option<&str>) -> RouterRequest {
    RouterRequest {
        model: model.map(|m| m.to_string()),
        messages: Arc::new(vec![ChatMessage::user("test")]),
        tools: Arc::new(vec![]),
        temperature: None,
        max_tokens: None,
        context: RequestContext::default(),
        tool_choice: None,
        tools_token_estimate: None,
        enable_caching: false,
        thinking: None,
    }
}

fn make_router_with_mock(
    provider: Arc<dyn LlmProvider>,
    provider_type: ProviderType,
    model: &str,
) -> LlmRouter {
    LlmRouter::single_provider(provider, provider_type, model.to_string())
}

#[tokio::test]
async fn test_routes_to_correct_provider() {
    let provider = Arc::new(MockProvider::new(
        "anthropic",
        vec![Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929"))],
    ));
    let router = make_router_with_mock(
        provider,
        ProviderType::Anthropic,
        "claude-sonnet-4-5-20250929",
    );

    let result = router.complete(make_request(None)).await;
    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response.model, "claude-sonnet-4-5-20250929");
}

#[tokio::test]
async fn test_key_rotation_on_rate_limit() {
    let provider = Arc::new(MockProvider::new(
        "anthropic",
        vec![
            Err(LlmError::RateLimited {
                retry_after_ms: 1000,
            }),
            Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929")),
        ],
    ));

    let key_pool = KeyPool::new(
        vec![
            ApiKey::new(
                "k1".to_string(),
                ProviderType::Anthropic,
                "sk-1".to_string(),
            ),
            ApiKey::new(
                "k2".to_string(),
                ProviderType::Anthropic,
                "sk-2".to_string(),
            ),
        ],
        SelectionStrategy::RoundRobin,
    );

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Anthropic,
        ProviderEntry {
            provider: provider,
            key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
        },
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "claude-sonnet-4-5-20250929".to_string(),
    );

    let result = router.complete(make_request(None)).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_overloaded_error_does_not_cooldown_key() {
    let provider = Arc::new(MockProvider::new(
        "anthropic",
        vec![
            Err(LlmError::Overloaded {
                status: 529,
                retry_after_ms: Some(5),
            }),
            Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929")),
        ],
    ));

    let key_pool = KeyPool::new(
        vec![ApiKey::new(
            "k1".to_string(),
            ProviderType::Anthropic,
            "sk-1".to_string(),
        )],
        SelectionStrategy::RoundRobin,
    );

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Anthropic,
        ProviderEntry {
            provider: provider,
            key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
        },
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "claude-sonnet-4-5-20250929".to_string(),
    );

    let result = router.complete(make_request(None)).await;
    assert!(result.is_ok());

    let statuses = router
        .key_statuses(ProviderType::Anthropic)
        .await
        .expect("anthropic provider key statuses should exist");
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].consecutive_rate_limits, 0);
    assert!(statuses[0].is_available);
}

#[tokio::test]
async fn test_openai_overloaded_error_does_not_cooldown_key() {
    let provider = Arc::new(MockProvider::new(
        "openai",
        vec![
            Err(LlmError::Overloaded {
                status: 503,
                retry_after_ms: Some(10),
            }),
            Ok(MockProvider::ok_response("gpt-5.2")),
        ],
    ));

    let key_pool = KeyPool::new(
        vec![ApiKey::new(
            "k1".to_string(),
            ProviderType::OpenAI,
            "sk-1".to_string(),
        )],
        SelectionStrategy::RoundRobin,
    );

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::OpenAI,
        ProviderEntry {
            provider: provider,
            key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
        },
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "gpt-5.2".to_string(),
    );

    let result = router.complete(make_request(Some("gpt-5.2"))).await;
    assert!(result.is_ok());

    let statuses = router
        .key_statuses(ProviderType::OpenAI)
        .await
        .expect("openai provider key statuses should exist");
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].consecutive_rate_limits, 0);
    assert!(statuses[0].is_available);
}

#[tokio::test]
async fn test_fallback_chain() {
    // Anthropic provider always rate-limits
    let anthropic = Arc::new(MockProvider::new(
        "anthropic",
        vec![Err(LlmError::RateLimited {
            retry_after_ms: 1000,
        })],
    ));
    let openai = Arc::new(MockProvider::new(
        "openai",
        vec![Ok(MockProvider::ok_response("gpt-5.2"))],
    ));

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Anthropic,
        ProviderEntry {
            provider: anthropic,
            key_pool: Arc::new(ArcSwap::from_pointee(KeyPool::new(
                vec![ApiKey::new(
                    "k1".to_string(),
                    ProviderType::Anthropic,
                    "sk-1".to_string(),
                )],
                SelectionStrategy::RoundRobin,
            ))),
        },
    );
    providers.insert(
        ProviderType::OpenAI,
        ProviderEntry {
            provider: openai,
            key_pool: Arc::new(ArcSwap::from_pointee(KeyPool::new(
                vec![ApiKey::new(
                    "k1".to_string(),
                    ProviderType::OpenAI,
                    "sk-1".to_string(),
                )],
                SelectionStrategy::RoundRobin,
            ))),
        },
    );

    let mut fallback_chains = HashMap::new();
    fallback_chains.insert(
        "claude-sonnet-4-5-20250929".to_string(),
        vec!["gpt-5.2".to_string()],
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        fallback_chains,
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "claude-sonnet-4-5-20250929".to_string(),
    );

    let result = router.complete(make_request(None)).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().model, "gpt-5.2");
}

#[tokio::test]
async fn test_cost_recording() {
    let provider = Arc::new(MockProvider::new(
        "anthropic",
        vec![Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929"))],
    ));
    let router = make_router_with_mock(
        provider,
        ProviderType::Anthropic,
        "claude-sonnet-4-5-20250929",
    );

    let mut req = make_request(None);
    req.context.agent_id = Some("agent1".to_string());

    router.complete(req).await.unwrap();

    let usage = router.cost_tracker.get_agent_usage("agent1").await;
    assert!(usage.is_some());
    assert_eq!(usage.unwrap().total_requests, 1);
}

#[tokio::test]
async fn test_unknown_model_error() {
    let provider = Arc::new(MockProvider::new("anthropic", vec![]));
    let router = make_router_with_mock(
        provider,
        ProviderType::Anthropic,
        "claude-sonnet-4-5-20250929",
    );

    let result = router
        .complete(make_request(Some("nonexistent-model-xyz")))
        .await;
    assert!(matches!(result, Err(LlmRouterError::UnknownModel(_))));
}

#[tokio::test]
async fn test_all_failed_no_fallback() {
    let provider = Arc::new(MockProvider::new(
        "anthropic",
        vec![Err(LlmError::RateLimited {
            retry_after_ms: 1000,
        })],
    ));

    let key_pool = KeyPool::new(
        vec![ApiKey::new(
            "k1".to_string(),
            ProviderType::Anthropic,
            "sk-1".to_string(),
        )],
        SelectionStrategy::RoundRobin,
    );

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Anthropic,
        ProviderEntry {
            provider: provider,
            key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
        },
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(), // No fallback chains
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "claude-sonnet-4-5-20250929".to_string(),
    );

    let result = router.complete(make_request(None)).await;
    assert!(matches!(result, Err(LlmRouterError::AllFallbacksFailed)));
}

#[tokio::test]
async fn test_hot_reload_keys() {
    let provider = Arc::new(MockProvider::new(
        "anthropic",
        vec![
            Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929")),
            Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929")),
        ],
    ));

    let key_pool = KeyPool::new(
        vec![ApiKey::new(
            "k1".to_string(),
            ProviderType::Anthropic,
            "sk-1".to_string(),
        )],
        SelectionStrategy::RoundRobin,
    );

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Anthropic,
        ProviderEntry {
            provider: provider,
            key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
        },
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "claude-sonnet-4-5-20250929".to_string(),
    );

    // First call works
    let result = router.complete(make_request(None)).await;
    assert!(result.is_ok());

    // Hot-reload with new key pool
    let new_pool = KeyPool::new(
        vec![
            ApiKey::new(
                "k_new_1".to_string(),
                ProviderType::Anthropic,
                "sk-new-1".to_string(),
            ),
            ApiKey::new(
                "k_new_2".to_string(),
                ProviderType::Anthropic,
                "sk-new-2".to_string(),
            ),
        ],
        SelectionStrategy::RoundRobin,
    );
    assert!(router.reload_keys(ProviderType::Anthropic, new_pool));

    // Second call works with new pool
    let result = router.complete(make_request(None)).await;
    assert!(result.is_ok());

    // Verify reload returns false for unconfigured provider
    let empty_pool = KeyPool::new(vec![], SelectionStrategy::RoundRobin);
    assert!(!router.reload_keys(ProviderType::OpenAI, empty_pool));
}

#[tokio::test]
async fn test_register_provider_new() {
    let provider = Arc::new(MockProvider::new(
        "openai",
        vec![Ok(MockProvider::ok_response("gpt-5.2"))],
    ));
    let pool = KeyPool::new(
        vec![ApiKey::new(
            "k1".to_string(),
            ProviderType::OpenAI,
            "sk-1".to_string(),
        )],
        SelectionStrategy::RoundRobin,
    );

    let router = LlmRouter::new(
        HashMap::new(),
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "gpt-5.2".to_string(),
    );

    assert!(!router.reload_keys(
        ProviderType::OpenAI,
        KeyPool::new(vec![], SelectionStrategy::RoundRobin)
    ));
    assert!(router.register_provider(ProviderType::OpenAI, provider, pool));
    assert!(
        router
            .configured_providers()
            .contains(&ProviderType::OpenAI)
    );
}

#[tokio::test]
async fn test_try_fallback_to_cli() {
    // API provider always rate-limits
    let api_provider = Arc::new(MockProvider::new(
        "anthropic",
        vec![Err(LlmError::RateLimited {
            retry_after_ms: 1000,
        })],
    ));
    let cli_provider = Arc::new(MockProvider::new(
        "claude_cli",
        vec![Ok(MockProvider::ok_response("claude_cli"))],
    ));

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Anthropic,
        ProviderEntry {
            provider: api_provider,
            key_pool: Arc::new(ArcSwap::from_pointee(KeyPool::new(
                vec![ApiKey::new(
                    "k1".to_string(),
                    ProviderType::Anthropic,
                    "sk-1".to_string(),
                )],
                SelectionStrategy::RoundRobin,
            ))),
        },
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(), // No model-level fallback chains
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "claude-sonnet-4-5-20250929".to_string(),
    );

    router.register_cli_backend(ProviderType::Anthropic, cli_provider);

    let result = router.complete(make_request(None)).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().model, "claude_cli");
}

#[tokio::test]
async fn test_cli_fallback_not_called_when_api_succeeds() {
    let api_provider = Arc::new(MockProvider::new(
        "anthropic",
        vec![Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929"))],
    ));
    let cli_provider = Arc::new(MockProvider::new(
        "claude_cli",
        vec![Ok(MockProvider::ok_response("claude_cli"))],
    ));

    let router = LlmRouter::single_provider(
        api_provider,
        ProviderType::Anthropic,
        "claude-sonnet-4-5-20250929".to_string(),
    );
    router.register_cli_backend(ProviderType::Anthropic, cli_provider);

    let result = router.complete(make_request(None)).await;
    assert!(result.is_ok());
    // Should use API, not CLI
    assert_eq!(result.unwrap().model, "claude-sonnet-4-5-20250929");
}

// ── Key-aware mock provider ────────────────────────────────────

struct KeyAwareMockProvider;

#[async_trait]
impl LlmProvider for KeyAwareMockProvider {
    fn name(&self) -> &str {
        "key_aware_mock"
    }
    fn supports_tools(&self) -> bool {
        false
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        Err(LlmError::NotConfigured)
    }
    async fn chat_with_key(&self, key: &str, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        if key.starts_with("sk-ant-oat") {
            Err(LlmError::AuthenticationFailed(
                "managed token cannot auth against HTTP API".into(),
            ))
        } else {
            Ok(MockProvider::ok_response("claude-haiku"))
        }
    }
}

#[tokio::test]
async fn test_no_api_keys_triggers_cli_fallback() {
    // Pool with only a managed key (source=ClaudeCode)
    let mut managed_key = ApiKey::new(
        "managed1".to_string(),
        ProviderType::Anthropic,
        "sk-ant-oat01-managed-token-placeholder-very-long".to_string(),
    );
    managed_key.source = crate::keys::key_pool::KeySource::ClaudeCode;

    let key_pool = KeyPool::new(vec![managed_key], SelectionStrategy::RoundRobin);

    let key_aware = Arc::new(KeyAwareMockProvider);
    let cli_provider = Arc::new(MockProvider::new(
        "claude_cli",
        vec![Ok(MockProvider::ok_response("claude_cli"))],
    ));

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Anthropic,
        ProviderEntry {
            provider: key_aware,
            key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
        },
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "claude-sonnet-4-5-20250929".to_string(),
    );

    router.register_cli_backend(ProviderType::Anthropic, cli_provider);

    // Should fall through to CLI backend since no API-compatible keys
    let result = router.complete(make_request(None)).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().model, "claude_cli");
}

#[tokio::test]
async fn test_mixed_pool_uses_api_key_only() {
    // Pool with managed key + real API key
    let mut managed_key = ApiKey::new(
        "managed1".to_string(),
        ProviderType::Anthropic,
        "sk-ant-oat01-managed-token-placeholder-very-long".to_string(),
    );
    managed_key.source = crate::keys::key_pool::KeySource::ClaudeCode;

    let mut api_key = ApiKey::new(
        "api1".to_string(),
        ProviderType::Anthropic,
        format!("sk-ant-api03-{}", "x".repeat(30)),
    );
    api_key.source = crate::keys::key_pool::KeySource::ApiConsole;

    let key_pool = KeyPool::new(vec![managed_key, api_key], SelectionStrategy::RoundRobin);

    let key_aware = Arc::new(KeyAwareMockProvider);

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Anthropic,
        ProviderEntry {
            provider: key_aware,
            key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
        },
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "claude-sonnet-4-5-20250929".to_string(),
    );

    // Should succeed using the API key, not the managed key
    let result = router.complete(make_request(None)).await;
    assert!(result.is_ok());
    // KeyAwareMockProvider returns "claude-haiku" for valid API keys
    assert_eq!(result.unwrap().model, "claude-haiku");
}

#[test]
fn test_flatten_messages() {
    let messages = vec![
        ChatMessage {
            role: Role::System,
            content: "You are helpful.".to_string(),
            parts: None,
            tool_calls: None,
            tool_call_id: None,
        },
        ChatMessage::user("Hello"),
        ChatMessage {
            role: Role::Assistant,
            content: "Hi!".to_string(),
            parts: None,
            tool_calls: None,
            tool_call_id: None,
        },
    ];
    let flattened = flatten_messages(&messages);
    assert!(flattened.contains("[System] You are helpful."));
    assert!(flattened.contains("Hello"));
    assert!(flattened.contains("[Assistant] Hi!"));
}

#[test]
fn test_truncate_messages_for_cli_small_input() {
    let messages = vec![
        ChatMessage {
            role: Role::System,
            content: "System prompt".to_string(),
            parts: None,
            tool_calls: None,
            tool_call_id: None,
        },
        ChatMessage::user("Hello"),
        ChatMessage {
            role: Role::Assistant,
            content: "Hi!".to_string(),
            parts: None,
            tool_calls: None,
            tool_call_id: None,
        },
    ];
    let result = truncate_messages_for_cli(&messages);
    // Small input — no truncation needed
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].content, "System prompt");
    assert_eq!(result[1].content, "Hello");
    assert_eq!(result[2].content, "Hi!");
}

#[test]
fn test_truncate_messages_for_cli_large_input() {
    let big_content = "x".repeat(8 * 1024); // 8KB per message
    let messages = vec![
        ChatMessage {
            role: Role::System,
            content: "System prompt".to_string(),
            parts: None,
            tool_calls: None,
            tool_call_id: None,
        },
        ChatMessage::user(&big_content), // middle — should be dropped
        ChatMessage {
            role: Role::Assistant,
            content: big_content.clone(),
            parts: None,
            tool_calls: None,
            tool_call_id: None,
        }, // middle — should be dropped
        ChatMessage::user(&big_content), // middle — should be dropped
        ChatMessage {
            role: Role::Tool,
            content: "tool result".to_string(),
            parts: None,
            tool_calls: None,
            tool_call_id: None,
        }, // second-to-last — kept
        ChatMessage::user("What next?"), // last — kept
    ];
    // Total > 16KB, so truncation should fire
    let result = truncate_messages_for_cli(&messages);
    // Should keep: system (1) + omission notice (1) + last 2 (2) = 4
    assert_eq!(result.len(), 4);
    assert_eq!(result[0].content, "System prompt");
    assert!(result[1].content.contains("earlier messages omitted"));
    assert_eq!(result[2].content, "tool result");
    assert_eq!(result[3].content, "What next?");
}

#[test]
fn test_truncate_messages_for_cli_empty() {
    let result = truncate_messages_for_cli(&[]);
    assert!(result.is_empty());
}

// ── estimated_llm_capacity tests ─────────────────────────────────

#[tokio::test]
async fn test_estimated_llm_capacity_single_key() {
    // Single provider, single key, default rate config (per_key_concurrency=2, global=4)
    let provider = Arc::new(MockProvider::new(
        "anthropic",
        vec![Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929"))],
    ));
    let router = make_router_with_mock(
        provider,
        ProviderType::Anthropic,
        "claude-sonnet-4-5-20250929",
    );

    // 1 available key * 5 per-key concurrency = 5, capped by global (10) = 5
    let info = router.estimated_llm_capacity(None).await;
    assert_eq!(info.available_api_keys, 1);
    assert_eq!(info.per_key_concurrency, 5);
    assert_eq!(info.key_capacity, 5);
    assert!(!info.has_cli_fallback);
    assert_eq!(info.effective_capacity, 5);
}

#[tokio::test]
async fn test_estimated_llm_capacity_multiple_keys() {
    let provider = Arc::new(MockProvider::new(
        "anthropic",
        vec![Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929"))],
    ));

    let key_pool = KeyPool::new(
        vec![
            ApiKey::new(
                "k1".to_string(),
                ProviderType::Anthropic,
                "sk-1".to_string(),
            ),
            ApiKey::new(
                "k2".to_string(),
                ProviderType::Anthropic,
                "sk-2".to_string(),
            ),
            ApiKey::new(
                "k3".to_string(),
                ProviderType::Anthropic,
                "sk-3".to_string(),
            ),
        ],
        SelectionStrategy::RoundRobin,
    );

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Anthropic,
        ProviderEntry {
            provider,
            key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
        },
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "claude-sonnet-4-5-20250929".to_string(),
    );

    // 3 keys * 5 per-key = 15, capped by global concurrency (10) = 10
    let info = router.estimated_llm_capacity(None).await;
    assert_eq!(info.available_api_keys, 3);
    assert_eq!(info.key_capacity, 15);
    assert!(!info.has_cli_fallback);
    assert_eq!(info.effective_capacity, 10);
}

#[tokio::test]
async fn test_estimated_llm_capacity_rate_limited_keys() {
    let provider = Arc::new(MockProvider::new(
        "anthropic",
        vec![Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929"))],
    ));

    let key_pool = KeyPool::new(
        vec![
            ApiKey::new(
                "k1".to_string(),
                ProviderType::Anthropic,
                "sk-1".to_string(),
            ),
            ApiKey::new(
                "k2".to_string(),
                ProviderType::Anthropic,
                "sk-2".to_string(),
            ),
        ],
        SelectionStrategy::RoundRobin,
    );

    // Rate-limit one key
    key_pool
        .report_result(
            "k1",
            CallResult::RateLimited {
                retry_after_ms: 60_000,
            },
        )
        .await;

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Anthropic,
        ProviderEntry {
            provider,
            key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
        },
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "claude-sonnet-4-5-20250929".to_string(),
    );

    // 1 available key * 5 per-key = 5, capped by global (10) = 5
    let info = router.estimated_llm_capacity(None).await;
    assert_eq!(info.available_api_keys, 1);
    assert_eq!(info.key_capacity, 5);
    assert_eq!(info.effective_capacity, 5);
}

#[tokio::test]
async fn test_estimated_llm_capacity_unknown_model() {
    let provider = Arc::new(MockProvider::new("anthropic", vec![]));
    let router = make_router_with_mock(
        provider,
        ProviderType::Anthropic,
        "claude-sonnet-4-5-20250929",
    );

    let info = router
        .estimated_llm_capacity(Some("nonexistent-model"))
        .await;
    assert_eq!(info.effective_capacity, 0);
    assert_eq!(info.available_api_keys, 0);
}

#[tokio::test]
async fn test_estimated_llm_capacity_with_cli_fallback() {
    // Single key + CLI backend: CLI is fallback-only, NOT added to parallel capacity
    let provider = Arc::new(MockProvider::new(
        "anthropic",
        vec![Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929"))],
    ));
    let cli_provider = Arc::new(MockProvider::new(
        "claude_cli",
        vec![Ok(MockProvider::ok_response("claude_cli"))],
    ));

    let router = make_router_with_mock(
        provider,
        ProviderType::Anthropic,
        "claude-sonnet-4-5-20250929",
    );
    router.register_cli_backend(ProviderType::Anthropic, cli_provider);

    // 1 key * 5 per-key = 5, CLI NOT counted as parallel bandwidth
    let info = router.estimated_llm_capacity(None).await;
    assert_eq!(info.available_api_keys, 1);
    assert_eq!(info.key_capacity, 5);
    assert!(info.has_cli_fallback);
    assert_eq!(info.effective_capacity, 5);
}

#[tokio::test]
async fn test_estimated_llm_capacity_all_keys_limited_with_cli() {
    // All keys rate-limited but CLI available — CLI provides fallback capacity of 1
    let provider = Arc::new(MockProvider::new(
        "anthropic",
        vec![Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929"))],
    ));
    let cli_provider = Arc::new(MockProvider::new(
        "claude_cli",
        vec![Ok(MockProvider::ok_response("claude_cli"))],
    ));

    let key_pool = KeyPool::new(
        vec![ApiKey::new(
            "k1".to_string(),
            ProviderType::Anthropic,
            "sk-1".to_string(),
        )],
        SelectionStrategy::RoundRobin,
    );
    key_pool
        .report_result(
            "k1",
            CallResult::RateLimited {
                retry_after_ms: 60_000,
            },
        )
        .await;

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Anthropic,
        ProviderEntry {
            provider,
            key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
        },
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "claude-sonnet-4-5-20250929".to_string(),
    );
    router.register_cli_backend(ProviderType::Anthropic, cli_provider);

    // 0 available keys, CLI fallback = effective capacity 1
    let info = router.estimated_llm_capacity(None).await;
    assert_eq!(info.available_api_keys, 0);
    assert_eq!(info.key_capacity, 0);
    assert!(info.has_cli_fallback);
    assert_eq!(info.effective_capacity, 1);
}

#[tokio::test]
async fn test_streaming_key_rotation_on_rate_limit() {
    // First call rate-limited, second call succeeds
    let provider = Arc::new(MockProvider::new(
        "openai",
        vec![
            Err(LlmError::RateLimited {
                retry_after_ms: 1000,
            }),
            Ok(MockProvider::ok_response("gpt-5.2")),
        ],
    ));

    let key_pool = KeyPool::new(
        vec![
            ApiKey::new("k1".to_string(), ProviderType::OpenAI, "sk-1".to_string()),
            ApiKey::new("k2".to_string(), ProviderType::OpenAI, "sk-2".to_string()),
        ],
        SelectionStrategy::RoundRobin,
    );

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::OpenAI,
        ProviderEntry {
            provider,
            key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
        },
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "gpt-5.2".to_string(),
    );

    let request = make_request(Some("gpt-5.2"));
    let result = router.complete_streaming(request).await;
    assert!(result.is_ok(), "Streaming should succeed after key rotation");
}

#[tokio::test]
async fn test_streaming_all_keys_rate_limited() {
    // Both calls rate-limited — should return AllKeysRateLimited
    let provider = Arc::new(MockProvider::new(
        "openai",
        vec![
            Err(LlmError::RateLimited {
                retry_after_ms: 1000,
            }),
            Err(LlmError::RateLimited {
                retry_after_ms: 2000,
            }),
        ],
    ));

    let key_pool = KeyPool::new(
        vec![
            ApiKey::new("k1".to_string(), ProviderType::OpenAI, "sk-1".to_string()),
            ApiKey::new("k2".to_string(), ProviderType::OpenAI, "sk-2".to_string()),
        ],
        SelectionStrategy::RoundRobin,
    );

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::OpenAI,
        ProviderEntry {
            provider,
            key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
        },
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "gpt-5.2".to_string(),
    );

    let request = make_request(Some("gpt-5.2"));
    let result = router.complete_streaming(request).await;
    assert!(
        matches!(result, Err(LlmRouterError::AllKeysRateLimited)),
        "Should return AllKeysRateLimited when all keys fail"
    );
}

#[tokio::test]
async fn test_streaming_non_retryable_error_fails_immediately() {
    // Non-retryable error (e.g. bad request) — should NOT retry
    let provider = Arc::new(MockProvider::new(
        "openai",
        vec![
            Err(LlmError::Api {
                status: 400,
                message: "Bad request".to_string(),
            }),
            Ok(MockProvider::ok_response("gpt-5.2")), // should never reach this
        ],
    ));

    let key_pool = KeyPool::new(
        vec![
            ApiKey::new("k1".to_string(), ProviderType::OpenAI, "sk-1".to_string()),
            ApiKey::new("k2".to_string(), ProviderType::OpenAI, "sk-2".to_string()),
        ],
        SelectionStrategy::RoundRobin,
    );

    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::OpenAI,
        ProviderEntry {
            provider,
            key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
        },
    );

    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "gpt-5.2".to_string(),
    );

    let request = make_request(Some("gpt-5.2"));
    let result = router.complete_streaming(request).await;
    assert!(
        matches!(
            result,
            Err(LlmRouterError::Llm(LlmError::Api { status: 400, .. }))
        ),
        "Non-retryable errors should fail immediately"
    );
}
