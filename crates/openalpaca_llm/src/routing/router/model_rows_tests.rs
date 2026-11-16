//! MODEL-ROWS: what a `[models."<id>"]` row that leaves out a figure is
//! counted at, the refusal's way out on a compatible server, and the
//! window of a model a local server lists without one (the advisor's picks
//! of 2026-11-16 under the owner's delegation, pending-decisions §1's
//! *Rev 96*, *Rev 97* and *Rev 67*).
//!
//! *Rev 97* (b): a price a row leaves out is no price — the compiled row's
//! where there is one, else counted as an unpriced discovered row is: the
//! $3 / $15 fallback on a keyed provider not on this machine or the home
//! network, announced once, 0 on a local one; an explicit 0 stands. *Rev
//! 67*: a row's omitted cache figure keeps the compiled one. *Rev 96* (5):
//! the stock lead does not fit 8,192 tokens (core's
//! `the_stock_lead_does_not_fit_a_local_server_s_8192_window`), so a
//! listed model with no window keeps the loop's 200,000 default and is
//! warned of once. Every server here is a loopback [`MockHttpServer`]; an
//! address elsewhere is only given to a provider that is never called.
//!
//! MODEL-ROWS-2 (the advisor's picks of 2026-11-16 on *Rev 102*, §1's
//! *Rev 105*): (4) a row that leaves out `context` keeps the compiled or
//! the discovered window, *Rev 97*'s field-by-field rule; (5) the
//! once-only warning covers a model any `openai`-provider server that is
//! not OpenAI's own lists with no window; (2), conditional, first
//! established here: a compiled id a local server lists with no row costs
//! the compiled price, not 0, so (a) is kept.

use super::*;
use crate::config::{LlmRuntimeConfig, ModelConfigEntry};
use crate::keys::key_pool::ApiKey;
use crate::providers::openai::OpenAiProvider;
use crate::test_support::{MockHttpServer, MockResponse, loopback_client};
use std::collections::HashSet;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);
/// A million tokens each way: the cost is then the two prices' sum.
const MILLION: u32 = 1_000_000;
/// `CostTracker::pricing_for`'s fallback for a price no row states.
const FALLBACK_IN: f64 = 3.0;
const FALLBACK_OUT: f64 = 15.0;
/// A gateway on the internet: never called.
const REMOTE: &str = "http://203.0.113.5:8080";
/// A server on the home network: never called.
const HOME: &str = "http://192.168.1.20:8080";

fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-9
}

/// A row for `provider` stating `input` and `output` (`None`: left out)
/// and a window.
fn row(provider: &str, input: Option<f64>, output: Option<f64>) -> ModelConfigEntry {
    ModelConfigEntry {
        provider: provider.to_string(),
        input_price: input,
        output_price: output,
        context: Some(32_768),
        supports_image: None,
        supports_audio: None,
        supports_document: None,
        supports_reasoning: None,
        supports_tools: None,
    }
}

/// [`row`] with `context` left out.
fn unsized_row(provider: &str, input: Option<f64>, output: Option<f64>) -> ModelConfigEntry {
    ModelConfigEntry {
        context: None,
        ..row(provider, input, output)
    }
}

/// The OpenAI provider built to call `base` (`…/v1` added).
fn openai_at(base: &str) -> OpenAiProvider {
    OpenAiProvider::with_client(
        loopback_client(TIMEOUT),
        "sk-test-not-a-key".to_string(),
        Some("gpt-6-luna".to_string()),
        Some(format!("{base}/v1")),
        None,
    )
}

/// A keyed OpenAI install whose provider is built to call `base` (`…/v1`
/// added) and whose runtime config says the same, `rows` its `[models]`.
fn openai_router(base: &str, rows: &[(&str, ModelConfigEntry)]) -> LlmRouter {
    openai_router_over(openai_at(base), &format!("{base}/v1"), rows)
}

/// [`openai_router`] over `provider`, the runtime config's `base_url`
/// being `runtime_base`.
fn openai_router_over(
    provider: OpenAiProvider,
    runtime_base: &str,
    rows: &[(&str, ModelConfigEntry)],
) -> LlmRouter {
    let provider = Arc::new(provider);
    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::OpenAI,
        ProviderEntry {
            provider,
            key_pool: Arc::new(ArcSwap::from_pointee(KeyPool::new(
                vec![ApiKey::new(
                    "test".to_string(),
                    ProviderType::OpenAI,
                    "sk-test-not-a-key".to_string(),
                )],
                SelectionStrategy::RoundRobin,
            ))),
        },
    );
    let config: HashMap<String, ModelConfigEntry> = rows
        .iter()
        .map(|(id, entry)| (id.to_string(), entry.clone()))
        .collect();
    let disabled: HashSet<ProviderType> = ProviderType::all()
        .iter()
        .filter(|p| **p != ProviderType::OpenAI)
        .cloned()
        .collect();
    let router = LlmRouter::new(
        providers,
        ModelRegistry::with_defaults_and_config(&config, &disabled),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "gpt-6-luna".to_string(),
    );
    runtime_base_url(&router, runtime_base);
    router
}

/// The runtime config's `[providers.openai] base_url`, as a reload sets it.
fn runtime_base_url(router: &LlmRouter, base: &str) {
    let mut runtime = LlmRuntimeConfig::default();
    runtime
        .provider_defaults
        .get_mut("openai")
        .expect("a compiled openai row")
        .base_url = Some(base.to_string());
    router.reload_runtime_config(runtime);
}

/// The warnings that name `id` with `needle`.
fn warnings(lines: &[&str], needle: &str, id: &str) -> usize {
    lines
        .iter()
        .filter(|l| l.contains(needle) && l.contains(id))
        .count()
}

/// The compiled row of `id`.
fn compiled(id: &str) -> crate::routing::model_registry::ModelInfo {
    ModelRegistry::with_defaults()
        .get_model_info(id)
        .unwrap_or_else(|| panic!("{id} is catalogued"))
}

// ── Rev 97 (b): a price left out is no price ──────────────────────────────

/// A row that sizes a gateway's model and states no price is counted at
/// the fallback, as the same model discovered would be, and announced once
/// however often it is counted. On the parent the row's left-out prices
/// were 0, so that spend passed every cost cap.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_priceless_row_on_a_remote_server_is_counted_at_the_fallback_once_announced() {
    let router = openai_router(REMOTE, &[("gw-model", row("openai", None, None))]);
    for _ in 0..3 {
        let cost = router
            .cost_tracker
            .calculate_cost("gw-model", MILLION, MILLION);
        assert!(
            close(cost, FALLBACK_IN + FALLBACK_OUT),
            "counted at $3 / $15, not {cost}"
        );
    }
    let info = router.model_registry().get_model_info("gw-model").unwrap();
    assert!(info.declared, "still the person's row");
    assert_eq!(info.context_window, 32_768, "its window applied");
    logs_assert(|lines| match warnings(lines, "no price", "gw-model") {
        1 => Ok(()),
        n => Err(format!("announced once, not {n} times")),
    });
}

/// A row that states one price counts that one and the other at the
/// fallback's figure, announced.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_row_stating_one_price_counts_the_other_at_the_fallback() {
    let router = openai_router(
        REMOTE,
        &[
            ("gw-in", row("openai", Some(1.0), None)),
            ("gw-out", row("openai", None, Some(2.0))),
        ],
    );
    let cost = router
        .cost_tracker
        .calculate_cost("gw-in", MILLION, MILLION);
    assert!(close(cost, 1.0 + FALLBACK_OUT), "$1 / $15, not {cost}");
    let cost = router
        .cost_tracker
        .calculate_cost("gw-out", MILLION, MILLION);
    assert!(close(cost, FALLBACK_IN + 2.0), "$3 / $2, not {cost}");
    assert!(logs_contain("no price"));
}

/// On this machine or the home network a price left out is 0, as a model
/// discovered there costs, and is not announced. A guard: green on the
/// parent as well.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_priceless_row_on_a_local_server_costs_nothing() {
    for base in [HOME, "http://127.0.0.1:1"] {
        let router = openai_router(base, &[("lan-model", row("openai", None, None))]);
        let cost = router
            .cost_tracker
            .calculate_cost("lan-model", MILLION, MILLION);
        assert_eq!(cost, 0.0, "{base}");
    }
    assert!(!logs_contain("no price"));
}

/// An explicit 0 is the person's price, on a server anywhere: never
/// repriced and never announced. A guard: green on the parent as well.
#[tokio::test]
#[tracing_test::traced_test]
async fn an_explicit_zero_price_on_a_remote_server_stands() {
    let router = openai_router(REMOTE, &[("gw-free", row("openai", Some(0.0), Some(0.0)))]);
    let cost = router
        .cost_tracker
        .calculate_cost("gw-free", MILLION, MILLION);
    assert_eq!(cost, 0.0);
    assert!(!logs_contain("no price"));
}

/// A row naming a compiled id that leaves its prices out keeps the
/// compiled row's, each half on its own, and is never announced. On the
/// parent the row's left-out prices were 0.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_row_naming_a_compiled_id_keeps_the_compiled_prices_it_leaves_out() {
    let sol = compiled("gpt-6.1-sol");
    let router = openai_router(
        "https://api.openai.com",
        &[("gpt-6.1-sol", row("openai", None, Some(7.0)))],
    );
    // 100,000 tokens each way, below the long-prompt threshold.
    let cost = router
        .cost_tracker
        .calculate_cost("gpt-6.1-sol", MILLION / 10, MILLION / 10);
    let expected = (sol.input_price_per_million + 7.0) / 10.0;
    assert!(close(cost, expected), "{expected}, not {cost}");
    let info = router
        .model_registry()
        .get_model_info("gpt-6.1-sol")
        .unwrap();
    assert_eq!(info.context_window, 32_768, "the row's window");
    assert!(!logs_contain("no price"));
}

/// The compiled price counts on a local server too: the row states none,
/// and the compiled row is the price the rule reaches first. On the
/// parent the row counted 0.
#[tokio::test]
async fn a_row_naming_a_compiled_id_on_a_local_server_counts_the_compiled_price() {
    let sol = compiled("gpt-6.1-sol");
    let router = openai_router(HOME, &[("gpt-6.1-sol", row("openai", None, None))]);
    let cost = router
        .cost_tracker
        .calculate_cost("gpt-6.1-sol", MILLION / 10, MILLION / 10);
    let expected = (sol.input_price_per_million + sol.output_price_per_million) / 10.0;
    assert!(close(cost, expected), "{expected}, not {cost}");
}

// ── Rev 67: a row's omitted cache figure keeps the compiled one ────────────

/// A row naming a compiled id, prices stated or not, reads and writes the
/// cache at the compiled row's multiples of the input rate it pays. On the
/// parent such a row read at the 0.1x a row with no figure gets.
#[tokio::test]
async fn a_row_naming_a_compiled_id_keeps_its_cache_figures() {
    let sol = compiled("gpt-6.1-sol");
    let rates = sol.cache_rates.expect("the compiled row states them");
    assert!(rates.read != 0.10, "a figure the rule can be seen keeping");
    for (input, label) in [(None, "no price"), (Some(1.0), "its own input price")] {
        let router = openai_router(
            "https://api.openai.com",
            &[("gpt-6.1-sol", row("openai", input, Some(10.0)))],
        );
        let rate = input.unwrap_or(sol.input_price_per_million);
        let read = router.cost_tracker.calculate_cost_with_cache(
            "gpt-6.1-sol",
            MILLION / 10,
            0,
            0,
            MILLION / 10,
        );
        let expected = rate * rates.read / 10.0;
        assert!(
            close(read, expected),
            "{label}: read {read}, not {expected}"
        );
        let write = router.cost_tracker.calculate_cost_with_cache(
            "gpt-6.1-sol",
            MILLION / 10,
            0,
            MILLION / 10,
            0,
        );
        let expected = rate * rates.write / 10.0;
        assert!(
            close(write, expected),
            "{label}: write {write}, not {expected}"
        );
    }
}

/// The long-prompt rate belongs to the compiled prices, half by half: a
/// price the row leaves out counts at the compiled row's, its long rate
/// above the threshold included; a price the row states is its own at
/// every length. On the parent no row kept the long rate.
#[tokio::test]
async fn a_price_a_row_leaves_out_keeps_the_compiled_long_prompt_rate() {
    let sol = compiled("gpt-6.1-sol");
    let long = sol.long_prompt.expect("the compiled row has one");
    let over = long.above_input_tokens + 1;

    let router = openai_router(
        "https://api.openai.com",
        &[("gpt-6.1-sol", row("openai", None, None))],
    );
    let cost = router.cost_tracker.calculate_cost("gpt-6.1-sol", over, 0);
    let expected = over as f64 * long.input_price_per_million / 1e6;
    assert!(
        close(cost, expected),
        "the long rate: {cost}, not {expected}"
    );

    let router = openai_router(
        "https://api.openai.com",
        &[("gpt-6.1-sol", row("openai", Some(1.0), Some(5.0)))],
    );
    let cost = router.cost_tracker.calculate_cost("gpt-6.1-sol", over, 0);
    let expected = over as f64 * 1.0 / 1e6;
    assert!(
        close(cost, expected),
        "the row's own: {cost}, not {expected}"
    );

    // One price stated: that half is the row's own at every length, the
    // half it leaves out the compiled long rate above the threshold.
    let router = openai_router(
        "https://api.openai.com",
        &[("gpt-6.1-sol", row("openai", Some(1.0), None))],
    );
    let cost = router
        .cost_tracker
        .calculate_cost("gpt-6.1-sol", over, MILLION);
    let expected = over as f64 * 1.0 / 1e6 + long.output_price_per_million;
    assert!(
        close(cost, expected),
        "input stated, output the long rate: {cost}, not {expected}"
    );

    let router = openai_router(
        "https://api.openai.com",
        &[("gpt-6.1-sol", row("openai", None, Some(5.0)))],
    );
    let cost = router
        .cost_tracker
        .calculate_cost("gpt-6.1-sol", over, MILLION);
    let expected = over as f64 * long.input_price_per_million / 1e6 + 5.0;
    assert!(
        close(cost, expected),
        "output stated, input the long rate: {cost}, not {expected}"
    );

    // Below the threshold the left-out half is the compiled standard rate.
    let under = long.above_input_tokens;
    let cost = router
        .cost_tracker
        .calculate_cost("gpt-6.1-sol", under, MILLION);
    let expected = under as f64 * sol.input_price_per_million / 1e6 + 5.0;
    assert!(
        close(cost, expected),
        "below the threshold: {cost}, not {expected}"
    );
}

// ── Rev 96 (1): the refusal names the way out ──────────────────────────────

/// With nothing routable the refusal names, beside turning a provider on,
/// the row that runs a compatible server whose model list cannot be read.
#[test]
fn the_no_routable_model_text_names_the_models_row_for_a_compatible_server() {
    let text = LlmRouterError::NoRoutableModel.to_string();
    assert!(text.starts_with("No model is available: no enabled provider offers one."));
    assert!(
        text.contains("[models.\"<id>\"]") && text.contains("provider = \"openai\""),
        "{text}"
    );
}

// ── Rev 96 (5): a listed model with no window, warned of once ─────────────

/// A llama.cpp-like server listing `id` with no window.
async fn listing(id: &'static str) -> MockHttpServer {
    listing_of(vec![id]).await
}

/// A llama.cpp-like server listing `ids`, none with a window.
async fn listing_of(ids: Vec<&'static str>) -> MockHttpServer {
    MockHttpServer::start(move |req| match req.path.as_str() {
        "/v1/models" => MockResponse::json(
            serde_json::json!({
                "object": "list",
                "data": ids
                    .iter()
                    .map(|id| serde_json::json!({"id": id, "object": "model", "owned_by": "local"}))
                    .collect::<Vec<_>>(),
            })
            .to_string(),
        ),
        _ => MockResponse::not_found(),
    })
    .await
}

/// A model a local server lists with no window keeps none — so the loop
/// sizes a run on it at its 200,000 default — and the daemon says so once,
/// naming the row that sizes it, however often discovery runs. On the
/// parent nothing was said.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_model_a_local_server_lists_with_no_window_is_warned_of_once() {
    let server = listing("gguf-no-window").await;
    let router = openai_router(&server.base_url, &[]);
    for _ in 0..3 {
        router.refresh_models_for(&ProviderType::OpenAI).await;
    }
    assert!(router.is_routable("gguf-no-window"));
    let info = router
        .model_registry()
        .get_model_info("gguf-no-window")
        .unwrap();
    assert_eq!(info.context_window, 0, "no window: not 8,192");
    logs_assert(
        |lines| match warnings(lines, "no context window", "gguf-no-window") {
            1 => Ok(()),
            n => Err(format!("warned once, not {n} times")),
        },
    );
    assert!(logs_contain("context = "));
}

/// No warning where a `[models]` row gives the window. A guard: green on
/// the parent as well.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_sized_row_is_not_warned_of() {
    let server = listing("gguf-sized").await;
    let router = openai_router(
        &server.base_url,
        &[("gguf-sized", row("openai", None, None))],
    );
    router.refresh_models_for(&ProviderType::OpenAI).await;
    assert!(router.is_routable("gguf-sized"));
    assert!(!logs_contain("no context window"));
}

// ── MODEL-ROWS-2, Rev 102 (5): every server that is not OpenAI's own ──────

/// A gateway that is neither on this machine nor on the home network,
/// listing a model with no window, is warned of once, the row that sizes
/// it named (*Rev 102* (5) (b)). On the parent such a server was not
/// warned of: the warning's scope was the local servers.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_model_a_gateway_elsewhere_lists_with_no_window_is_warned_of_once() {
    let elsewhere = listing("gw-no-window").await;
    let router = openai_router_over(openai_at(&elsewhere.base_url), &format!("{REMOTE}/v1"), &[]);
    assert!(
        !router
            .model_registry()
            .is_local_server_provider(&ProviderType::OpenAI),
        "counted as a server elsewhere"
    );
    for _ in 0..3 {
        router.refresh_models_for(&ProviderType::OpenAI).await;
    }
    assert!(router.is_routable("gw-no-window"));
    logs_assert(
        |lines| match warnings(lines, "no context window", "gw-no-window") {
            1 => Ok(()),
            n => Err(format!("warned once, not {n} times")),
        },
    );
    assert!(logs_contain("context = "));
}

/// OpenAI's own API is not warned of: a model it lists that no compiled
/// row names (a dated snapshot, an older id) is not a server's model of
/// unknown size the person has to describe. A guard: green on the parent
/// as well.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_model_openai_s_own_api_lists_with_no_window_is_not_warned_of() {
    let server = listing("gpt-snapshot-2026-01-01").await;
    let router = openai_router_over(
        openai_at(&server.base_url).with_openai_api(true),
        "https://api.openai.com/v1",
        &[],
    );
    router.refresh_models_for(&ProviderType::OpenAI).await;
    assert!(
        router
            .model_registry()
            .get_model_info("gpt-snapshot-2026-01-01")
            .is_some_and(|info| info.context_window == 0),
        "listed, with no window"
    );
    assert!(!logs_contain("no context window"));
}

// ── MODEL-ROWS-2, Rev 102 (4): a row that leaves out `context` ────────────

/// An Ollama at `base` (`/v1` added) with an empty catalogue, its
/// `[models]` rows read from `rows` as at boot: before any discovery.
fn ollama_router(base: &str, rows: &[(&str, ModelConfigEntry)]) -> LlmRouter {
    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Ollama,
        ProviderEntry {
            provider: ollama_at(base),
            key_pool: Arc::new(ArcSwap::from_pointee(KeyPool::new(
                vec![],
                SelectionStrategy::RoundRobin,
            ))),
        },
    );
    let registry = ModelRegistry::new(HashMap::new());
    registry.reload_from_config(&config_of(rows), &HashSet::new());
    LlmRouter::new(
        providers,
        registry,
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        "unset".to_string(),
    )
}

/// An Ollama provider at `base` (`/v1` added).
fn ollama_at(base: &str) -> Arc<dyn LlmProvider> {
    Arc::new(crate::providers::ollama::OllamaProvider::with_client(
        loopback_client(TIMEOUT),
        "unset".to_string(),
        Some(format!("{base}/v1")),
        None,
    ))
}

fn config_of(rows: &[(&str, ModelConfigEntry)]) -> HashMap<String, ModelConfigEntry> {
    rows.iter()
        .map(|(id, entry)| (id.to_string(), entry.clone()))
        .collect()
}

/// An Ollama with `tag` installed, `/api/show` giving its window.
async fn ollama_with(tag: &'static str, window: u64) -> MockHttpServer {
    MockHttpServer::start(move |req| match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/tags") => {
            MockResponse::json(serde_json::json!({"models": [{"name": tag}]}).to_string())
        }
        ("POST", "/api/show") => MockResponse::json(
            serde_json::json!({
                "capabilities": ["completion", "tools"],
                "model_info": {"llama.context_length": window},
            })
            .to_string(),
        ),
        _ => MockResponse::not_found(),
    })
    .await
}

fn window_of(router: &LlmRouter, id: &str) -> u32 {
    router
        .model_registry()
        .get_model_info(id)
        .unwrap_or_else(|| panic!("{id} is registered"))
        .context_window
}

/// A row written only to price an Ollama tag keeps the window `/api/show`
/// gave, whether the row was read before discovery (a boot) or after it
/// (a hand edit's reload); a row's own `context` still wins, and one
/// taken out again gives the discovered window back, not the figure the
/// deleted line gave. On the parent the row set 200,000 over it.
#[tokio::test]
async fn a_row_that_leaves_out_context_keeps_the_discovered_window() {
    let server = ollama_with("priced-tag", 32_768).await;
    let priced = || [("priced-tag", unsized_row("ollama", Some(0.5), Some(1.0)))];

    // The boot's order: the row first, then discovery.
    let router = ollama_router(&server.base_url, &priced());
    router.refresh_models_for(&ProviderType::Ollama).await;
    assert_eq!(
        window_of(&router, "priced-tag"),
        32_768,
        "row, then discovery"
    );

    // A reload's order: discovery first, then the row.
    let router = ollama_router(&server.base_url, &[]);
    router.refresh_models_for(&ProviderType::Ollama).await;
    assert_eq!(window_of(&router, "priced-tag"), 32_768);
    let registry = router.model_registry();
    registry.reload_from_config(&config_of(&priced()), &HashSet::new());
    assert_eq!(
        window_of(&router, "priced-tag"),
        32_768,
        "discovery, then row"
    );

    // The row's own `context` wins, and taking it out again gives back
    // the discovered window.
    let sized = [(
        "priced-tag",
        ModelConfigEntry {
            context: Some(16_384),
            ..unsized_row("ollama", Some(0.5), Some(1.0))
        },
    )];
    registry.reload_from_config(&config_of(&sized), &HashSet::new());
    assert_eq!(window_of(&router, "priced-tag"), 16_384, "the row's own");
    registry.reload_from_config(&config_of(&priced()), &HashSet::new());
    assert_eq!(
        window_of(&router, "priced-tag"),
        32_768,
        "the line taken out"
    );
}

/// A row naming a compiled id that leaves out `context` keeps the
/// compiled window; one stating it is its own. On the parent the row set
/// 200,000 over the compiled 1,050,000.
#[tokio::test]
async fn a_row_naming_a_compiled_id_that_leaves_out_context_keeps_the_compiled_window() {
    let sol = compiled("gpt-6.1-sol");
    assert!(
        sol.context_window > 200_000,
        "a window the rule can be seen keeping"
    );
    let router = openai_router(
        "https://api.openai.com",
        &[("gpt-6.1-sol", unsized_row("openai", Some(1.0), Some(5.0)))],
    );
    assert_eq!(window_of(&router, "gpt-6.1-sol"), sol.context_window);
    let router = openai_router(
        "https://api.openai.com",
        &[("gpt-6.1-sol", row("openai", Some(1.0), Some(5.0)))],
    );
    assert_eq!(window_of(&router, "gpt-6.1-sol"), 32_768, "the row's own");
}

/// A row that leaves out `context` for a model no compiled row names and
/// a compatible server lists with no window states no window: the model
/// keeps none, so a run on it is budgeted at the loop's 200,000 default,
/// and it is warned of once as an unsized listed model is. On the parent
/// the row set 200,000 and nothing was said.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_priced_row_with_no_window_anywhere_is_warned_of_once() {
    let server = listing("gguf-priced").await;
    let router = openai_router(
        &server.base_url,
        &[("gguf-priced", unsized_row("openai", Some(0.0), Some(0.0)))],
    );
    for _ in 0..2 {
        router.refresh_models_for(&ProviderType::OpenAI).await;
    }
    assert_eq!(window_of(&router, "gguf-priced"), 0, "no window stated");
    logs_assert(
        |lines| match warnings(lines, "no context window", "gguf-priced") {
            1 => Ok(()),
            n => Err(format!("warned once, not {n} times")),
        },
    );
}

/// A provider turned off and on again asks discovery afresh: the window
/// the turned-off provider's discovery gave goes with it, so the row read
/// back on the enable (`set_provider_enabled`'s order: the defaults, the
/// rows, the provider, then discovery) keeps the window the provider now
/// reports, not the one it had (`remove_by_provider`'s clearing).
#[tokio::test]
async fn a_provider_turned_off_and_on_forgets_the_window_it_had() {
    let before = ollama_with("priced-tag", 32_768).await;
    let after = ollama_with("priced-tag", 65_536).await;
    let priced = [("priced-tag", unsized_row("ollama", Some(0.5), Some(1.0)))];
    let router = ollama_router(&before.base_url, &priced);
    router.refresh_models_for(&ProviderType::Ollama).await;
    assert_eq!(window_of(&router, "priced-tag"), 32_768, "before");

    router.deregister_provider(&ProviderType::Ollama);
    let registry = router.model_registry();
    registry.restore_defaults_for_provider(&ProviderType::Ollama);
    registry.reload_from_config(&config_of(&priced), &HashSet::new());
    router.register_provider(
        ProviderType::Ollama,
        ollama_at(&after.base_url),
        KeyPool::new(vec![], SelectionStrategy::RoundRobin),
    );
    router.refresh_models_for(&ProviderType::Ollama).await;
    assert_eq!(
        window_of(&router, "priced-tag"),
        65_536,
        "the window the provider reports now"
    );
}

/// An id taken out of the registry takes the window its discovery gave
/// with it: a row for it read later that leaves out `context` states none
/// (`remove`'s clearing).
#[test]
fn a_removed_id_forgets_the_window_discovery_gave_it() {
    let registry = ModelRegistry::new(HashMap::new());
    registry.register_discovered(
        "priced-tag".to_string(),
        crate::routing::model_registry::ModelInfo {
            provider: ProviderType::Ollama,
            context_window: 32_768,
            ..compiled("gpt-6.1-sol")
        },
    );
    assert!(registry.remove("priced-tag"));
    let priced = [("priced-tag", unsized_row("ollama", Some(0.5), Some(1.0)))];
    registry.reload_from_config(&config_of(&priced), &HashSet::new());
    let window = registry
        .get_model_info("priced-tag")
        .expect("the row registers it")
        .context_window;
    assert_eq!(window, 0, "no window stated, none remembered");
}

/// A row naming a compiled id that leaves out `context` keeps the
/// compiled window even when a discovery gave the id another: the
/// compiled row's window comes before the discovered one, as
/// `register_discovered` never replaces a compiled window.
#[test]
fn a_compiled_window_comes_before_a_discovered_one() {
    let sol = compiled("gpt-6.1-sol");
    let registry = ModelRegistry::new(HashMap::new());
    registry.register_discovered(
        "gpt-6.1-sol".to_string(),
        crate::routing::model_registry::ModelInfo {
            context_window: 8_192,
            ..sol.clone()
        },
    );
    let priced = [("gpt-6.1-sol", unsized_row("openai", Some(1.0), Some(5.0)))];
    registry.reload_from_config(&config_of(&priced), &HashSet::new());
    let window = registry
        .get_model_info("gpt-6.1-sol")
        .expect("the row registers it")
        .context_window;
    assert_eq!(window, sol.context_window, "the compiled window");
}

// ── MODEL-ROWS-2, Rev 102 (2): established first ──────────────────────────

/// *Rev 102* (2)'s condition, established before anything was built: a
/// compiled id a server on this machine or the home network lists, with
/// no `[models]` row, costs the compiled row's price — not 0, which only a
/// discovered id with no compiled row gets there — and is not announced;
/// an uncatalogued id the same server lists costs 0. So the condition
/// fails and (a) is kept: a row that states no price for that id counts
/// what the bare listing counts (`a_row_naming_a_compiled_id_on_a_local_
/// server_counts_the_compiled_price`). Green on the parent: a pin.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_compiled_id_a_local_server_lists_with_no_row_costs_the_compiled_price() {
    let sol = compiled("gpt-6.1-sol");
    let server = listing_of(vec!["gpt-6.1-sol", "gguf-local"]).await;
    let router = openai_router(&server.base_url, &[]);
    assert!(
        router
            .model_registry()
            .is_local_server_provider(&ProviderType::OpenAI),
        "a server on this machine"
    );
    router.refresh_models_for(&ProviderType::OpenAI).await;
    assert!(router.is_routable("gpt-6.1-sol"), "listed, so served");
    let cost = router
        .cost_tracker
        .calculate_cost("gpt-6.1-sol", MILLION / 10, MILLION / 10);
    let expected = (sol.input_price_per_million + sol.output_price_per_million) / 10.0;
    assert!(expected > 0.0);
    assert!(
        close(cost, expected),
        "the compiled price {expected}, not {cost}"
    );
    let cost = router
        .cost_tracker
        .calculate_cost("gguf-local", MILLION, MILLION);
    assert_eq!(cost, 0.0, "an uncatalogued id there is 0");
    assert!(!logs_contain("no price"));
}

// ── CAP-FLAGS, Rev 110 (3) (b): a row that leaves out a capability flag ──

/// An Ollama with `tag` installed, `/api/show` giving a 32,768 window and
/// `capabilities`.
async fn ollama_capable(
    tag: &'static str,
    capabilities: &'static [&'static str],
) -> MockHttpServer {
    MockHttpServer::start(move |req| match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/tags") => {
            MockResponse::json(serde_json::json!({"models": [{"name": tag}]}).to_string())
        }
        ("POST", "/api/show") => MockResponse::json(
            serde_json::json!({
                "capabilities": capabilities,
                "model_info": {"llama.context_length": 32_768},
            })
            .to_string(),
        ),
        _ => MockResponse::not_found(),
    })
    .await
}

/// `id`'s registered row.
fn info_of(registry: &ModelRegistry, id: &str) -> crate::routing::model_registry::ModelInfo {
    registry
        .get_model_info(id)
        .unwrap_or_else(|| panic!("{id} is registered"))
}

/// [`unsized_row`] for Ollama, a price and nothing else.
fn priced_only() -> ModelConfigEntry {
    unsized_row("ollama", Some(0.5), Some(1.0))
}

/// A row written only to price a vision Ollama tag keeps the image
/// support `/api/show` reported, whether the row was read before
/// discovery (a boot) or after it (a hand edit's reload), and a
/// `supports_image` line taken out again gives the discovered value back.
/// On the parent the row's left-out flag said `false` over it in both
/// orders (the review's probe).
#[tokio::test]
async fn a_row_that_leaves_out_a_flag_keeps_what_discovery_read() {
    let server = ollama_capable("vision-tag", &["completion", "tools", "vision"]).await;
    let priced = || [("vision-tag", priced_only())];

    // The boot's order: the row first, then discovery.
    let router = ollama_router(&server.base_url, &priced());
    router.refresh_models_for(&ProviderType::Ollama).await;
    let info = info_of(router.model_registry(), "vision-tag");
    assert!(info.supports_image, "row, then discovery");
    assert!(info.supports_tools, "row, then discovery");

    // A reload's order: discovery first, then the row.
    let router = ollama_router(&server.base_url, &[]);
    router.refresh_models_for(&ProviderType::Ollama).await;
    let registry = router.model_registry();
    assert!(info_of(registry, "vision-tag").supports_image, "discovered");
    registry.reload_from_config(&config_of(&priced()), &HashSet::new());
    assert!(
        info_of(registry, "vision-tag").supports_image,
        "discovery, then row"
    );

    // A stated `false` wins, and the line taken out again gives back what
    // discovery read.
    let stated = [(
        "vision-tag",
        ModelConfigEntry {
            supports_image: Some(false),
            ..priced_only()
        },
    )];
    registry.reload_from_config(&config_of(&stated), &HashSet::new());
    assert!(
        !info_of(registry, "vision-tag").supports_image,
        "the row's own"
    );
    registry.reload_from_config(&config_of(&priced()), &HashSet::new());
    assert!(
        info_of(registry, "vision-tag").supports_image,
        "the line taken out"
    );
}

/// A flag the row states wins over what discovery read, in both orders:
/// `false` on a vision tag, `true` on a tag that reports no vision. Green
/// on the parent: a guard against a fix that lets discovery overwrite a
/// stated flag.
#[tokio::test]
async fn a_flag_the_row_states_wins_over_discovery() {
    let vision = ollama_capable("vision-tag", &["completion", "tools", "vision"]).await;
    let plain = ollama_capable("plain-tag", &["completion", "tools"]).await;
    let stating = |value| ModelConfigEntry {
        supports_image: Some(value),
        ..priced_only()
    };
    for (server, tag, value) in [(&vision, "vision-tag", false), (&plain, "plain-tag", true)] {
        let rows = [(tag, stating(value))];
        // The row first, then discovery.
        let router = ollama_router(&server.base_url, &rows);
        router.refresh_models_for(&ProviderType::Ollama).await;
        assert_eq!(
            info_of(router.model_registry(), tag).supports_image,
            value,
            "{tag}: row, then discovery"
        );
        // Discovery first, then the row.
        let router = ollama_router(&server.base_url, &[]);
        router.refresh_models_for(&ProviderType::Ollama).await;
        let registry = router.model_registry();
        registry.reload_from_config(&config_of(&rows), &HashSet::new());
        assert_eq!(
            info_of(registry, tag).supports_image,
            value,
            "{tag}: discovery, then row"
        );
        // A later discovery leaves the stated flag alone.
        router.refresh_models_for(&ProviderType::Ollama).await;
        assert_eq!(
            info_of(registry, tag).supports_image,
            value,
            "{tag}: discovery again"
        );
    }

    // `supports_tools` stated `false` on a tag `/api/show` says takes
    // tools: the row's own, in both orders.
    let rows = [(
        "plain-tag",
        ModelConfigEntry {
            supports_tools: Some(false),
            ..priced_only()
        },
    )];
    let router = ollama_router(&plain.base_url, &rows);
    router.refresh_models_for(&ProviderType::Ollama).await;
    assert!(
        !info_of(router.model_registry(), "plain-tag").supports_tools,
        "tools: row, then discovery"
    );
    let router = ollama_router(&plain.base_url, &[]);
    router.refresh_models_for(&ProviderType::Ollama).await;
    let registry = router.model_registry();
    registry.reload_from_config(&config_of(&rows), &HashSet::new());
    router.refresh_models_for(&ProviderType::Ollama).await;
    assert!(
        !info_of(registry, "plain-tag").supports_tools,
        "tools: discovery, then row, then discovery again"
    );
}

/// `supports_tools` follows the same rule: a row that leaves it out on a
/// tag whose `/api/show` names no `tools` capability keeps discovery's
/// `false`, so the ladder stops preferring it as it does with no row. On
/// the parent the row's left-out flag said `true` over it.
#[tokio::test]
async fn a_row_that_leaves_out_supports_tools_keeps_discovery_s_no() {
    let server = ollama_capable("chat-only", &["completion"]).await;
    let priced = [("chat-only", priced_only())];
    let router = ollama_router(&server.base_url, &priced);
    router.refresh_models_for(&ProviderType::Ollama).await;
    assert!(
        !info_of(router.model_registry(), "chat-only").supports_tools,
        "row, then discovery"
    );
    let router = ollama_router(&server.base_url, &[]);
    router.refresh_models_for(&ProviderType::Ollama).await;
    let registry = router.model_registry();
    registry.reload_from_config(&config_of(&priced), &HashSet::new());
    assert!(
        !info_of(registry, "chat-only").supports_tools,
        "discovery, then row"
    );
}

/// A provider turned off and on again forgets the flags its discovery
/// read: the row read back on the enable, before the new discovery, holds
/// the defaults, not the turned-off provider's reading
/// (`remove_by_provider`'s clearing), and then takes what the provider
/// reports now.
#[tokio::test]
async fn a_provider_turned_off_and_on_forgets_the_flags_it_read() {
    let before = ollama_capable("priced-tag", &["completion", "tools", "vision"]).await;
    let after = ollama_capable("priced-tag", &["completion"]).await;
    let priced = [("priced-tag", priced_only())];
    let router = ollama_router(&before.base_url, &priced);
    router.refresh_models_for(&ProviderType::Ollama).await;
    assert!(
        info_of(router.model_registry(), "priced-tag").supports_image,
        "before"
    );

    router.deregister_provider(&ProviderType::Ollama);
    let registry = router.model_registry();
    registry.restore_defaults_for_provider(&ProviderType::Ollama);
    registry.reload_from_config(&config_of(&priced), &HashSet::new());
    let info = info_of(registry, "priced-tag");
    assert!(!info.supports_image, "nothing read since the enable");
    assert!(info.supports_tools, "the row's default");
    router.register_provider(
        ProviderType::Ollama,
        ollama_at(&after.base_url),
        KeyPool::new(vec![], SelectionStrategy::RoundRobin),
    );
    router.refresh_models_for(&ProviderType::Ollama).await;
    let info = info_of(registry, "priced-tag");
    assert!(!info.supports_image, "what the provider reports now");
    assert!(!info.supports_tools, "what the provider reports now");
}

/// An id taken out of the registry takes the flags its discovery read
/// with it: a row for it read later that leaves them out holds the
/// defaults (`remove`'s clearing).
#[test]
fn a_removed_id_forgets_the_flags_discovery_read() {
    let registry = ModelRegistry::new(HashMap::new());
    let discovered = || crate::routing::model_registry::ModelInfo {
        provider: ProviderType::Ollama,
        input_price_per_million: 0.0,
        output_price_per_million: 0.0,
        context_window: 32_768,
        discovered: true,
        supports_image: true,
        supports_audio: false,
        supports_document: false,
        supports_reasoning: false,
        supports_tools: false,
        declared: false,
        long_prompt: None,
        cache_rates: None,
        request_contract: crate::routing::model_registry::RequestContract::NONE,
    };
    let priced = [("priced-tag", priced_only())];
    registry.register_discovered("priced-tag".to_string(), discovered());
    registry.reload_from_config(&config_of(&priced), &HashSet::new());
    let info = info_of(&registry, "priced-tag");
    assert!(info.supports_image && !info.supports_tools, "what was read");

    assert!(registry.remove("priced-tag"));
    registry.reload_from_config(&config_of(&priced), &HashSet::new());
    let info = info_of(&registry, "priced-tag");
    assert!(!info.supports_image, "nothing remembered: the default");
    assert!(info.supports_tools, "nothing remembered: the default");
}

/// A row naming a compiled id that leaves out its flags keeps the
/// compiled row's, even when a discovery read the id otherwise (a bare
/// listing reports no image support for every id), in both orders: the
/// compiled row comes before the discovered reading, as it does for the
/// window. On the parent the row's left-out flags said `false`.
#[test]
fn a_compiled_flag_comes_before_a_discovered_one() {
    let sol = compiled("gpt-6.1-sol");
    assert!(
        sol.supports_image && sol.supports_reasoning,
        "flags the rule can be seen keeping"
    );
    let bare = || crate::routing::model_registry::ModelInfo {
        supports_image: false,
        supports_reasoning: false,
        ..sol.clone()
    };
    let priced = [("gpt-6.1-sol", unsized_row("openai", Some(1.0), Some(5.0)))];

    // Discovery first, then the row.
    let registry = ModelRegistry::new(HashMap::new());
    registry.register_discovered("gpt-6.1-sol".to_string(), bare());
    registry.reload_from_config(&config_of(&priced), &HashSet::new());
    let info = info_of(&registry, "gpt-6.1-sol");
    assert!(info.supports_image, "discovery, then row");
    assert!(info.supports_reasoning, "discovery, then row");

    // The row first, then discovery.
    let registry = ModelRegistry::new(HashMap::new());
    registry.reload_from_config(&config_of(&priced), &HashSet::new());
    registry.register_discovered("gpt-6.1-sol".to_string(), bare());
    let info = info_of(&registry, "gpt-6.1-sol");
    assert!(info.supports_image, "row, then discovery");
    assert!(info.supports_reasoning, "row, then discovery");
}

/// The PDF flag rides the same rung: a row pricing a compiled id whose
/// compiled row reads PDFs (`claude-opus-5-5`) and leaving the flag out
/// keeps it, whatever a discovery read, in both orders. `gpt-6.1-sol`, the
/// test above, cannot show it: its compiled `supports_document` is `false`.
/// (No compiled row says `supports_audio`, so the audio flag's compiled
/// rung has nothing to keep today.)
#[test]
fn a_compiled_pdf_flag_comes_before_a_discovered_one() {
    let opus = compiled("claude-opus-5-5");
    assert!(
        opus.supports_document,
        "the flag the rule can be seen keeping"
    );
    let bare = || crate::routing::model_registry::ModelInfo {
        supports_document: false,
        ..opus.clone()
    };
    let priced = [(
        "claude-opus-5-5",
        unsized_row("anthropic", Some(4.0), Some(20.0)),
    )];

    // Discovery first, then the row.
    let registry = ModelRegistry::new(HashMap::new());
    registry.register_discovered("claude-opus-5-5".to_string(), bare());
    registry.reload_from_config(&config_of(&priced), &HashSet::new());
    assert!(
        info_of(&registry, "claude-opus-5-5").supports_document,
        "discovery, then row"
    );

    // The row first, then discovery.
    let registry = ModelRegistry::new(HashMap::new());
    registry.reload_from_config(&config_of(&priced), &HashSet::new());
    registry.register_discovered("claude-opus-5-5".to_string(), bare());
    assert!(
        info_of(&registry, "claude-opus-5-5").supports_document,
        "row, then discovery"
    );
}

/// A later discovery's reading replaces a flag the row left out (the
/// latest wins, *Rev 115*'s second item, (a)): a tag `/api/show` first
/// describes with vision and tools, then, with no provider turned off in
/// between, with neither (pulled again), is read the second way both on
/// the live row and on a reload after it, in both orders.
#[tokio::test]
async fn a_later_discovery_s_reading_replaces_a_left_out_flag() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let pulled_again = Arc::new(AtomicBool::new(false));
    let switch = Arc::clone(&pulled_again);
    let server = MockHttpServer::start(move |req| match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/tags") => {
            MockResponse::json(serde_json::json!({"models": [{"name": "pulled-tag"}]}).to_string())
        }
        ("POST", "/api/show") => {
            let capabilities: &[&str] = if switch.load(Ordering::SeqCst) {
                &["completion"]
            } else {
                &["completion", "tools", "vision"]
            };
            MockResponse::json(
                serde_json::json!({
                    "capabilities": capabilities,
                    "model_info": {"llama.context_length": 32_768},
                })
                .to_string(),
            )
        }
        _ => MockResponse::not_found(),
    })
    .await;
    let priced = [("pulled-tag", priced_only())];

    for row_first in [true, false] {
        pulled_again.store(false, Ordering::SeqCst);
        let router = ollama_router(&server.base_url, if row_first { &priced } else { &[] });
        router.refresh_models_for(&ProviderType::Ollama).await;
        let registry = router.model_registry();
        if !row_first {
            registry.reload_from_config(&config_of(&priced), &HashSet::new());
        }
        let info = info_of(registry, "pulled-tag");
        assert!(
            info.supports_image && info.supports_tools,
            "row first {row_first}: the first reading"
        );

        pulled_again.store(true, Ordering::SeqCst);
        router.refresh_models_for(&ProviderType::Ollama).await;
        let info = info_of(registry, "pulled-tag");
        assert!(
            !info.supports_image,
            "row first {row_first}: the latest, live"
        );
        assert!(
            !info.supports_tools,
            "row first {row_first}: the latest, live"
        );
        registry.reload_from_config(&config_of(&priced), &HashSet::new());
        let info = info_of(registry, "pulled-tag");
        assert!(
            !info.supports_image,
            "row first {row_first}: the latest, reloaded"
        );
        assert!(
            !info.supports_tools,
            "row first {row_first}: the latest, reloaded"
        );
    }
}
