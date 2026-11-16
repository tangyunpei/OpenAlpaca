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

/// A keyed OpenAI install whose provider is built to call `base` (`…/v1`
/// added) and whose runtime config says the same, `rows` its `[models]`.
fn openai_router(base: &str, rows: &[(&str, ModelConfigEntry)]) -> LlmRouter {
    let provider = Arc::new(OpenAiProvider::with_client(
        loopback_client(TIMEOUT),
        "sk-test-not-a-key".to_string(),
        Some("gpt-6-luna".to_string()),
        Some(format!("{base}/v1")),
        None,
    ));
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
    runtime_base_url(&router, &format!("{base}/v1"));
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
    MockHttpServer::start(move |req| match req.path.as_str() {
        "/v1/models" => MockResponse::json(
            serde_json::json!({
                "object": "list",
                "data": [{"id": id, "object": "model", "owned_by": "local"}],
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

/// No warning where a `[models]` row gives the window, nor where the
/// server is not on this machine or the home network (the pick's scope).
#[tokio::test]
#[tracing_test::traced_test]
async fn a_sized_row_or_a_server_elsewhere_is_not_warned_of() {
    let server = listing("gguf-sized").await;
    let router = openai_router(
        &server.base_url,
        &[("gguf-sized", row("openai", None, None))],
    );
    router.refresh_models_for(&ProviderType::OpenAI).await;

    let elsewhere = listing("gguf-elsewhere").await;
    let router = openai_router(&elsewhere.base_url, &[]);
    runtime_base_url(&router, &format!("{REMOTE}/v1"));
    router.refresh_models_for(&ProviderType::OpenAI).await;
    assert!(router.is_routable("gguf-elsewhere"));
    assert!(!logs_contain("no context window"));
}
