//! OpenAI's own rows apply only to OpenAI's own API (COMPAT-SERVER, the
//! owner's answer of 2026-11-15 to pending-decisions §1 *Rev 86*'s first
//! item, (a)).
//!
//! An `openai` provider built to call another server — llama.cpp, LM
//! Studio, vLLM, a gateway — is asked for no compiled row: an alias
//! collapses to the parent's model (M-4 step 3, announced as M-5 says), a
//! compiled `gpt-*` row the server did not list is not routable, and what
//! the router can name is what the server lists, a person's `[models]` row
//! and another loaded provider's. On OpenAI's own hosts, the regional ones
//! included, nothing changes. Every server here is a loopback
//! [`MockHttpServer`]; an address on OpenAI's own hosts is only given to a
//! provider that is never called.

use super::*;
use crate::config::{LlmRuntimeConfig, ModelConfigEntry};
use crate::error::LlmError;
use crate::keys::key_pool::ApiKey;
use crate::providers::openai::OpenAiProvider;
use crate::routing::model_registry::{ModelInfo, RequestContract};
use crate::test_support::{MockHttpServer, MockResponse, loopback_client, refused_base_url};
use crate::types::{ChatMessage, ChatRequest, ChatResponse};
use async_trait::async_trait;
use std::collections::HashSet;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);
/// The model a llama.cpp-like server lists.
const SERVED: &str = "qwen3-32b";

/// OpenAI's `GET /models` listing `ids`.
fn models_body(ids: &[&str]) -> String {
    serde_json::json!({
        "object": "list",
        "data": ids
            .iter()
            .map(|id| serde_json::json!({"id": id, "object": "model", "owned_by": "local"}))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

/// A compatible server listing `ids` and answering a chat with the model it
/// was asked for; with no `ids`, its model list is not found.
async fn compatible_server(ids: &'static [&'static str]) -> MockHttpServer {
    MockHttpServer::start(move |req| match req.path.as_str() {
        "/v1/models" if !ids.is_empty() => MockResponse::json(models_body(ids)),
        "/v1/chat/completions" => {
            let body: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
            MockResponse::json(
                serde_json::json!({
                    "model": body["model"],
                    "choices": [{
                        "message": {"role": "assistant", "content": "pong"},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 1000, "completion_tokens": 1000}
                })
                .to_string(),
            )
        }
        _ => MockResponse::not_found(),
    })
    .await
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

/// The `openai` arm built to call `base_url` (`…/v1`), as the boot builder
/// builds it.
fn openai_at(base_url: &str) -> Arc<dyn LlmProvider> {
    Arc::new(OpenAiProvider::with_client(
        loopback_client(TIMEOUT),
        "sk-test-not-a-key".to_string(),
        Some("gpt-6-luna".to_string()),
        Some(base_url.to_string()),
        None,
    ))
}

/// A loaded provider that is never called: a local Ollama, or Anthropic.
struct Idle(&'static str);

#[async_trait]
impl LlmProvider for Idle {
    fn name(&self) -> &str {
        self.0
    }
    fn supports_tools(&self) -> bool {
        true
    }
    fn requires_key(&self) -> bool {
        self.0 != "ollama"
    }
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Err(LlmError::NotConfigured)
    }
}

/// A router over `loaded`, each provider's compiled rows in the catalogue
/// (a disabled one's not, R58b), `config` its `[models]` rows, `default`
/// the `[orchestrator] model`, and `[providers.openai] base_url` at
/// `openai_base` with `pins` under its `model_aliases`.
fn router_over(
    loaded: Vec<(ProviderType, Arc<dyn LlmProvider>)>,
    config: &HashMap<String, ModelConfigEntry>,
    default: &str,
    openai_base: &str,
    pins: &[(ModelAlias, &str)],
) -> LlmRouter {
    let enabled: Vec<ProviderType> = loaded.iter().map(|(p, _)| p.clone()).collect();
    let disabled: HashSet<ProviderType> = ProviderType::all()
        .iter()
        .filter(|p| !enabled.contains(p))
        .cloned()
        .collect();
    let mut providers = HashMap::new();
    for (provider_type, provider) in loaded {
        let key_pool = if provider_type.is_local() {
            Arc::new(ArcSwap::from_pointee(KeyPool::new(
                vec![],
                SelectionStrategy::RoundRobin,
            )))
        } else {
            keyed_pool(provider_type.clone())
        };
        providers.insert(provider_type, ProviderEntry { provider, key_pool });
    }
    let mut runtime = LlmRuntimeConfig::default();
    let openai = runtime
        .provider_defaults
        .get_mut("openai")
        .expect("a compiled openai row");
    openai.base_url = Some(openai_base.to_string());
    for (alias, model) in pins {
        openai.model_aliases.insert(*alias, model.to_string());
    }
    LlmRouter::new_with_runtime(
        providers,
        ModelRegistry::with_defaults_and_config(config, &disabled),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        default.to_string(),
        runtime,
        RateLimitConfig::default(),
    )
}

/// An OpenAI-only install whose `openai` provider and `llm.toml` both name
/// `base`, with the seeded main model, `gpt-6-luna`.
fn openai_only(base: &str, pins: &[(ModelAlias, &str)]) -> LlmRouter {
    router_over(
        vec![(ProviderType::OpenAI, openai_at(base))],
        &HashMap::new(),
        "gpt-6-luna",
        base,
        pins,
    )
}

/// A row as `provider`'s discovery registers one, written directly where
/// the provider must not be called.
fn discovered_by(router: &LlmRouter, provider: ProviderType, id: &str) {
    router.model_registry().register_discovered(
        id.to_string(),
        ModelInfo {
            provider,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 0,
            discovered: true,
            supports_image: false,
            supports_audio: false,
            supports_document: false,
            supports_reasoning: false,
            supports_tools: true,
            declared: false,
            long_prompt: None,
            cache_rates: None,
            request_contract: RequestContract::NONE,
        },
    );
}

/// A model the `openai` provider's discovery listed.
fn listed(router: &LlmRouter, id: &str) {
    discovered_by(router, ProviderType::OpenAI, id);
}

fn request(model: &str) -> RouterRequest {
    RouterRequest {
        model: Some(model.to_string()),
        messages: Arc::new(vec![ChatMessage::user("ping")]),
        tools: Arc::new(vec![]),
        temperature: None,
        max_tokens: None,
        context: RequestContext {
            task_id: Some("task-1".to_string()),
            ..RequestContext::default()
        },
        tool_choice: None,
        tools_token_estimate: None,
        enable_caching: false,
        thinking: None,
        context_management: None,
        fallback_models: Vec::new(),
        ephemeral_system_notice: None,
    }
}

fn resolved(router: &LlmRouter, alias: ModelAlias) -> Option<String> {
    router.resolve_alias(alias, None).map(|r| r.model)
}

/// On a loopback compatible server every alias and the main model are the
/// model the server lists, the compiled `gpt-*` rows are not routable, the
/// call names the server's model, and it is counted at the loopback rule's
/// 0 — with no line saying an alias passed a model over. On the parent
/// `strong` was `gpt-6.1-sol`, `fast` `gpt-6-luna`, the main model
/// `gpt-6-luna`, and the call asked the server for `gpt-6.1-sol`.
#[tokio::test]
#[tracing_test::traced_test]
async fn on_a_loopback_compatible_server_the_aliases_are_the_servers_own_model() {
    let server = compatible_server(&[SERVED]).await;
    let base = format!("{}/v1", server.base_url);
    let router = openai_only(&base, &[]);
    router.refresh_models_for(&ProviderType::OpenAI).await;

    for alias in ModelAlias::ALL {
        let resolution = router.resolve_alias(alias, None).expect("a model");
        assert_eq!(resolution.model, SERVED, "{alias}");
        assert!(resolution.passed_over.is_empty(), "{alias}");
    }
    assert_eq!(router.effective_default_model().as_deref(), Some(SERVED));
    assert_eq!(router.routable_models(), vec![SERVED.to_string()]);
    for compiled in ["gpt-6.1-sol", "gpt-6-luna", "gpt-6.1-terra", "gpt-6-astra"] {
        assert!(!router.is_routable(compiled), "{compiled}");
    }

    let strong = resolved(&router, ModelAlias::Strong).unwrap();
    let routed = router
        .complete(request(&strong))
        .await
        .expect("the server answers");
    assert_eq!(routed.model, SERVED);
    let chats: Vec<String> = server
        .requests()
        .await
        .into_iter()
        .filter(|r| r.path == "/v1/chat/completions")
        .map(|r| r.body)
        .collect();
    assert_eq!(chats.len(), 1);
    let sent: serde_json::Value = serde_json::from_str(&chats[0]).unwrap();
    assert_eq!(sent["model"], SERVED);
    let spent = router
        .cost_tracker
        .get_task_usage("task-1")
        .await
        .expect("the call was counted");
    assert_eq!(spent.total_cost_usd, 0.0, "the loopback rule's 0");
    assert!(!logs_contain("would use"));
}

/// A compatible server whose model list cannot be read leaves no compiled
/// row standing in for it: discovery no longer marks the `gpt-*` rows as
/// served, so nothing is routable (a call is then refused naming the fix,
/// `NoRoutableModel`), where the parent ran on `gpt-6-luna`.
#[tokio::test]
async fn a_compatible_server_whose_list_fails_is_not_given_the_compiled_rows() {
    let server = compatible_server(&[]).await;
    let base = format!("{}/v1", server.base_url);
    let router = openai_only(&base, &[]);
    let discovery = router.refresh_models_for(&ProviderType::OpenAI).await;
    assert_eq!(discovery.models, 0, "the list was not found");

    assert_eq!(router.effective_default_model(), None);
    assert_eq!(router.routable_models(), Vec::<String>::new());
    assert!(router.resolve_alias(ModelAlias::Strong, None).is_none());
    assert!(!router.is_routable("gpt-6-luna"));
}

/// A `[models]` row the person wrote for the server is routable without a
/// discovery, and is the main model and every alias: the compiled `fast`
/// row, which the main-model ladder prefers on OpenAI's own API (ML-7), no
/// longer stands before it. A discovery that cannot read the server's list
/// offers that row and no compiled one, and a reload that names OpenAI's
/// own `base_url` changes nothing while the provider still calls the
/// server. On the parent the main model was `gpt-6-luna`.
#[tokio::test]
async fn a_persons_models_row_on_a_compatible_server_is_its_main_model() {
    let base = format!("{}/v1", refused_base_url());
    let config = HashMap::from([(
        "my-gguf".to_string(),
        ModelConfigEntry {
            provider: "openai".to_string(),
            input_price: None,
            output_price: None,
            context: Some(16_384),
            supports_image: None,
            supports_audio: None,
            supports_document: None,
            supports_reasoning: None,
            supports_tools: None,
        },
    )]);
    let router = router_over(
        vec![(ProviderType::OpenAI, openai_at(&base))],
        &config,
        "gpt-6-luna",
        &base,
        &[],
    );
    assert_eq!(router.effective_default_model().as_deref(), Some("my-gguf"));
    assert_eq!(
        resolved(&router, ModelAlias::Strong).as_deref(),
        Some("my-gguf")
    );
    assert_eq!(router.routable_models(), vec!["my-gguf".to_string()]);

    let discovery = router.refresh_models_for(&ProviderType::OpenAI).await;
    assert!(discovery.error.is_some(), "nothing listens there");
    let offered: Vec<String> = router
        .available_models()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(offered, vec!["my-gguf".to_string()]);

    let mut runtime = LlmRuntimeConfig::default();
    runtime
        .provider_defaults
        .get_mut("openai")
        .expect("a compiled openai row")
        .base_url = Some("https://api.openai.com/v1".to_string());
    router.reload_runtime_config(runtime);
    assert!(!router.is_routable("gpt-6-luna"));
    assert_eq!(
        resolved(&router, ModelAlias::Strong).as_deref(),
        Some("my-gguf")
    );
}

/// On OpenAI's own API the compiled rows still apply, whatever else the
/// provider's discovery listed. A guard: green on the parent as well.
#[tokio::test]
async fn on_openai_s_own_api_the_compiled_rows_apply() {
    let router = openai_only("https://api.openai.com/v1", &[]);
    listed(&router, SERVED);
    assert_eq!(
        resolved(&router, ModelAlias::Strong).as_deref(),
        Some("gpt-6.1-sol")
    );
    assert_eq!(
        resolved(&router, ModelAlias::Fast).as_deref(),
        Some("gpt-6-luna")
    );
    assert!(router.is_routable("gpt-6.1-sol"));
    assert!(router.is_routable(SERVED));
    assert_eq!(
        router.effective_default_model().as_deref(),
        Some("gpt-6-luna")
    );
}

/// A regional host of OpenAI's own API counts as OpenAI's own. A guard:
/// green on the parent as well.
#[tokio::test]
async fn a_regional_host_counts_as_openai_s_own_api() {
    for base in [
        "https://eu.api.openai.com/v1",
        "https://jp.api.openai.com/v1",
    ] {
        let router = openai_only(base, &[]);
        assert_eq!(
            resolved(&router, ModelAlias::Strong).as_deref(),
            Some("gpt-6.1-sol"),
            "{base}"
        );
        assert_eq!(
            resolved(&router, ModelAlias::Fast).as_deref(),
            Some("gpt-6-luna"),
            "{base}"
        );
    }
}

/// A pin under `[providers.openai.model_aliases]` still wins on a
/// compatible server (M-4 step 1); an alias with none collapses. The pin is
/// a guard, green on the parent; `fast`'s collapse is red there.
#[tokio::test]
async fn a_pin_still_wins_on_a_compatible_server() {
    let base = format!("{}/v1", refused_base_url());
    let router = openai_only(&base, &[(ModelAlias::Strong, "qwen3-coder-30b")]);
    listed(&router, SERVED);
    listed(&router, "qwen3-coder-30b");
    assert_eq!(
        resolved(&router, ModelAlias::Strong).as_deref(),
        Some("qwen3-coder-30b")
    );
    assert_eq!(resolved(&router, ModelAlias::Fast).as_deref(), Some(SERVED));
}

/// A gateway that lists an OpenAI id serves it: the id is routable there,
/// but no alias names it — the compiled alias row is OpenAI's own API's
/// (built as the owner's (a) reads; what such a row is priced and sized at
/// is §1's owner item). On the parent `strong` was `gpt-6.1-sol` here too.
#[tokio::test]
async fn a_gateway_listing_an_openai_id_serves_it_but_the_aliases_collapse() {
    let base = format!("{}/v1", refused_base_url());
    let router = router_over(
        vec![(ProviderType::OpenAI, openai_at(&base))],
        &HashMap::new(),
        SERVED,
        &base,
        &[],
    );
    listed(&router, SERVED);
    listed(&router, "gpt-6.1-sol");
    assert!(router.is_routable("gpt-6.1-sol"));
    assert!(!router.is_routable("gpt-6-luna"));
    assert_eq!(
        router.routable_models(),
        vec!["gpt-6.1-sol".to_string(), SERVED.to_string()]
    );
    assert_eq!(
        resolved(&router, ModelAlias::Strong).as_deref(),
        Some(SERVED)
    );
}

/// M-5 on a mixed install: with a local main model and an `openai`
/// provider at a compatible server, no other loaded provider has a model
/// for an alias, so the collapse is not announced, as on Ollama alone. On
/// the parent OpenAI's compiled row counted, and `("strong", "ollama")`
/// was said.
#[tokio::test]
async fn a_compatible_openai_beside_a_local_main_model_announces_no_collapse() {
    let base = format!("{}/v1", refused_base_url());
    let router = router_over(
        vec![
            (ProviderType::OpenAI, openai_at(&base)),
            (ProviderType::Ollama, Arc::new(Idle("ollama"))),
        ],
        &HashMap::new(),
        "qwen3:8b",
        &base,
        &[],
    );
    discovered_by(&router, ProviderType::Ollama, "qwen3:8b");
    assert_eq!(
        resolved(&router, ModelAlias::Strong).as_deref(),
        Some("qwen3:8b")
    );
    assert_eq!(
        router.noted_alias_collapses(),
        Vec::<(String, String)>::new()
    );
}

/// M-5 on a mixed install the other way: the parent on a compatible
/// server, Anthropic loaded, so another loaded provider has a model for
/// the alias and the collapse is said once at `INFO`. On the parent the
/// alias took OpenAI's compiled row instead and nothing was said.
#[tokio::test]
async fn a_compatible_openai_parent_beside_anthropic_announces_its_collapse() {
    let base = format!("{}/v1", refused_base_url());
    let router = router_over(
        vec![
            (ProviderType::OpenAI, openai_at(&base)),
            (ProviderType::Anthropic, Arc::new(Idle("anthropic"))),
        ],
        &HashMap::new(),
        SERVED,
        &base,
        &[],
    );
    listed(&router, SERVED);
    assert_eq!(
        resolved(&router, ModelAlias::Strong).as_deref(),
        Some(SERVED)
    );
    assert_eq!(
        router.noted_alias_collapses(),
        vec![("strong".to_string(), "openai".to_string())]
    );
}

/// A non-catalogue id OpenAI's own discovery listed.
const OWN_ONLY: &str = "an-own-api-only-model";

/// An OpenAI-only install on OpenAI's own API whose discovery listed
/// `gpt-6-luna`, `gpt-6.1-sol` and [`OWN_ONLY`], its provider then rebuilt
/// in place at `base` (`…/v1`), as `PUT …/providers/openai/enabled`
/// rebuilds a loaded provider after a `base_url` edit. A rebuild at the
/// same address keeps what discovery found.
fn rebuilt_from_own_api(base: &str) -> LlmRouter {
    let own = "https://api.openai.com/v1";
    let router = openai_only(own, &[]);
    for id in ["gpt-6-luna", "gpt-6.1-sol", OWN_ONLY] {
        listed(&router, id);
    }
    let pool = || {
        KeyPool::new(
            vec![ApiKey::new(
                "test".to_string(),
                ProviderType::OpenAI,
                "sk-test-not-a-key".to_string(),
            )],
            SelectionStrategy::RoundRobin,
        )
    };
    router.register_provider(ProviderType::OpenAI, openai_at(own), pool());
    assert!(router.is_routable(OWN_ONLY), "a same-address rebuild");
    router.register_provider(ProviderType::OpenAI, openai_at(base), pool());
    router
}

/// A provider rebuilt in place at a compatible server forgets what OpenAI's
/// own discovery found: before any refresh no compiled row and no id that
/// discovery listed is routable there, and a refresh offers what the server
/// lists. On the change before this test the rows OpenAI's API had listed
/// stayed marked, so `gpt-6-luna` stayed the main model and the server was
/// asked for it.
#[tokio::test]
async fn a_provider_rebuilt_at_a_compatible_server_forgets_openai_s_listing() {
    let server = compatible_server(&[SERVED]).await;
    let router = rebuilt_from_own_api(&format!("{}/v1", server.base_url));
    for id in ["gpt-6-luna", "gpt-6.1-sol", OWN_ONLY] {
        assert!(!router.is_routable(id), "{id} before a refresh");
    }
    assert_eq!(router.effective_default_model(), None);

    router.refresh_models_for(&ProviderType::OpenAI).await;
    assert_eq!(router.effective_default_model().as_deref(), Some(SERVED));
    assert_eq!(router.routable_models(), vec![SERVED.to_string()]);
    let offered: Vec<String> = router
        .available_models()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(offered, vec![SERVED.to_string()]);
}

/// The same rebuild at a compatible server whose list cannot be read
/// leaves nothing routable: no row OpenAI's API listed stands in for it.
#[tokio::test]
async fn a_provider_rebuilt_at_an_unlisting_compatible_server_keeps_nothing() {
    let server = compatible_server(&[]).await;
    let router = rebuilt_from_own_api(&format!("{}/v1", server.base_url));
    router.refresh_models_for(&ProviderType::OpenAI).await;
    assert_eq!(router.effective_default_model(), None);
    assert_eq!(router.routable_models(), Vec::<String>::new());
    assert!(!router.is_routable("gpt-6-luna"));
}

/// A `[models]` row of the person's naming a compiled `gpt-*` id stays
/// routable on a compatible server with no discovery, and is its main
/// model, while the compiled rows it does not name are not; a discovery
/// that cannot read the server's list offers it.
#[tokio::test]
async fn a_persons_row_naming_a_compiled_id_is_routable_on_a_compatible_server() {
    let base = format!("{}/v1", refused_base_url());
    let config = HashMap::from([(
        "gpt-6.1-sol".to_string(),
        ModelConfigEntry {
            provider: "openai".to_string(),
            input_price: None,
            output_price: None,
            context: None,
            supports_image: None,
            supports_audio: None,
            supports_document: None,
            supports_reasoning: None,
            supports_tools: None,
        },
    )]);
    let router = router_over(
        vec![(ProviderType::OpenAI, openai_at(&base))],
        &config,
        "gpt-6-luna",
        &base,
        &[],
    );
    assert!(router.is_routable("gpt-6.1-sol"));
    assert!(!router.is_routable("gpt-6-luna"));
    assert_eq!(router.routable_models(), vec!["gpt-6.1-sol".to_string()]);
    assert_eq!(
        router.effective_default_model().as_deref(),
        Some("gpt-6.1-sol")
    );

    router.refresh_models_for(&ProviderType::OpenAI).await;
    let offered: Vec<String> = router
        .available_models()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(offered, vec!["gpt-6.1-sol".to_string()]);
}
