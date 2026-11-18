//! C8 (the subagent design's SA-3, §8 C8, §9 C8): the shipped templates name
//! `inherit` or a model alias, a lead and each subagent it spawns run on the
//! model their template's word resolves to, and `spawn_subagent` takes a
//! per-call `model`, routable or refused.
//!
//! Every run here is a whole lead run over the shipped (bundled) templates,
//! on a router built by hand over one recording provider standing in for
//! every loaded provider: what is asserted is the model each request names
//! as it reaches the provider, the tool results the lead reads, and what the
//! spawn tool's definitions say. No socket, no key, no live model. An
//! alias's id is read from `ALIAS_TABLE` (`compiled_alias`, [`row`]), so a
//! release that moves the table moves those expectations with it; the
//! catalogue's listed ids, a pin's value and the earlier bundle's ids are
//! written as literals, each one on a provider page read for C8.

use super::*;
use crate::bundled::bundled_templates;
use openalpaca_llm::{
    ApiKey, ChatRequest, ChatResponse, CostTracker, FinishReason, KeyPool, LlmError, LlmProvider,
    LlmRouterConfig, LlmRuntimeConfig, ModelAlias, ModelInfo, ModelRegistry, ProviderEntry,
    ProviderType, SelectionStrategy, ToolCall, ToolDefinition, Usage, compiled_alias,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// What one request asked for.
#[derive(Clone)]
struct Seen {
    model: Option<String>,
    lead: bool,
    /// The request's last user message: a subagent's is its objective.
    last_user: String,
    tools: Vec<ToolDefinition>,
    messages: Vec<openalpaca_llm::ChatMessage>,
}

/// One model standing in for every loaded provider. A request whose tools
/// hold `spawn_subagent` is the lead's, answered from `lead` in order (then
/// with a final answer); any other is a subagent's, answered from the script
/// whose key its objective contains (else one plain answer).
struct Models {
    lead: Mutex<VecDeque<ChatResponse>>,
    children: Mutex<Vec<(String, VecDeque<ChatResponse>)>>,
    seen: Mutex<Vec<Seen>>,
    /// A model whose every request is refused for its credentials, as a
    /// provider answers a key it will not take: the router then walks the
    /// ladder from it (`LlmRouterError::AllKeysRefused`).
    refused: Mutex<Option<String>>,
    /// The address it says it was built to call ([`Models::at`]); none
    /// unless a test sets one.
    address: OnceLock<String>,
    /// Whether it says it calls its vendor's own API, as the OpenAI arm
    /// does for OpenAI's own hosts only (COMPAT-SERVER); true unless a
    /// test says otherwise.
    own_api: AtomicBool,
}

impl Models {
    fn new(lead: Vec<ChatResponse>) -> Arc<Self> {
        Arc::new(Self {
            lead: Mutex::new(lead.into()),
            children: Mutex::new(Vec::new()),
            seen: Mutex::new(Vec::new()),
            refused: Mutex::new(None),
            address: OnceLock::new(),
            own_api: AtomicBool::new(true),
        })
    }

    /// It stands in for a provider built to call `address`, which is its
    /// vendor's own API when `own_api` (the OpenAI arm: OpenAI's own hosts
    /// only) and, otherwise, an OpenAI-compatible server. Nothing is sent
    /// to the address.
    fn at(self: &Arc<Self>, address: &str, own_api: bool) -> Arc<Self> {
        self.address.set(address.to_string()).expect("set once");
        self.own_api.store(own_api, Ordering::SeqCst);
        self.clone()
    }

    /// Every request naming `model` is refused for its credentials.
    fn refusing(self: &Arc<Self>, model: &str) -> Arc<Self> {
        *self.refused.lock().unwrap() = Some(model.to_string());
        self.clone()
    }

    /// Every model a request of the lead (`lead`) or of a subagent named.
    fn models_asked(&self, lead: bool) -> Vec<String> {
        self.seen()
            .into_iter()
            .filter(|s| s.lead == lead)
            .filter_map(|s| s.model)
            .collect()
    }

    /// The subagent whose objective contains `key` answers with `script`.
    fn child(self: &Arc<Self>, key: &str, script: Vec<ChatResponse>) -> Arc<Self> {
        self.children
            .lock()
            .unwrap()
            .push((key.to_string(), script.into()));
        self.clone()
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// The model the first request of the lead named.
    fn lead_model(&self) -> String {
        self.seen()
            .into_iter()
            .find(|s| s.lead)
            .and_then(|s| s.model)
            .expect("the lead made a request")
    }

    /// The model the subagent whose objective contains `key` was run on.
    fn child_model(&self, key: &str) -> String {
        self.seen()
            .into_iter()
            .find(|s| !s.lead && s.last_user.contains(key))
            .and_then(|s| s.model)
            .unwrap_or_else(|| panic!("no subagent request for {key}"))
    }

    /// The lead's request `n` (0-based).
    fn lead_request(&self, n: usize) -> Seen {
        self.seen()
            .into_iter()
            .filter(|s| s.lead)
            .nth(n)
            .unwrap_or_else(|| panic!("the lead made no request {n}"))
    }

    /// Every tool result the lead was handed, in order.
    fn tool_results(&self) -> Vec<String> {
        let last = self
            .seen()
            .into_iter()
            .rfind(|s| s.lead)
            .expect("the lead made a request");
        last.messages
            .iter()
            .filter(|m| m.role == openalpaca_llm::Role::Tool)
            .map(|m| m.content.clone())
            .collect()
    }
}

fn answer(content: &str, tool_calls: Vec<ToolCall>) -> ChatResponse {
    ChatResponse {
        content: content.to_string(),
        finish_reason: if tool_calls.is_empty() {
            FinishReason::Stop
        } else {
            FinishReason::ToolUse
        },
        tool_calls,
        model: String::new(),
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        },
        thinking: None,
        echo: None,
        parts: None,
    }
}

fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: format!("call-{name}"),
        name: name.to_string(),
        arguments,
    }
}

#[async_trait]
impl LlmProvider for Models {
    fn name(&self) -> &str {
        "models"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn requires_key(&self) -> bool {
        false
    }

    fn base_url(&self) -> Option<&str> {
        self.address.get().map(String::as_str)
    }

    fn serves_compiled_rows(&self) -> bool {
        self.own_api.load(Ordering::SeqCst)
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let lead = request.tools.iter().any(|t| t.name == "spawn_subagent");
        let last_user = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == openalpaca_llm::Role::User)
            .map(|m| m.content.clone())
            .unwrap_or_default();
        self.seen.lock().unwrap().push(Seen {
            model: request.model.clone(),
            lead,
            last_user: last_user.clone(),
            tools: request.tools.as_ref().clone(),
            messages: request.messages.as_ref().clone(),
        });
        if request.model.is_some() && *self.refused.lock().unwrap() == request.model {
            return Err(LlmError::AuthenticationFailed(
                "the key was refused".to_string(),
            ));
        }
        let next = if lead {
            self.lead.lock().unwrap().pop_front()
        } else {
            let mut children = self.children.lock().unwrap();
            children
                .iter_mut()
                .find(|(key, _)| last_user.contains(key.as_str()))
                .and_then(|(_, script)| script.pop_front())
        };
        let mut response = next.unwrap_or_else(|| answer("done", vec![]));
        response.model = request.model.unwrap_or_else(|| "unnamed".to_string());
        Ok(response)
    }
}

/// A router with `loaded` switched on over `models`, each loaded provider's
/// compiled rows in the catalogue (a disabled provider contributes none), no
/// discovery, and `default` the `[orchestrator] model` — the seeded one on a
/// stock install. `chains` is `llm.toml`'s `[fallback_chains]`.
fn install_with(
    models: &Arc<Models>,
    loaded: &[ProviderType],
    default: &str,
    chains: HashMap<String, Vec<String>>,
) -> Arc<LlmRouter> {
    let mut providers = HashMap::new();
    for provider in loaded {
        providers.insert(
            provider.clone(),
            ProviderEntry {
                provider: models.clone(),
                key_pool: Arc::new(ArcSwap::from_pointee(KeyPool::new(
                    vec![ApiKey::new(
                        "test".to_string(),
                        provider.clone(),
                        "sk-test-not-a-key".to_string(),
                    )],
                    SelectionStrategy::RoundRobin,
                ))),
            },
        );
    }
    let disabled: HashSet<ProviderType> = ProviderType::all()
        .iter()
        .filter(|p| !loaded.contains(p))
        .cloned()
        .collect();
    let registry = ModelRegistry::with_defaults_and_config(&HashMap::new(), &disabled);
    let router = LlmRouter::new(
        providers,
        registry,
        chains,
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        default.to_string(),
    );
    Arc::new(router)
}

fn install(models: &Arc<Models>, loaded: &[ProviderType], default: &str) -> Arc<LlmRouter> {
    install_with(models, loaded, default, HashMap::new())
}

/// A model a local Ollama reported, as discovery registers it.
fn pulled(router: &LlmRouter, tag: &str) {
    router.model_registry().register(
        tag.to_string(),
        ModelInfo {
            provider: ProviderType::Ollama,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 32_768,
            discovered: true,
            supports_image: false,
            supports_audio: false,
            supports_document: false,
            supports_reasoning: false,
            supports_tools: true,
            declared: false,
            long_prompt: None,
            cache_rates: None,
            request_contract: openalpaca_llm::RequestContract::NONE,
        },
    );
}

/// What a load or a hot reload of this `llm.toml` hands the router.
fn load(router: &LlmRouter, llm_toml: &str) {
    let config: LlmRouterConfig = toml::from_str(llm_toml).expect("llm.toml parses");
    router.reload_runtime_config(LlmRuntimeConfig::from(&config));
}

/// The id `alias` names on `provider` in this build's table.
fn row(provider: ProviderType, alias: ModelAlias) -> String {
    compiled_alias(&provider, alias)
        .unwrap_or_else(|| panic!("{provider} has a row for {alias}"))
        .to_string()
}

/// A store's registry holding the shipped templates, as the daemon's start
/// loads them with nothing in `config/agents/`.
fn shipped() -> Arc<SharedContext> {
    let shared = Arc::new(SharedContext::new());
    shared.agent_registry.set_bundled_layer(bundled_templates());
    shared
}

/// The eight shipped workers, each an objective that names it.
const WORKERS: [&str; 8] = [
    "code_agent",
    "explore_agent",
    "general_agent",
    "planning_agent",
    "research_agent",
    "review_agent",
    "system_agent",
    "writing_agent",
];

/// The shipped workers whose template names `fast`: the two read-only
/// profiles, and `research_agent` since OUTPUT-BUDGET-4 (c), which keeps
/// its $0.35 cap on the cheaper row. The other five name `inherit`.
const ON_FAST: [&str; 3] = ["explore_agent", "research_agent", "system_agent"];

/// The lead spawns `items` in one batch, waits, and answers.
fn batch_lead(items: serde_json::Value) -> Vec<ChatResponse> {
    vec![
        answer(
            "",
            vec![call(
                "spawn_subagents_batch",
                serde_json::json!({ "subagents": items }),
            )],
        ),
        answer("", vec![call("wait_for_subagents", serde_json::json!({}))]),
        answer("all done", vec![]),
    ]
}

/// The lead spawns one subagent with `arguments`, waits, and answers.
fn single_lead(arguments: serde_json::Value) -> Vec<ChatResponse> {
    vec![
        answer("", vec![call("spawn_subagent", arguments)]),
        answer("", vec![call("wait_for_subagents", serde_json::json!({}))]),
        answer("all done", vec![]),
    ]
}

fn every_worker() -> serde_json::Value {
    serde_json::Value::Array(
        WORKERS
            .iter()
            .map(|id| serde_json::json!({ "agent_id": id, "objective": format!("work for {id}") }))
            .collect(),
    )
}

/// `id`'s instance, spawned from the store's registry as the dispatcher
/// spawns a lead.
fn instance(shared: &SharedContext, id: &str) -> SubAgent {
    shared
        .agent_registry
        .spawn_instance(id, "task-1".to_string())
        .unwrap_or_else(|e| panic!("{id} spawns: {e}"))
}

/// One lead run of `lead` over `router` and `shared`'s templates.
async fn run(
    lead: &SubAgent,
    router: Arc<LlmRouter>,
    shared: Arc<SharedContext>,
    bus: EventBus,
) -> LeadAgentResult {
    let home = tempfile::tempdir().unwrap();
    let home_root = home.path().canonicalize().unwrap();
    let _guard = crate::test_util::HomeStoreGuard::set(&home_root);
    run_lead_agent(
        lead,
        "weigh the herd",
        router,
        Arc::new(ToolRegistry::default()),
        shared,
        bus,
        None,
        None,
        None,
        "task-1",
        "user-1",
        "user-1:cli",
        "cli",
        &Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        MemoryScopeContext::global_only(),
        None,
        None,
        None,
        "lead::task-1",
        "",
        None,
        Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new()),
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        None,
        false,
        None,
    )
    .await
}

/// The shipped lead spawns all eight shipped workers on `router`.
async fn every_template_on(models: &Arc<Models>, router: Arc<LlmRouter>) {
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    let result = run(&lead, router, shared, EventBus::default()).await;
    assert!(result.success, "{:?}", result.loop_result.finish_reason);
    let results = models.tool_results();
    assert!(
        results[0].contains("8/8 subagents spawned"),
        "every worker spawned: {}",
        results[0]
    );
}

// ── Every shipped template, by install (M-4, M-6, M-6a, M-6c) ───────────────

/// **A stock Anthropic install** (the seeded `[orchestrator] model`, Claude
/// Haiku 5.5): the lead names `strong` and runs on Anthropic's `strong` row,
/// the five that follow it `inherit` the model that answered it, and
/// `explore_agent`, `research_agent` and `system_agent` run on `fast`'s
/// row. On the parent the nine named retired ids (Sonnet 4.6, Haiku 4.5)
/// that this router does not hold, so each walked the ladder to the main
/// model, Haiku 5.5 (ML-9).
#[tokio::test]
async fn each_shipped_template_answers_on_its_latest_row_on_an_anthropic_only_install() {
    let models = Models::new(batch_lead(every_worker()));
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    every_template_on(&models, router).await;

    let strong = row(ProviderType::Anthropic, ModelAlias::Strong);
    let fast = row(ProviderType::Anthropic, ModelAlias::Fast);
    assert_eq!(models.lead_model(), strong, "the lead names strong");
    for id in WORKERS {
        let expected = if ON_FAST.contains(&id) {
            &fast
        } else {
            &strong
        };
        assert_eq!(&models.child_model(id), expected, "{id}");
    }
}

/// **An OpenAI-only install, catalogue only** (no discovery; the seeded
/// OpenAI default, GPT-6 Luna, as the main model): the lead and its five on
/// OpenAI's `strong` row, the three `fast` templates on its `fast` row. On the
/// parent every shipped Claude pin was unroutable here and fell to Luna.
#[tokio::test]
async fn each_shipped_template_answers_on_its_latest_row_on_an_openai_only_install() {
    let models = Models::new(batch_lead(every_worker()));
    let router = install(&models, &[ProviderType::OpenAI], "gpt-6-luna");
    every_template_on(&models, router).await;

    let strong = row(ProviderType::OpenAI, ModelAlias::Strong);
    let fast = row(ProviderType::OpenAI, ModelAlias::Fast);
    assert_eq!(models.lead_model(), strong);
    for id in WORKERS {
        let expected = if ON_FAST.contains(&id) {
            &fast
        } else {
            &strong
        };
        assert_eq!(&models.child_model(id), expected, "{id}");
    }
}

/// O-13 (a): on an OpenAI-only install whose person set `[orchestrator]
/// model`, the compiled `strong` row, not that main model, carries the lead
/// (a pin of `strong` restores it, M-6c). On the parent the lead's Claude
/// pin fell to the main model.
#[tokio::test]
async fn on_openai_only_with_a_configured_main_model_the_lead_runs_on_strong() {
    let models = Models::new(vec![answer("all done", vec![])]);
    let router = install(&models, &[ProviderType::OpenAI], "gpt-6.1-terra");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert_eq!(
        models.lead_model(),
        row(ProviderType::OpenAI, ModelAlias::Strong)
    );
}

/// M-6c's pin: `[providers.anthropic.model_aliases] strong = "<id>"` keeps
/// the lead, and the six that follow it, on the model the person names.
#[tokio::test]
async fn a_strong_pin_keeps_the_lead_on_the_pinned_model() {
    let models = Models::new(batch_lead(serde_json::json!([
        { "agent_id": "general_agent", "objective": "work for general_agent" }
    ])));
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    load(
        &router,
        r#"
[providers.anthropic.model_aliases]
strong = "claude-sonnet-5-5"
"#,
    );
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert_eq!(models.lead_model(), "claude-sonnet-5-5");
    assert_eq!(models.child_model("general_agent"), "claude-sonnet-5-5");
}

/// M-6c: with Ollama as the main model and Anthropic also loaded, the
/// shipped templates' workflows run on the local model — an alias never
/// leaves its parent's provider (M-4). Green on the parent too, which the
/// design's M-6c did not foresee: since MODELS-LATEST the parent's templates
/// named retired ids no catalogue row holds, so they walked the ladder to
/// the local main model (ML-9's second case); before MODELS-LATEST the
/// shipped Claude pins were routable here and workflows went to Anthropic.
#[tokio::test]
async fn on_ollama_main_with_anthropic_loaded_workflows_stay_on_the_local_model() {
    let models = Models::new(batch_lead(serde_json::json!([
        { "agent_id": "explore_agent", "objective": "work for explore_agent" },
        { "agent_id": "general_agent", "objective": "work for general_agent" }
    ])));
    let router = install(
        &models,
        &[ProviderType::Anthropic, ProviderType::Ollama],
        "qwen3:8b",
    );
    pulled(&router, "qwen3:8b");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert_eq!(models.lead_model(), "qwen3:8b");
    assert_eq!(models.child_model("explore_agent"), "qwen3:8b");
    assert_eq!(models.child_model("general_agent"), "qwen3:8b");
}

/// M-6c: Anthropic and OpenAI both loaded, the main model on OpenAI —
/// workflows follow the main model's provider. On the parent the lead ran
/// on its Claude pin, routable here.
#[tokio::test]
async fn on_anthropic_and_openai_with_an_openai_main_the_lead_follows_the_main_provider() {
    let models = Models::new(batch_lead(serde_json::json!([
        { "agent_id": "explore_agent", "objective": "work for explore_agent" }
    ])));
    let router = install(
        &models,
        &[ProviderType::Anthropic, ProviderType::OpenAI],
        "gpt-6-luna",
    );
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert_eq!(
        models.lead_model(),
        row(ProviderType::OpenAI, ModelAlias::Strong)
    );
    assert_eq!(
        models.child_model("explore_agent"),
        row(ProviderType::OpenAI, ModelAlias::Fast)
    );
}

/// M-6c: a `[fallback_chains]` entry for a Claude id an earlier bundle
/// pinned no longer steers the shipped templates on an install without
/// Anthropic — an alias resolves to a routable model and never walks the
/// ladder. On the parent the lead's `claude-sonnet-4-6` took this chain.
#[tokio::test]
async fn a_fallback_chain_for_a_claude_id_no_longer_steers_the_shipped_templates() {
    let models = Models::new(vec![answer("all done", vec![])]);
    let chains = HashMap::from([(
        "claude-sonnet-4-6".to_string(),
        vec!["gpt-6.1-terra".to_string()],
    )]);
    let router = install_with(&models, &[ProviderType::OpenAI], "gpt-6-luna", chains);
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert_eq!(
        models.lead_model(),
        row(ProviderType::OpenAI, ModelAlias::Strong)
    );
}

/// M-6c: `[orchestrator] fallback_models` (stored under the main model's id)
/// still serves the main loop but no longer steers the shipped templates.
#[tokio::test]
async fn a_routable_orchestrator_fallback_model_no_longer_steers_the_shipped_templates() {
    let models = Models::new(vec![answer("all done", vec![])]);
    // The seeded `[orchestrator]` of an install seeded before MODELS-LATEST,
    // its main model now retired, its fallback routable.
    let chains = HashMap::from([(
        "claude-haiku-4-5-20251001".to_string(),
        vec!["claude-sonnet-5-5".to_string()],
    )]);
    let router = install_with(
        &models,
        &[ProviderType::Anthropic],
        "claude-haiku-4-5-20251001",
        chains,
    );
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert_eq!(
        models.lead_model(),
        row(ProviderType::Anthropic, ModelAlias::Strong)
    );
}

// ── inherit (M-1, M-1a, M-1b, M-6e) ─────────────────────────────────────────

/// M-1: a subagent on `inherit` runs on the model that **answered** its
/// lead — here a lead whose own id is not routable, answered by its chain's
/// rung — not on the lead's requested id nor the router default. On the
/// parent `general_agent` pinned a retired Sonnet and fell to the default.
#[tokio::test]
async fn a_subagent_on_inherit_runs_on_the_model_that_answered_its_lead() {
    let models = Models::new(single_lead(serde_json::json!({
        "agent_id": "general_agent",
        "objective": "work for general_agent"
    })));
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    let mut lead = instance(&shared, "lead_agent");
    lead.llm_config.model = Some("claude-opus-4-6".to_string());
    lead.llm_config.fallback_models = vec!["claude-sonnet-5-5".to_string()];
    run(&lead, router, shared, EventBus::default()).await;
    assert_eq!(
        models.lead_model(),
        "claude-sonnet-5-5",
        "the ladder's rung"
    );
    assert_eq!(models.child_model("general_agent"), "claude-sonnet-5-5");
}

/// M-1a: a template without `orchestration` borrowed as a lead (a second
/// concurrent workflow while the singleton `lead_agent` is busy:
/// `code_agent` on a stock store) whose choice is `inherit` runs on the
/// shipped lead's choice, `strong`, never on the main loop's model.
#[tokio::test]
async fn a_borrowed_lead_on_inherit_runs_on_strong() {
    let models = Models::new(vec![answer("all done", vec![])]);
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    let lead = instance(&shared, "code_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert_eq!(
        models.lead_model(),
        row(ProviderType::Anthropic, ModelAlias::Strong)
    );
}

/// M-1a with a set (AGENT-SETS): a set stored over `lead_agent` naming a
/// model carries a borrowed lead's `inherit` too — every workflow's lead on
/// one model, as `lead_agent`'s own run is. Added in C8's review; red with
/// the borrowed lead reading `lead_agent`'s bare template line.
#[tokio::test]
async fn a_set_over_lead_agent_carries_a_borrowed_lead_on_inherit_too() {
    let models = Models::new(vec![answer("all done", vec![])]);
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    shared.agent_registry.restore_set(
        "lead_agent",
        Some(serde_json::json!({ "llm": { "model": "claude-sonnet-5-5" } })),
    );
    assert_eq!(
        instance(&shared, "lead_agent").llm_config.model.as_deref(),
        Some("claude-sonnet-5-5"),
        "lead_agent's own run takes the set"
    );
    let lead = instance(&shared, "code_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert_eq!(models.lead_model(), "claude-sonnet-5-5");
}

/// M-1b: a template of the person's own with no `model:` line is `inherit`:
/// it runs on its lead's answering model (`strong`'s on a stock install),
/// where on the parent it ran on the router default.
#[tokio::test]
async fn a_persons_template_without_a_model_line_follows_its_lead() {
    let models = Models::new(single_lead(serde_json::json!({
        "agent_id": "my_agent",
        "objective": "work for my_agent"
    })));
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    let mine = crate::test_util::make_agent("my_agent", vec!["memory_read"]);
    assert!(mine.llm_config.model.is_none());
    shared
        .agent_registry
        .register_template(crate::test_util::template_from_agent(&mine));
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert_eq!(
        models.child_model("my_agent"),
        row(ProviderType::Anthropic, ModelAlias::Strong)
    );
}

/// M-6e: the lead's alias resolves before its loop config, so the window
/// and the context-budget telemetry read the model that will answer.
#[tokio::test]
async fn the_leads_alias_is_resolved_before_its_loop_config() {
    let models = Models::new(vec![answer("all done", vec![])]);
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    let bus = EventBus::default();
    let mut events = bus.subscribe();
    run(&lead, router, shared, bus).await;
    let mut budgeted = None;
    while let Ok(event) = events.try_recv() {
        if let crate::events::SystemEvent::ContextBudgetComputed { model, .. } = event {
            budgeted = Some(model);
            break;
        }
    }
    assert_eq!(
        budgeted.as_deref(),
        Some(row(ProviderType::Anthropic, ModelAlias::Strong).as_str())
    );
}

// ── A word or a per-call model drops the template's chain (M-6b) ────────────
//
// Added after C8 (`5e3b5078`), in the review of the commits after
// `02da85b9`: each of the three is red with its `fallback_models.clear()`
// taken out (`run_lead_agent`'s for the lead, `SpawnSubagentTool::execute`'s
// for a child, and that one's `chosen_per_call` for the per-call case). The
// shipped templates carry no chain, so each test's template is a person's
// own, or the shipped lead with one written in: a model the provider
// refuses walks the ladder, and with the chain dropped it reaches the main
// model, never the chain's `claude-sonnet-5-5`.

/// The main model of these three runs, and the rung a dropped chain leaves.
const MAIN: &str = "claude-haiku-5-5";
/// The chain each template carries: routable, neither the main model nor
/// the alias's row, so a run that walks it is told apart.
const CHAIN: &str = "claude-sonnet-5-5";

/// A lead naming `strong` that carries a chain: its model refused, it
/// answers on the main model and never asks the chain's.
#[tokio::test]
async fn a_lead_on_an_alias_does_not_walk_its_templates_chain() {
    let strong = row(ProviderType::Anthropic, ModelAlias::Strong);
    let models = Models::new(vec![answer("all done", vec![])]).refusing(&strong);
    let router = install(&models, &[ProviderType::Anthropic], MAIN);
    let shared = shipped();
    let mut lead = instance(&shared, "lead_agent");
    assert_eq!(lead.llm_config.model.as_deref(), Some("strong"));
    lead.llm_config.fallback_models = vec![CHAIN.to_string()];
    let result = run(&lead, router, shared, EventBus::default()).await;
    assert!(result.success, "{:?}", result.loop_result.finish_reason);
    let asked = models.models_asked(true);
    assert_eq!(asked.first(), Some(&strong), "the lead asked strong first");
    assert!(
        asked.iter().any(|m| m == MAIN),
        "the ladder's rung: {asked:?}"
    );
    assert!(
        !asked.iter().any(|m| m == CHAIN),
        "the template's chain is dropped: {asked:?}"
    );
}

/// A child whose template names `strong` and carries a chain: its model
/// refused, it answers on the main model and never asks the chain's.
#[tokio::test]
async fn a_child_on_an_alias_does_not_walk_its_templates_chain() {
    let strong = row(ProviderType::Anthropic, ModelAlias::Strong);
    let models = Models::new(single_lead(serde_json::json!({
        "agent_id": "my_agent",
        "objective": "work for my_agent"
    })))
    .refusing(&strong);
    let router = install(&models, &[ProviderType::Anthropic], MAIN);
    let shared = shipped();
    let mut mine = crate::test_util::make_agent("my_agent", vec!["memory_read"]);
    mine.llm_config.model = Some("strong".to_string());
    mine.llm_config.fallback_models = vec![CHAIN.to_string()];
    assert!(
        shared
            .agent_registry
            .register_template(crate::test_util::template_from_agent(&mine))
    );
    let mut lead = instance(&shared, "lead_agent");
    lead.llm_config.model = Some(MAIN.to_string());
    run(&lead, router, shared, EventBus::default()).await;
    let asked = models.models_asked(false);
    assert_eq!(asked.first(), Some(&strong), "the child asked strong first");
    assert!(
        asked.iter().any(|m| m == MAIN),
        "the ladder's rung: {asked:?}"
    );
    assert!(
        !asked.iter().any(|m| m == CHAIN),
        "the template's chain is dropped: {asked:?}"
    );
}

/// A per-call id on a template naming an id and carrying a chain: the
/// per-call model refused, the child answers on the main model and never
/// asks the chain's — the choice is not a word, so only the per-call rule
/// drops it.
#[tokio::test]
async fn a_child_on_a_per_call_id_does_not_walk_its_templates_chain() {
    let strong = row(ProviderType::Anthropic, ModelAlias::Strong);
    let models = Models::new(single_lead(serde_json::json!({
        "agent_id": "my_agent",
        "objective": "work for my_agent",
        "model": strong
    })))
    .refusing(&strong);
    let router = install(&models, &[ProviderType::Anthropic], MAIN);
    let shared = shipped();
    let mut mine = crate::test_util::make_agent("my_agent", vec!["memory_read"]);
    mine.llm_config.model = Some(MAIN.to_string());
    mine.llm_config.fallback_models = vec![CHAIN.to_string()];
    assert!(
        shared
            .agent_registry
            .register_template(crate::test_util::template_from_agent(&mine))
    );
    let mut lead = instance(&shared, "lead_agent");
    lead.llm_config.model = Some(MAIN.to_string());
    run(&lead, router, shared, EventBus::default()).await;
    let asked = models.models_asked(false);
    assert_eq!(
        asked.first(),
        Some(&strong),
        "the child asked its per-call id"
    );
    assert!(
        asked.iter().any(|m| m == MAIN),
        "the ladder's rung: {asked:?}"
    );
    assert!(
        !asked.iter().any(|m| m == CHAIN),
        "the template's chain is dropped: {asked:?}"
    );
}

// ── The per-call model (M-7, M-7a, M-7c, M-8, M-10) ─────────────────────────

/// M-7: a per-call alias resolves against the lead, outranks the template's
/// `model:`, and the child runs on it.
#[tokio::test]
async fn a_per_call_alias_resolves_against_the_lead() {
    let models = Models::new(single_lead(serde_json::json!({
        "agent_id": "explore_agent",
        "objective": "work for explore_agent",
        "model": "strong"
    })));
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert_eq!(
        models.child_model("explore_agent"),
        row(ProviderType::Anthropic, ModelAlias::Strong)
    );
}

/// M-7: a per-call model id that is routable is used as asked.
#[tokio::test]
async fn a_routable_per_call_model_is_used() {
    let models = Models::new(single_lead(serde_json::json!({
        "agent_id": "general_agent",
        "objective": "work for general_agent",
        "model": "claude-fable-5-2"
    })));
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert_eq!(models.child_model("general_agent"), "claude-fable-5-2");
}

/// M-7: a per-call model that is not routable is refused before anything
/// starts, never substituted, and the refusal lists what can be chosen.
#[tokio::test]
async fn a_per_call_model_that_is_not_routable_is_refused_with_the_routable_list() {
    let models = Models::new(single_lead(serde_json::json!({
        "agent_id": "general_agent",
        "objective": "work for general_agent",
        "model": "gpt-9"
    })));
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert!(
        models.seen().iter().all(|s| s.lead),
        "no subagent ran: {:?}",
        models
            .seen()
            .iter()
            .map(|s| s.last_user.clone())
            .collect::<Vec<_>>()
    );
    let refusal = &models.tool_results()[0];
    let strong = row(ProviderType::Anthropic, ModelAlias::Strong);
    assert!(
        refusal.contains(&format!(
            "model 'gpt-9' is not routable on this install. Routable now: \
             claude-fable-5-2, claude-haiku-5-5, claude-opus-5-5, claude-sonnet-5-5. \
             Aliases now: fast → {}, balanced → {}, strong → {strong}. \
             Leave model out to use the template's.",
            row(ProviderType::Anthropic, ModelAlias::Fast),
            row(ProviderType::Anthropic, ModelAlias::Balanced),
        )),
        "{refusal}"
    );
}

/// M-7: a per-call `model` that is not a string is refused and spawns
/// nothing — on `spawn_subagent` by the loop's check of the tool's schema,
/// and in a batch item, which that check does not reach, by the tool's own
/// refusal, that item's FAILED line. Added in C8's review (a path no test
/// reached).
#[tokio::test]
async fn a_per_call_model_that_is_not_a_string_is_refused() {
    let models = Models::new(single_lead(serde_json::json!({
        "agent_id": "general_agent",
        "objective": "work for general_agent",
        "model": 5
    })));
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert!(models.seen().iter().all(|s| s.lead), "no subagent ran");
    assert!(
        models.tool_results()[0].contains("parameter 'model' should be string"),
        "{}",
        models.tool_results()[0]
    );

    let models = Models::new(batch_lead(serde_json::json!([
        { "agent_id": "general_agent", "objective": "work for general_agent", "model": 5 }
    ])));
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert!(models.seen().iter().all(|s| s.lead), "no subagent ran");
    assert!(
        models.tool_results()[0].contains(
            "[1] FAILED (general_agent): model must be a string — inherit, fast, balanced, \
             strong or a model id — or left out for the template's model"
        ),
        "{}",
        models.tool_results()[0]
    );
}

/// M-7c: a batch item's `model` reaches its child, and an item whose model
/// is not routable is that item's FAILED line while the batch goes on.
#[tokio::test]
async fn a_batch_items_model_reaches_its_child_and_an_unroutable_one_fails_that_item_only() {
    let models = Models::new(batch_lead(serde_json::json!([
        { "agent_id": "explore_agent", "objective": "work for explore_agent", "model": "balanced" },
        { "agent_id": "review_agent", "objective": "work for review_agent", "model": "gpt-9" }
    ])));
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert_eq!(
        models.child_model("explore_agent"),
        row(ProviderType::Anthropic, ModelAlias::Balanced)
    );
    let summary = &models.tool_results()[0];
    assert!(
        summary.contains("Batch spawn: 1/2 subagents spawned successfully."),
        "{summary}"
    );
    assert!(
        summary.contains("[2] FAILED (review_agent): model 'gpt-9' is not routable"),
        "{summary}"
    );
}

/// M-7a, §7 O-7 (a): a per-call model never lifts the template's cap —
/// `explore_agent` asked for `strong` still stops at its own $0.25, priced
/// on the model that answered. On the parent the `model` argument was not
/// read: the child ran on its template's model, cheap enough to finish.
#[tokio::test]
async fn a_per_call_strong_model_still_stops_at_the_templates_cap() {
    let mut costly = answer(
        "",
        vec![call("workspace_read", serde_json::json!({"key": "k"}))],
    );
    costly.usage = Usage {
        input_tokens: 10,
        output_tokens: 100_000,
        ..Default::default()
    };
    let models = Models::new(single_lead(serde_json::json!({
        "agent_id": "explore_agent",
        "objective": "work for explore_agent",
        "model": "strong"
    })))
    .child("work for explore_agent", vec![costly]);
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    let waited = models.tool_results().join("\n");
    assert!(
        waited.contains("of $0.25"),
        "the child stopped at its template's cap: {waited}"
    );
}

/// M-10: the spawn's result names the model the child runs on, what chose
/// it, and that the lead stays on its own.
#[tokio::test]
async fn the_spawn_result_names_the_model_and_that_the_lead_stays() {
    let models = Models::new(batch_lead(serde_json::json!([
        { "agent_id": "explore_agent", "objective": "work for explore_agent" },
        { "agent_id": "general_agent", "objective": "work for general_agent" },
        { "agent_id": "review_agent", "objective": "work for review_agent", "model": "strong" }
    ])));
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    let strong = row(ProviderType::Anthropic, ModelAlias::Strong);
    let fast = row(ProviderType::Anthropic, ModelAlias::Fast);
    let summary = &models.tool_results()[0];
    for expected in [
        format!(") on {fast} (its template's fast; you stay on {strong}),"),
        format!(") on {strong} (your model),"),
        format!(") on {strong} (as you asked: strong; you stay on {strong}),"),
    ] {
        assert!(summary.contains(&expected), "{expected} in {summary}");
    }
}

/// C8-2 (c), the advisor's pick the owner took on 2026-11-17: a model the
/// agent's own model constraints deny — here `general_agent`'s set denies
/// the lead's model, which it would inherit, and a per-call one — is
/// still accepted (the refusal, (b), is the owner's and not taken), and
/// the child runs on the router default as before; the spawn's reply and
/// the lane's `subagent_open` now name that model, saying it replaced the
/// one chosen. An allowed per-call model still names itself. Where the
/// default the loop reaches is denied too (here `writing_agent`'s set
/// denies the router default it asks for), no replacement is claimed and,
/// since C8-2-REPLY, the reply says the child will be refused at its first
/// round and `subagent_open` carries `model_refused`. On the parent the
/// reply named the denied model as the one the child ran on.
#[tokio::test]
async fn a_model_the_agents_constraints_deny_is_named_as_replaced_by_the_one_used() {
    use crate::session_log::{SessionLogLimits, SessionLogService, read_records};

    let strong = row(ProviderType::Anthropic, ModelAlias::Strong);
    let models = Models::new(batch_lead(serde_json::json!([
        { "agent_id": "general_agent", "objective": "work for general_agent" },
        {
            "agent_id": "general_agent",
            "objective": "asked work for general_agent",
            "model": "claude-fable-5-2"
        },
        {
            "agent_id": "review_agent",
            "objective": "work for review_agent",
            "model": "claude-sonnet-5-5"
        },
        {
            "agent_id": "writing_agent",
            "objective": "work for writing_agent",
            "model": "claude-haiku-5-5"
        }
    ])));
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    shared.agent_registry.restore_set(
        "general_agent",
        Some(serde_json::json!({
            "constraints": { "denied_models": [strong.clone(), "claude-fable-5-2"] }
        })),
    );
    shared.agent_registry.restore_set(
        "writing_agent",
        Some(serde_json::json!({
            "constraints": { "denied_models": ["claude-haiku-5-5"] }
        })),
    );
    let dir = tempfile::tempdir().unwrap();
    let service = SessionLogService::new(
        dir.path().to_path_buf(),
        None,
        SessionLogLimits::default(),
        "test".to_string(),
    );
    let handle = service.handle_for("sess-lead");
    shared.register_task_session_log("task-1", handle.clone());
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;

    // What the children ran on: the router default for the denied ones,
    // the asked model for the allowed one — unchanged from the parent.
    assert_eq!(models.lead_model(), strong);
    assert_eq!(models.child_model("asked work for"), "claude-haiku-5-5");
    let inherited = models
        .seen()
        .into_iter()
        .find(|s| {
            !s.lead
                && s.last_user.contains("work for general_agent")
                && !s.last_user.contains("asked work")
        })
        .and_then(|s| s.model)
        .expect("the inheriting child made a request");
    assert_eq!(inherited, "claude-haiku-5-5");
    assert_eq!(models.child_model("review_agent"), "claude-sonnet-5-5");

    let summary = &models.tool_results()[0];
    for expected in [
        format!(
            ") on claude-haiku-5-5 (it replaced {strong}, your model, which this agent's model \
             constraints deny; you stay on {strong}),"
        ),
        format!(
            ") on claude-haiku-5-5 (it replaced claude-fable-5-2, as you asked, which this \
             agent's model constraints deny; you stay on {strong}),"
        ),
        format!(") on claude-sonnet-5-5 (as you asked: claude-sonnet-5-5; you stay on {strong}),"),
        // C8-2-REPLY: the default the loop reaches is denied too.
        format!(
            ") to be refused at its first round (claude-haiku-5-5, as you asked and the router \
             default, is a model this agent's model constraints deny; you stay on {strong}),"
        ),
    ] {
        assert!(summary.contains(&expected), "{expected} in {summary}");
    }
    assert!(
        !summary.contains(&format!(") on {strong} (your model),")),
        "the denied model is not named as the one used: {summary}"
    );

    assert!(handle.flush().await);
    let records = read_records(&dir.path().join("sess-lead")).unwrap();
    let opens: Vec<_> = records
        .iter()
        .filter(|r| r.kind == "subagent_open")
        .collect();
    assert_eq!(opens.len(), 4, "four lanes opened");
    let by_objective = |key: &str| {
        opens
            .iter()
            .find(|r| {
                r.data["objective"]
                    .as_str()
                    .is_some_and(|o| o.starts_with(key))
            })
            .unwrap_or_else(|| panic!("no lane for {key}"))
    };
    let inherited = by_objective("work for general_agent");
    assert_eq!(inherited.data["model"], "claude-haiku-5-5");
    assert_eq!(inherited.data["model_replaced"], strong.as_str());
    let asked = by_objective("asked work for general_agent");
    assert_eq!(asked.data["model"], "claude-haiku-5-5");
    assert_eq!(asked.data["model_replaced"], "claude-fable-5-2");
    let allowed = by_objective("work for review_agent");
    assert_eq!(allowed.data["model"], "claude-sonnet-5-5");
    assert!(
        allowed.data.get("model_replaced").is_none(),
        "only a replacement says so: {}",
        allowed.data
    );
    let denied_default = by_objective("work for writing_agent");
    assert_eq!(denied_default.data["model"], "claude-haiku-5-5");
    assert!(
        denied_default.data.get("model_replaced").is_none(),
        "no replacement where the default is denied too: {}",
        denied_default.data
    );
    assert_eq!(denied_default.data["model_refused"], true);
    for lane in [inherited, asked, allowed] {
        assert!(lane.data.get("model_refused").is_none(), "{}", lane.data);
    }
}

/// The batch of [`a_child_whose_constraints_deny_the_model_it_reaches_is_refused_at_its_first_round`]
/// and [`the_spawn_says_a_child_the_constraints_deny_everywhere_will_be_refused`]:
/// `writing_agent` asked onto the router default its set denies, and
/// `review_agent` inheriting the lead's model on a set whose
/// `allowed_models` hold neither it nor the router default — the two shapes
/// in which no model the agent may use is reachable (C8-2-REPLY).
fn denied_everywhere(strong: &str) -> (Arc<Models>, Arc<LlmRouter>, Arc<SharedContext>) {
    let models = Models::new(batch_lead(serde_json::json!([
        {
            "agent_id": "writing_agent",
            "objective": "work for writing_agent",
            "model": "claude-haiku-5-5"
        },
        { "agent_id": "review_agent", "objective": "work for review_agent" }
    ])));
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    shared.agent_registry.restore_set(
        "writing_agent",
        Some(serde_json::json!({
            "constraints": { "denied_models": ["claude-haiku-5-5"] }
        })),
    );
    assert_ne!(
        strong, "claude-sonnet-5-5",
        "the lead's model is not allowed"
    );
    shared.agent_registry.restore_set(
        "review_agent",
        Some(serde_json::json!({
            "constraints": { "allowed_models": ["claude-sonnet-5-5"] }
        })),
    );
    (models, router, shared)
}

/// C8-2-REPLY's pin, written before its wording (the advisor's pick (b) on
/// Rev 127's item 12, taken 2026-11-17): where an agent's model constraints
/// deny the model chosen for it *and* the router default its loop is sent
/// to in its place, the child is still spawned (no refusal is added at the
/// spawn: option (c), the owner's), sends one request on that default, and
/// its loop refuses the answer at its first round ("Model access denied");
/// the lead reads that failure in `wait_for_subagents`. Green on the
/// parent: the behaviour the reply now describes, unchanged.
#[tokio::test]
async fn a_child_whose_constraints_deny_the_model_it_reaches_is_refused_at_its_first_round() {
    let strong = row(ProviderType::Anthropic, ModelAlias::Strong);
    let (models, router, shared) = denied_everywhere(&strong);
    let lead = instance(&shared, "lead_agent");
    let result = run(&lead, router, shared, EventBus::default()).await;
    assert!(result.success, "{:?}", result.loop_result.finish_reason);

    for key in ["work for writing_agent", "work for review_agent"] {
        let sent: Vec<Option<String>> = models
            .seen()
            .into_iter()
            .filter(|s| !s.lead && s.last_user.contains(key))
            .map(|s| s.model)
            .collect();
        assert_eq!(
            sent,
            vec![Some("claude-haiku-5-5".to_string())],
            "{key}: one request, on the router default, then nothing"
        );
    }
    let results = models.tool_results();
    let waited = &results[1];
    assert!(
        waited.contains("0 completed, 2 failed"),
        "both children failed: {waited}"
    );
    for (agent, reason) in [
        ("writing_agent", "is on the deny list"),
        ("review_agent", "is not in the allow list"),
    ] {
        let line = waited
            .lines()
            .find(|l| l.starts_with(&format!("- **{agent}::")))
            .unwrap_or_else(|| panic!("no line for {agent}: {waited}"));
        assert!(
            line.contains(&format!(
                "failed — Agent '{agent}' could not finish: Model access denied"
            )) && line.contains("model='claude-haiku-5-5'")
                && line.contains(reason),
            "{agent} was refused at its first round on the router default: {line}"
        );
    }
}

/// C8-2-REPLY (Rev 127's item 12, the advisor's pick (b), taken
/// 2026-11-17): in the two shapes the pin above shows refused, the spawn's
/// reply says that the agent's model constraints deny the model chosen,
/// and the default it would run on in its place, and that the child will
/// be refused at its first round; `subagent_open` names the model it is
/// sent to, `model_replaced` the chosen one where they differ, and
/// `model_refused` says so. On the parent the reply said the child ran on
/// the denied model, "(your model)" for the inheriting one.
#[tokio::test]
async fn the_spawn_says_a_child_the_constraints_deny_everywhere_will_be_refused() {
    use crate::session_log::{SessionLogLimits, SessionLogService, read_records};

    let strong = row(ProviderType::Anthropic, ModelAlias::Strong);
    let (models, router, shared) = denied_everywhere(&strong);
    let dir = tempfile::tempdir().unwrap();
    let service = SessionLogService::new(
        dir.path().to_path_buf(),
        None,
        SessionLogLimits::default(),
        "test".to_string(),
    );
    let handle = service.handle_for("sess-lead");
    shared.register_task_session_log("task-1", handle.clone());
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;

    let summary = &models.tool_results()[0];
    for expected in [
        format!(
            ") to be refused at its first round (claude-haiku-5-5, as you asked and the router \
             default, is a model this agent's model constraints deny; you stay on {strong}),"
        ),
        format!(
            ") to be refused at its first round ({strong}, your model, is a model this agent's \
             model constraints deny, and so is claude-haiku-5-5, the router default it would \
             run on in its place; you stay on {strong}),"
        ),
    ] {
        assert!(summary.contains(&expected), "{expected} in {summary}");
    }
    assert!(
        !summary.contains(&format!(") on {strong} (your model),")),
        "the denied model is not named as the one used: {summary}"
    );

    assert!(handle.flush().await);
    let records = read_records(&dir.path().join("sess-lead")).unwrap();
    let open = |key: &str| {
        records
            .iter()
            .find(|r| {
                r.kind == "subagent_open"
                    && r.data["objective"]
                        .as_str()
                        .is_some_and(|o| o.starts_with(key))
            })
            .unwrap_or_else(|| panic!("no lane for {key}"))
            .data
            .clone()
    };
    let asked = open("work for writing_agent");
    assert_eq!(asked["model"], "claude-haiku-5-5");
    assert!(asked.get("model_replaced").is_none(), "{asked}");
    assert_eq!(asked["model_refused"], true);
    let inherited = open("work for review_agent");
    assert_eq!(inherited["model"], "claude-haiku-5-5");
    assert_eq!(inherited["model_replaced"], strong.as_str());
    assert_eq!(inherited["model_refused"], true);
}

/// C8-2-REPLY (Rev 127's item 15, the advisor's pick (b), taken
/// 2026-11-17): a child whose constraints deny the model chosen for it is
/// budgeted on the window of the model it is sent to, not of the denied
/// one: its `round` record's `context.window` is the router default's.
/// Here the default's window is set to half the denied model's, so the
/// two cannot coincide. On the parent the lane was sized on the denied
/// model's window.
#[tokio::test]
async fn a_denied_childs_window_is_the_one_of_the_model_it_runs_on() {
    use crate::session_log::{SessionLogLimits, SessionLogService, read_records};

    let strong = row(ProviderType::Anthropic, ModelAlias::Strong);
    let models = Models::new(batch_lead(serde_json::json!([
        { "agent_id": "general_agent", "objective": "work for general_agent" }
    ])));
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let registry = router.model_registry();
    let denied_window = registry.get_model_info(&strong).unwrap().context_window;
    let mut info = registry.get_model_info("claude-haiku-5-5").unwrap();
    info.context_window = denied_window / 2;
    registry.register("claude-haiku-5-5".to_string(), info);
    let used_window = denied_window / 2;
    assert!(used_window > 0 && used_window != denied_window);
    let shared = shipped();
    shared.agent_registry.restore_set(
        "general_agent",
        Some(serde_json::json!({
            "constraints": { "denied_models": [strong.clone()] }
        })),
    );
    let dir = tempfile::tempdir().unwrap();
    let service = SessionLogService::new(
        dir.path().to_path_buf(),
        None,
        SessionLogLimits::default(),
        "test".to_string(),
    );
    let handle = service.handle_for("sess-lead");
    shared.register_task_session_log("task-1", handle.clone());
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert_eq!(
        models.child_model("work for general_agent"),
        "claude-haiku-5-5"
    );

    assert!(handle.flush().await);
    let records = read_records(&dir.path().join("sess-lead")).unwrap();
    let child_round = records
        .iter()
        .find(|r| {
            r.kind == "round"
                && r.agent
                    .as_deref()
                    .is_some_and(|a| a.starts_with("general_agent"))
        })
        .expect("the child answered a round");
    assert_eq!(child_round.data["model"], "claude-haiku-5-5");
    assert_eq!(
        child_round.data["context"]["window"],
        serde_json::json!(used_window),
        "sized on the model used, not on {strong}'s {denied_window}"
    );
}

/// M-8: the `model` parameter of both spawn tools names what each alias is
/// here, the routable ids and the lead's own model; the choices are in the
/// parameter and nowhere else (§7 O-8 (a)).
#[tokio::test]
async fn the_model_parameter_describes_the_aliases_and_the_routable_ids() {
    let models = Models::new(vec![answer("all done", vec![])]);
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    let strong = row(ProviderType::Anthropic, ModelAlias::Strong);
    let expected = format!(
        "Run this subagent on another model than its template's. Here fast is {}, balanced \
         {}, strong {strong}; or one of: claude-fable-5-2, claude-haiku-5-5, claude-opus-5-5, \
         claude-sonnet-5-5. Omit it to use the template's model — most templates inherit \
         yours, {strong}. The template's cost cap still applies.",
        row(ProviderType::Anthropic, ModelAlias::Fast),
        row(ProviderType::Anthropic, ModelAlias::Balanced),
    );
    let tools = models.lead_request(0).tools;
    let single = tools.iter().find(|t| t.name == "spawn_subagent").unwrap();
    assert_eq!(
        single.parameters["properties"]["model"]["description"],
        serde_json::json!(expected)
    );
    let batch = tools
        .iter()
        .find(|t| t.name == "spawn_subagents_batch")
        .unwrap();
    assert_eq!(
        batch.parameters["properties"]["subagents"]["items"]["properties"]["model"]["description"],
        serde_json::json!(expected)
    );
    assert!(single.strict.is_none() && batch.strict.is_none(), "M-9");
}

/// M-8 for an edited `lead_agent.md` still naming an id the install cannot
/// route: the menu is built against the model the lead answers on (the
/// ladder's), which its children inherit and its aliases resolve against,
/// not against the unroutable id. Added in C8's review; red with the menu
/// built against the lead's resolved id.
#[tokio::test]
async fn an_edited_leads_menu_names_the_model_the_lead_answers_on() {
    let models = Models::new(vec![answer("all done", vec![])]);
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    let mut lead = instance(&shared, "lead_agent");
    lead.llm_config.model = Some("claude-sonnet-4-6".to_string());
    run(&lead, router, shared, EventBus::default()).await;
    let expected = format!(
        "Run this subagent on another model than its template's. Here fast is {}, balanced \
         {}, strong {}; or one of: claude-fable-5-2, claude-haiku-5-5, claude-opus-5-5, \
         claude-sonnet-5-5. Omit it to use the template's model — most templates inherit \
         yours, claude-haiku-5-5. The template's cost cap still applies.",
        row(ProviderType::Anthropic, ModelAlias::Fast),
        row(ProviderType::Anthropic, ModelAlias::Balanced),
        row(ProviderType::Anthropic, ModelAlias::Strong),
    );
    let tools = models.lead_request(0).tools;
    let single = tools.iter().find(|t| t.name == "spawn_subagent").unwrap();
    assert_eq!(
        single.parameters["properties"]["model"]["description"],
        serde_json::json!(expected)
    );
}

/// M-6c: an Ollama-only install — every shipped template runs on the local
/// main model (step 3, the collapse), and the router's substitution warning
/// is no longer logged for them: an alias or `inherit` resolves to a
/// routable model before the request, so the ladder is never walked. On the
/// parent every template's retired Claude id walked it, each pair noted.
#[tokio::test]
async fn every_shipped_template_answers_on_the_local_model_on_an_ollama_only_install_unannounced() {
    let models = Models::new(batch_lead(every_worker()));
    let router = install(&models, &[ProviderType::Ollama], "qwen3:8b");
    pulled(&router, "qwen3:8b");
    every_template_on(&models, router.clone()).await;
    assert_eq!(models.lead_model(), "qwen3:8b");
    for id in WORKERS {
        assert_eq!(models.child_model(id), "qwen3:8b", "{id}");
    }
    assert_eq!(router.noted_substitutions(), Vec::<(String, String)>::new());
    // M-5: no collapse line either — no other loaded provider has a model
    // for an alias (added in C8's review).
    assert_eq!(
        router.noted_alias_collapses(),
        Vec::<(String, String)>::new()
    );
}

/// M-6d: an **edited** override still naming a model id the install cannot
/// route keeps working — the id is not checked at load, and at the call it
/// walks the ladder, announced, as before C8.
#[tokio::test]
async fn an_edited_override_pinning_a_claude_id_still_runs_on_an_ollama_install() {
    let models = Models::new(single_lead(serde_json::json!({
        "agent_id": "my_coder",
        "objective": "work for my_coder"
    })));
    let router = install(&models, &[ProviderType::Ollama], "qwen3:8b");
    pulled(&router, "qwen3:8b");
    let shared = shipped();
    let mut mine = crate::test_util::make_agent("my_coder", vec!["memory_read"]);
    mine.llm_config.model = Some("claude-sonnet-4-6".to_string());
    shared
        .agent_registry
        .register_template(crate::test_util::template_from_agent(&mine));
    let lead = instance(&shared, "lead_agent");
    run(&lead, router.clone(), shared, EventBus::default()).await;
    assert_eq!(models.child_model("my_coder"), "qwen3:8b");
    assert!(
        router
            .noted_substitutions()
            .contains(&("claude-sonnet-4-6".to_string(), "qwen3:8b".to_string())),
        "announced: {:?}",
        router.noted_substitutions()
    );
}

/// M-7: the refusal's list is capped — forty routable ids give the first
/// thirty, sorted, then "and 10 more"; on a one-model local install every
/// alias is that model.
#[tokio::test]
async fn a_per_call_refusal_lists_thirty_routable_ids_and_how_many_more() {
    let models = Models::new(single_lead(serde_json::json!({
        "agent_id": "general_agent",
        "objective": "work for general_agent",
        "model": "gpt-9"
    })));
    let router = install(&models, &[ProviderType::Ollama], "m00");
    let tags: Vec<String> = (0..40).map(|i| format!("m{i:02}")).collect();
    for tag in &tags {
        pulled(&router, tag);
    }
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    let refusal = &models.tool_results()[0];
    assert!(
        refusal.contains(&format!(
            "model 'gpt-9' is not routable on this install. Routable now: {}, and 10 more. \
             Aliases now: fast → m00, balanced → m00, strong → m00. Leave model out to use \
             the template's.",
            tags[..30].join(", ")
        )),
        "{refusal}"
    );
}

/// A plugin's agent that completes at once.
struct PluginAgent;

#[async_trait]
impl openalpaca_api::plugin_traits::PluginAgentExecutor for PluginAgent {
    async fn spawn(
        &self,
        _instance_id: &str,
        _task_id: &str,
        _instructions: &str,
        _context: &serde_json::Value,
    ) -> Result<bool, String> {
        Ok(true)
    }

    async fn step(
        &self,
        _instance_id: &str,
        _tool_results: Option<&serde_json::Value>,
    ) -> Result<(String, String, Vec<serde_json::Value>), String> {
        Ok(("complete".to_string(), "plugin result".to_string(), vec![]))
    }

    async fn stop(&self, _instance_id: &str) -> Result<(), String> {
        Ok(())
    }

    fn plugin_id(&self) -> &str {
        "test_plugin"
    }

    fn agent_id(&self) -> &str {
        "plugin_researcher"
    }
}

/// M-7d: a plugin-backed template runs out of process on its plugin's own
/// model, so a per-call `model` for it is refused rather than accepted and
/// honoured by nothing.
#[tokio::test]
async fn a_per_call_model_on_a_plugin_template_is_refused() {
    let models = Models::new(single_lead(serde_json::json!({
        "agent_id": "plugin_researcher",
        "objective": "work for plugin_researcher",
        "model": "strong"
    })));
    let router = install(&models, &[ProviderType::Anthropic], "claude-haiku-5-5");
    let shared = shipped();
    let mut template = crate::test_util::template_from_agent(&crate::test_util::make_agent(
        "plugin_researcher",
        vec![],
    ));
    template.source = crate::agent::template::AgentSource::Plugin {
        plugin_id: "test_plugin".to_string(),
        executor: Arc::new(PluginAgent),
    };
    assert!(shared.agent_registry.register_template(template));
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert!(
        models.tool_results()[0].contains(
            "'plugin_researcher' runs out of process on its plugin's own model; leave model out."
        ),
        "{}",
        models.tool_results()[0]
    );
}

/// A provider that answers nothing.
struct Refuses;

#[async_trait]
impl LlmProvider for Refuses {
    fn name(&self) -> &str {
        "refuses"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn requires_key(&self) -> bool {
        false
    }

    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Err(LlmError::NotConfigured)
    }
}

/// M-6e: a lead that ends before any round answered (`model_used: None`)
/// still hands its dispatcher the id its choice resolved to — what the
/// usage row and the completion report name — never its template's word.
#[tokio::test]
async fn a_lead_ending_with_no_answering_model_reports_a_model_id_not_an_alias() {
    let router = Arc::new(LlmRouter::single_provider(
        Arc::new(Refuses),
        ProviderType::Anthropic,
        "claude-haiku-5-5".to_string(),
    ));
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    assert_eq!(lead.llm_config.model.as_deref(), Some("strong"));
    let result = run(&lead, router, shared, EventBus::default()).await;
    assert!(result.loop_result.model_used.is_none(), "no round answered");
    assert_eq!(
        result.lead_model,
        Some(row(ProviderType::Anthropic, ModelAlias::Strong))
    );
}

/// M-6c (a characterization, green on the parent too): with Ollama as the
/// main model and OpenAI also loaded, workflows stay on the local model —
/// no alias steps to OpenAI's row by itself (M-4) — said once at `INFO` for
/// each alias a run used (`strong`, the lead's; `fast`, `explore_agent`'s),
/// never for one only the spawn tools' menu listed (`balanced`). The
/// collapse assertion was added in C8's review, red with the menu resolving
/// the aliases as a run does.
#[tokio::test]
async fn on_ollama_main_with_openai_loaded_workflows_stay_on_the_local_model() {
    let models = Models::new(batch_lead(serde_json::json!([
        { "agent_id": "explore_agent", "objective": "work for explore_agent" },
        { "agent_id": "general_agent", "objective": "work for general_agent" }
    ])));
    let router = install(
        &models,
        &[ProviderType::OpenAI, ProviderType::Ollama],
        "qwen3:8b",
    );
    pulled(&router, "qwen3:8b");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router.clone(), shared, EventBus::default()).await;
    assert_eq!(models.lead_model(), "qwen3:8b");
    assert_eq!(models.child_model("explore_agent"), "qwen3:8b");
    assert_eq!(models.child_model("general_agent"), "qwen3:8b");
    assert_eq!(
        router.noted_alias_collapses(),
        vec![
            ("fast".to_string(), "ollama".to_string()),
            ("strong".to_string(), "ollama".to_string()),
        ]
    );
}

// ── An OpenAI-compatible server behind [providers.openai] (COMPAT-SERVER) ───
//
// The owner's answer of 2026-11-15 to pending-decisions §1 *Rev 86*'s first
// item, (a): OpenAI's compiled rows count only where the `openai` provider
// calls OpenAI's own API. The provider here stands in for one built to call
// a llama.cpp-like server on loopback (`Models::at`, nothing is sent there),
// whose discovery listed one model of its own; `llm.toml` names the same
// `base_url`, so what it serves is priced as a local server's (item 17).

/// The compatible server's address; never reached.
const COMPAT_BASE: &str = "http://127.0.0.1:1/v1";
/// The one model the compatible server listed.
const SERVED: &str = "qwen3-32b";
/// The window a lead is budgeted at when its model's row gives none.
const DEFAULT_WINDOW: usize = 200_000;

/// A model the `openai` provider's discovery listed, as it registers one: no
/// price, no `[models]` row, no window (OpenAI's model list gives none).
fn listed(router: &LlmRouter, id: &str) {
    router.model_registry().register_discovered(
        id.to_string(),
        ModelInfo {
            provider: ProviderType::OpenAI,
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
            request_contract: openalpaca_llm::RequestContract::NONE,
        },
    );
}

/// An OpenAI-only install whose `[providers.openai] base_url` is `base`
/// (`own_api`: one of OpenAI's own hosts), with the seeded main model,
/// `extra` added to `llm.toml`, and [`SERVED`] discovered.
fn openai_install_at(
    models: &Arc<Models>,
    base: &str,
    own_api: bool,
    extra: &str,
) -> Arc<LlmRouter> {
    models.at(base, own_api);
    let router = install(models, &[ProviderType::OpenAI], "gpt-6-luna");
    load(
        &router,
        &format!("[providers.openai]\nbase_url = \"{base}\"\n{extra}"),
    );
    listed(&router, SERVED);
    router
}

/// The window the lead's loop was sized to, from its budget event.
fn budgeted_window(
    events: &mut tokio::sync::broadcast::Receiver<crate::events::SystemEvent>,
) -> Option<usize> {
    while let Ok(event) = events.try_recv() {
        if let crate::events::SystemEvent::ContextBudgetComputed { window_size, .. } = event {
            return Some(window_size);
        }
    }
    None
}

/// On a compatible server the lead (`strong`), the two `fast` profiles and
/// the six on `inherit` all run on the server's own model, at the loopback
/// rule's 0, unannounced (M-5: no other loaded provider has a model for an
/// alias); the spawn tools' menu offers only that model. The lead is
/// budgeted at the 200,000-token default, since the listed row gives no
/// window. On the parent the lead and its six asked the server for
/// `gpt-6.1-sol` and the two `fast` agents for `gpt-6-luna`, counted at
/// their rates and sized to `gpt-6.1-sol`'s window.
#[tokio::test]
async fn on_a_compatible_openai_server_every_shipped_template_runs_on_its_own_model() {
    let models = Models::new(batch_lead(every_worker()));
    let router = openai_install_at(&models, COMPAT_BASE, false, "");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    let bus = EventBus::default();
    let mut events = bus.subscribe();
    let result = run(&lead, router.clone(), shared, bus).await;
    assert!(result.success, "{:?}", result.loop_result.finish_reason);
    assert!(models.tool_results()[0].contains("8/8 subagents spawned"));

    assert_eq!(models.lead_model(), SERVED);
    for id in WORKERS {
        assert_eq!(models.child_model(id), SERVED, "{id}");
    }
    let asked: Vec<String> = models
        .models_asked(true)
        .into_iter()
        .chain(models.models_asked(false))
        .collect();
    assert!(asked.iter().all(|m| m == SERVED), "{asked:?}");

    let spent = router
        .cost_tracker
        .get_task_usage("task-1")
        .await
        .expect("the run was counted");
    assert_eq!(spent.total_cost_usd, 0.0, "the loopback rule's 0");
    assert_eq!(budgeted_window(&mut events), Some(DEFAULT_WINDOW));
    assert_eq!(router.noted_substitutions(), Vec::<(String, String)>::new());
    assert_eq!(
        router.noted_alias_collapses(),
        Vec::<(String, String)>::new()
    );

    let menu = format!(
        "Run this subagent on another model than its template's. Here fast, balanced and \
         strong are all {SERVED}; or one of: {SERVED}. Omit it to use the template's model \
         — most templates inherit yours, {SERVED}. The template's cost cap still applies."
    );
    let tools = models.lead_request(0).tools;
    let single = tools.iter().find(|t| t.name == "spawn_subagent").unwrap();
    assert_eq!(
        single.parameters["properties"]["model"]["description"],
        serde_json::json!(menu)
    );
}

/// A `[models."qwen3-32b"]` row with `context` sizes the lead on a
/// compatible server: the row the person wrote gives the window that the
/// server's list does not.
#[tokio::test]
async fn a_models_row_s_context_sizes_the_lead_on_a_compatible_server() {
    let models = Models::new(batch_lead(every_worker()));
    let router = openai_install_at(&models, COMPAT_BASE, false, "");
    let sized: HashMap<String, openalpaca_llm::ModelConfigEntry> = toml::from_str(&format!(
        "[\"{SERVED}\"]\nprovider = \"openai\"\ncontext = 32768\n"
    ))
    .expect("a [models] row parses");
    router
        .model_registry()
        .reload_from_config(&sized, &HashSet::new());
    listed(&router, SERVED);
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    let bus = EventBus::default();
    let mut events = bus.subscribe();
    run(&lead, router, shared, bus).await;
    assert_eq!(models.lead_model(), SERVED);
    assert_eq!(budgeted_window(&mut events), Some(32_768));
}

/// The same install on OpenAI's own API (its default `base_url`) keeps the
/// compiled rows: the lead and its five on `strong`'s, the three `fast`
/// templates on `fast`'s, though the provider's discovery listed another
/// model too. A guard: green on the parent as well.
#[tokio::test]
async fn on_openai_s_own_api_the_shipped_templates_keep_openai_s_rows() {
    let models = Models::new(batch_lead(every_worker()));
    let router = openai_install_at(&models, "https://api.openai.com/v1", true, "");
    every_template_on(&models, router).await;

    let strong = row(ProviderType::OpenAI, ModelAlias::Strong);
    let fast = row(ProviderType::OpenAI, ModelAlias::Fast);
    assert_eq!(models.lead_model(), strong);
    for id in WORKERS {
        let expected = if ON_FAST.contains(&id) {
            &fast
        } else {
            &strong
        };
        assert_eq!(&models.child_model(id), expected, "{id}");
    }
}

/// A pin under `[providers.openai.model_aliases]` still wins on a
/// compatible server (M-4 step 1): the lead and the six that follow it run
/// on the pinned model, and `explore_agent`'s `fast`, unpinned, collapses
/// to its parent's, the same model. The pin is a guard, green on the
/// parent as well; the `fast` line is red there (`gpt-6-luna`).
#[tokio::test]
async fn a_strong_pin_still_wins_on_a_compatible_openai_server() {
    let models = Models::new(batch_lead(serde_json::json!([
        { "agent_id": "explore_agent", "objective": "work for explore_agent" },
        { "agent_id": "general_agent", "objective": "work for general_agent" }
    ])));
    let router = openai_install_at(
        &models,
        COMPAT_BASE,
        false,
        "[providers.openai.model_aliases]\nstrong = \"qwen3-coder-30b\"\n",
    );
    listed(&router, "qwen3-coder-30b");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert_eq!(models.lead_model(), "qwen3-coder-30b");
    assert_eq!(models.child_model("general_agent"), "qwen3-coder-30b");
    assert_eq!(models.child_model("explore_agent"), "qwen3-coder-30b");
}

/// M-7 on a compatible server: a per-call OpenAI id the server did not
/// list is refused before anything starts, and the refusal lists what the
/// server serves. On the parent `gpt-6.1-sol` was routable there, and the
/// subagent asked the server for it.
#[tokio::test]
async fn a_per_call_openai_id_is_refused_on_a_compatible_server() {
    let models = Models::new(single_lead(serde_json::json!({
        "agent_id": "general_agent",
        "objective": "work for general_agent",
        "model": "gpt-6.1-sol"
    })));
    let router = openai_install_at(&models, COMPAT_BASE, false, "");
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    run(&lead, router, shared, EventBus::default()).await;
    assert!(models.seen().iter().all(|s| s.lead), "no subagent ran");
    let refusal = &models.tool_results()[0];
    assert!(
        refusal.contains(&format!(
            "model 'gpt-6.1-sol' is not routable on this install. Routable now: {SERVED}. \
             Aliases now: fast → {SERVED}, balanced → {SERVED}, strong → {SERVED}. \
             Leave model out to use the template's."
        )),
        "{refusal}"
    );
}

// ── MODEL-ROWS: the stock lead against a local server's 8,192 ──────────────

/// What a stock lead run on a local compatible server measured.
struct StockLead {
    /// The window its loop was sized to.
    window: usize,
    /// Its fixed zone: the system prompt and the tools, as the loop counts
    /// them (bytes / 4).
    system_prompt: usize,
    tools: usize,
    /// How many tools its first request carried.
    tool_count: usize,
    /// The rounds that compacted, and the lead's requests.
    compactions: usize,
    lead_requests: usize,
    /// Requests without the lead's tools: a compaction's summary calls.
    other_requests: usize,
}

/// The stock lead on [`SERVED`] at a loopback compatible server, its row's
/// window `window` (0: none known, as the server's list leaves it), over
/// `rounds` rounds of one cheap tool call each: the builtins and the
/// memory reads in the registry, the built-in skills in the catalogue,
/// the shipped templates, the default `daemon.toml`, no `USER.md`, no
/// extension — the smallest surface a daemon gives its lead.
async fn stock_lead(window: u32, rounds: usize) -> StockLead {
    let mut script: Vec<ChatResponse> = (0..rounds)
        .map(|_| {
            answer(
                &"a round's notes ".repeat(64),
                vec![call("check_subagent_status", serde_json::json!({}))],
            )
        })
        .collect();
    script.push(answer("done", vec![]));
    let models = Models::new(script);
    let router = openai_install_at(&models, COMPAT_BASE, false, "");
    let mut info = router.model_registry().get_model_info(SERVED).unwrap();
    info.context_window = window;
    router.model_registry().register(SERVED.to_string(), info);

    let home = tempfile::tempdir().unwrap();
    let home_root = home.path().canonicalize().unwrap();
    let _guard = crate::test_util::HomeStoreGuard::set(&home_root);
    let registry = ToolRegistry::default();
    for tool in crate::tools::builtins::builtin_tools(None, None, None) {
        registry.register(tool).unwrap();
    }
    let memory = Arc::new(crate::memory::service::MemoryService::open_default(None));
    for tool in crate::tools::builtins::memory_read_tools(memory) {
        registry.register(tool).unwrap();
    }
    let catalog = crate::orchestrator::skill_catalog::SkillCatalog::new();
    let pack = openalpaca_app_support::paths::bundled_skills_dir().unwrap();
    assert!(
        catalog.load_bundled_layer(&pack) > 0,
        "the built-in skills load"
    );
    let shared = shipped();
    let lead = instance(&shared, "lead_agent");
    let bus = EventBus::default();
    let mut events = bus.subscribe();
    let result = run_lead_agent(
        &lead,
        "weigh the herd",
        router,
        Arc::new(registry),
        shared,
        bus,
        None,
        None,
        None,
        "task-1",
        "user-1",
        "user-1:cli",
        "cli",
        &Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        MemoryScopeContext::global_only(),
        None,
        None,
        None,
        "lead::task-1",
        "",
        None,
        Arc::new(catalog),
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        None,
        false,
        None,
    )
    .await;
    assert!(result.success, "{:?}", result.loop_result.finish_reason);

    let mut budget = None;
    let mut compactions = 0;
    while let Ok(event) = events.try_recv() {
        match event {
            crate::events::SystemEvent::CompactionTriggered { .. } => compactions += 1,
            crate::events::SystemEvent::ContextBudgetComputed {
                window_size,
                section_breakdown,
                ..
            } if budget.is_none() => budget = Some((window_size, section_breakdown)),
            _ => {}
        }
    }
    let (window, sections) = budget.expect("the lead publishes its budget");
    let section = |name: &str| {
        sections
            .iter()
            .find(|(n, _)| n == name)
            .map_or(0, |(_, t)| *t)
    };
    let seen = models.seen();
    StockLead {
        window,
        system_prompt: section("system_prompt"),
        tools: section("tools"),
        tool_count: models.lead_request(0).tools.len(),
        compactions,
        lead_requests: seen.iter().filter(|s| s.lead).count(),
        other_requests: seen.iter().filter(|s| !s.lead).count(),
    }
}

/// **MODEL-ROWS's measurement** (*Rev 96* (5), the advisor's pick of
/// 2026-11-16, CONDITIONAL on it): the stock lead's prompt and tools do
/// not fit 8,192 tokens, so a local compatible server's model is not sized
/// at `DEFAULT_LOCAL_CONTEXT_WINDOW` and keeps the 200,000 default, warned
/// once (option (a)). Sized at 8,192, the fixed zone alone — 5,087 tokens
/// of system prompt and 5,320 of 17 tools at this commit — is past the
/// window, let alone its compaction trigger (window less 16.5 %), so the
/// loop compacts on every round and cannot get under it: what a turn adds
/// is folded away each round. At the default, no round compacts. A change
/// that brings the fixed zone under the trigger turns this red: then
/// *Rev 96* (5)'s (b) is worth asking again. The tool count and the
/// rounds are not pinned: a lead tool added or taken away moves the
/// figures, not the finding.
#[tokio::test]
async fn the_stock_lead_does_not_fit_a_local_server_s_8192_window() {
    const ROUNDS: usize = 6;
    let small = stock_lead(8_192, ROUNDS).await;
    assert_eq!(small.window, 8_192);
    let fixed = small.system_prompt + small.tools;
    let trigger = 8_192 - (8_192.0 * 0.165) as usize;
    assert!(
        fixed > 8_192,
        "the fixed zone, {} + {} = {fixed}, is past the whole window",
        small.system_prompt,
        small.tools
    );
    assert!(fixed > trigger);
    assert_eq!(
        small.compactions, small.lead_requests,
        "every round compacts: {} of {}",
        small.compactions, small.lead_requests
    );
    assert!(
        small.other_requests > 0,
        "and its summary tier asks the same server to summarise"
    );

    let as_built = stock_lead(0, ROUNDS).await;
    assert_eq!(as_built.window, DEFAULT_WINDOW, "as built: the default");
    assert_eq!(as_built.system_prompt + as_built.tools, fixed);
    assert_eq!(as_built.compactions, 0, "no round compacts at the default");
    assert_eq!(as_built.other_requests, 0);
    eprintln!(
        "MEASURE window=8192 system_prompt={} tools={} tool_count={} fixed={fixed} \
         trigger={trigger} rounds={} compactions={} summary_requests={}; \
         window={} compactions={} summary_requests={}",
        small.system_prompt,
        small.tools,
        small.tool_count,
        small.lead_requests,
        small.compactions,
        small.other_requests,
        as_built.window,
        as_built.compactions,
        as_built.other_requests
    );
}
