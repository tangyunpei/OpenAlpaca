//! The compiled catalogue against the providers' own pages (C7: the
//! subagent design's M-3c, and O-17 (a), read on 2026-11-12; MODELS-LATEST:
//! the design's §4.9, the owner's models directive of 2026-11-14 and his
//! agreement, the pages re-read on 2026-11-14).
//!
//! Anthropic: `platform.claude.com/docs/en/about-claude/pricing`, the models
//! overview (`/docs/en/about-claude/models/overview`), each model's own
//! overview and migration guide (`/docs/en/models/<model>/...`), the
//! model-deprecations page and `.../build-with-claude/thinking` and
//! `thinking-troubleshooting`. OpenAI:
//! `developers.openai.com/api/docs/pricing`, `.../models/all`, each model's
//! own page, `.../guides/latest-model` and `.../deprecations`.

use super::*;

const EPS: f64 = 1e-9;

fn row(registry: &ModelRegistry, id: &str) -> ModelInfo {
    registry
        .get_model_info(id)
        .unwrap_or_else(|| panic!("{id} has a compiled row"))
}

fn assert_price(info: &ModelInfo, id: &str, input: f64, output: f64) {
    assert!(
        (info.input_price_per_million - input).abs() < EPS
            && (info.output_price_per_million - output).abs() < EPS,
        "{id} is priced ${input} / ${output} per million, not ${} / ${}",
        info.input_price_per_million,
        info.output_price_per_million
    );
}

/// M-3c: the two flagships the owner's O-13 answer names, at the rate and
/// window their pages print. Red on the parent, which has neither row.
#[test]
fn the_catalogue_carries_the_latest_flagships_at_their_page_prices() {
    let registry = ModelRegistry::with_defaults();

    let opus = row(&registry, "claude-opus-5-5");
    assert_eq!(opus.provider, ProviderType::Anthropic);
    assert_price(&opus, "claude-opus-5-5", 4.0, 20.0);
    assert_eq!(opus.context_window, 1_000_000);
    assert!(opus.supports_tools && opus.supports_image && opus.supports_document);
    assert!(opus.declared && !opus.discovered);

    let sol = row(&registry, "gpt-6.1-sol");
    assert_eq!(sol.provider, ProviderType::OpenAI);
    assert_price(&sol, "gpt-6.1-sol", 2.0, 10.0);
    assert_eq!(sol.context_window, 1_050_000);
    assert!(sol.supports_tools && sol.supports_image && sol.supports_reasoning);
    assert!(!sol.supports_audio && !sol.supports_document);
    assert!(sol.declared && !sol.discovered);
}

/// O-17 (a): the rows for models the providers' own pages call shut down
/// (OpenAI: `o3-mini`, `o1`, `o1-mini`) or retired (Anthropic:
/// `claude-opus-4-20250514`, `claude-sonnet-4-20250514`) are gone, so a pin
/// naming one walks the ladder before the call instead of failing at the
/// provider. Red on the parent, which lists all five.
#[test]
fn the_shut_down_and_retired_rows_are_gone() {
    let registry = ModelRegistry::with_defaults();
    for id in REMOVED_ROWS {
        assert!(
            registry.get_model_info(id).is_none(),
            "{id} is shut down or retired and has no compiled row"
        );
    }
    // Nor does re-enabling a provider put one back.
    let registry = ModelRegistry::new(HashMap::new());
    registry.restore_defaults_for_provider(&ProviderType::OpenAI);
    registry.restore_defaults_for_provider(&ProviderType::Anthropic);
    for id in REMOVED_ROWS {
        assert!(
            registry.get_model_info(id).is_none(),
            "{id} is not restored"
        );
    }
}

/// The ids O-17 (a) removes from the catalogue.
const REMOVED_ROWS: [&str; 5] = [
    "o3-mini",
    "o1",
    "o1-mini",
    "claude-opus-4-20250514",
    "claude-sonnet-4-20250514",
];

/// Nothing the release ships names a removed row: the seeded `llm.toml`, the
/// nine bundled templates' `model:` and `fallback_models` lines, and the
/// runtime defaults (characterization; the parent's shipped files already
/// name none, so the rows could go).
#[test]
fn nothing_shipped_names_a_removed_row() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut shipped = vec!["scripts/release/templates/config/llm.toml".to_string()];
    for template in [
        "code_agent",
        "explore_agent",
        "general_agent",
        "lead_agent",
        "planning_agent",
        "research_agent",
        "review_agent",
        "system_agent",
        "writing_agent",
    ] {
        shipped.push(format!("config/agents/{template}.md"));
    }
    for path in &shipped {
        let text = std::fs::read_to_string(root.join(path))
            .unwrap_or_else(|e| panic!("{path} is readable: {e}"));
        for id in REMOVED_ROWS {
            let quoted = format!("\"{id}\"");
            assert!(!text.contains(&quoted), "{path} names {id}");
        }
    }
    let defaults = crate::config::LlmRuntimeConfig::default();
    for (provider, d) in &defaults.provider_defaults {
        assert!(
            !REMOVED_ROWS.contains(&d.default_model.as_str()),
            "{provider}'s default names a removed row"
        );
    }
}

/// MODELS-LATEST (the design's §4.9.2, ML-1): the catalogue holds the
/// eight latest models, undated, and no other row. Red on the parent, whose
/// eleven rows hold two of them.
#[test]
fn the_catalogue_holds_the_eight_latest_models_and_no_other() {
    let registry = ModelRegistry::with_defaults();
    let mut compiled = registry.model_ids();
    compiled.sort();
    let mut latest: Vec<String> = LATEST.iter().map(|id| id.to_string()).collect();
    latest.sort();
    assert_eq!(compiled, latest);
}

/// The eight rows the owner agreed on 2026-11-14 ("I agree on the new
/// models"): Anthropic's "Latest models" table (the models overview) and
/// OpenAI's "Flagship models" (`.../models/all`).
const LATEST: [&str; 8] = [
    "claude-fable-5-2",
    "claude-opus-5-5",
    "claude-sonnet-5-5",
    "claude-haiku-5-5",
    "gpt-6-astra",
    "gpt-6.1-sol",
    "gpt-6.1-terra",
    "gpt-6-luna",
];

/// The nine rows the directive retires (§4.9.7): every older row the
/// parent's catalogue held.
const RETIRED_ROWS: [&str; 9] = [
    "claude-opus-4-6",
    "claude-sonnet-4-6",
    "claude-sonnet-4-5-20250929",
    "claude-haiku-4-5-20251001",
    "gpt-5.2",
    "gpt-5-mini",
    "gpt-5-nano",
    "gpt-4o-mini",
    "o3",
];

/// None of the nine is a compiled row, and re-enabling a provider puts none
/// back (§4.9.7). Red on the parent, which lists all nine.
#[test]
fn a_retired_id_is_not_a_compiled_row() {
    let registry = ModelRegistry::with_defaults();
    for id in RETIRED_ROWS {
        assert!(
            registry.get_model_info(id).is_none(),
            "{id} is retired and has no compiled row"
        );
    }
    let registry = ModelRegistry::new(HashMap::new());
    registry.restore_defaults_for_provider(&ProviderType::OpenAI);
    registry.restore_defaults_for_provider(&ProviderType::Anthropic);
    for id in RETIRED_ROWS {
        assert!(
            registry.get_model_info(id).is_none(),
            "{id} is not restored"
        );
    }
}

/// One row as its pages print it (§4.9.2): $ per million in and out, the
/// cache multiples, the long-prompt rate, the window and what it takes.
struct Page {
    id: &'static str,
    provider: ProviderType,
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
    /// (above this many input tokens, $ in, $ out)
    long: Option<(u32, f64, f64)>,
    window: u32,
    image: bool,
    document: bool,
    audio: bool,
    reasoning: bool,
}

/// The pages read 2026-11-14. Anthropic: the pricing page's "Model pricing"
/// table, its footnotes ("0.025x" on Fable 5.2, "0.05x" on Opus 5.5 and
/// Sonnet 5.5, "the standard 0.1x multiplier" on the rest) and "Long
/// context pricing" (Haiku 5.5 "priced by prompt length": over 100,000
/// tokens $0.50 / $2.50); each model's overview ("Context window: 1M
/// tokens"); `.../build-with-claude/pdf-support` ("All active models
/// support PDF processing"). OpenAI: each model's page ("Input", "Cached
/// input", "Cache writes", "Output"; "1,050,000 context window"; "Input
/// modalities: text, image"; "Reasoning token support"; "Prompts with more
/// than 272K input tokens are priced at 2x input and cache rates and 1.5x
/// output for the full request") and the pricing page's "Long context"
/// columns.
fn pages() -> [Page; 8] {
    let anthropic = |id, input, output, cache_read, long| Page {
        id,
        provider: ProviderType::Anthropic,
        input,
        output,
        cache_read,
        cache_write: 1.25,
        long,
        window: 1_000_000,
        image: true,
        document: true,
        audio: false,
        reasoning: false,
    };
    let openai = |id, input, output, cache_read, long_in, long_out| Page {
        id,
        provider: ProviderType::OpenAI,
        input,
        output,
        cache_read,
        cache_write: 1.25,
        long: Some((272_000, long_in, long_out)),
        window: 1_050_000,
        image: true,
        document: false,
        audio: false,
        reasoning: true,
    };
    [
        anthropic("claude-fable-5-2", 8.0, 40.0, 0.025, None),
        anthropic("claude-opus-5-5", 4.0, 20.0, 0.05, None),
        anthropic("claude-sonnet-5-5", 2.0, 10.0, 0.05, None),
        anthropic(
            "claude-haiku-5-5",
            0.10,
            0.50,
            0.10,
            Some((100_000, 0.50, 2.50)),
        ),
        openai("gpt-6-astra", 10.0, 50.0, 0.10, 20.0, 75.0),
        openai("gpt-6.1-sol", 2.0, 10.0, 0.05, 4.0, 15.0),
        openai("gpt-6.1-terra", 1.0, 5.0, 0.05, 2.0, 7.50),
        openai("gpt-6-luna", 0.10, 0.50, 0.10, 0.20, 0.75),
    ]
}

/// ML-1, ML-2: each row carries its page's prices, cache multiples,
/// long-prompt rate, window and content support. Red on the parent, which
/// lacks six of the eight rows.
#[test]
fn each_latest_row_carries_its_pages_figures() {
    let registry = ModelRegistry::with_defaults();
    for page in pages() {
        let id = page.id;
        let info = row(&registry, id);
        assert_eq!(info.provider, page.provider, "{id}");
        assert_price(&info, id, page.input, page.output);
        assert_eq!(
            info.cache_rates,
            Some(CacheRates {
                read: page.cache_read,
                write: page.cache_write,
            }),
            "{id}'s cache multiples"
        );
        let long = info.long_prompt.map(|l| {
            (
                l.above_input_tokens,
                l.input_price_per_million,
                l.output_price_per_million,
            )
        });
        assert_eq!(long, page.long, "{id}'s long-prompt rate");
        assert_eq!(info.context_window, page.window, "{id}'s window");
        assert_eq!(info.supports_image, page.image, "{id} image");
        assert_eq!(info.supports_document, page.document, "{id} PDF");
        assert_eq!(info.supports_audio, page.audio, "{id} audio");
        assert_eq!(info.supports_reasoning, page.reasoning, "{id} reasoning");
        assert!(info.supports_tools, "{id} can be given tools");
        assert!(info.declared && !info.discovered, "{id} is a compiled row");
    }
}

/// ML-4 (§4.9.3): each row's request contract is its pages'. Anthropic,
/// read 2026-11-14: `.../build-with-claude/thinking` ("Sampling
/// parameters": non-default `temperature`, `top_p` or `top_k` "return a 400
/// error on every request" on all four; forced tool use rejected "on every
/// request" by Opus 5.5, Sonnet 5.5 and Fable 5.2; `"omitted"` the display
/// default on all four); `.../thinking-troubleshooting`'s table (Fable 5.2
/// and Opus 5.5 "Adaptive only", "Always on", rejecting `"enabled"` and
/// `"disabled"`; Sonnet 5.5 rejecting both, `between_tools` "in place of
/// `"disabled"`"; Haiku 5.5 accepting `"disabled"` at effort `high` or
/// below); the Haiku 5.5 migration guide ("accepts a forced `tool_choice`").
/// OpenAI (`.../guides/latest-model`): "When reasoning effort is not
/// `none`, remove `temperature`, `top_p`, and `top_logprobs`" — the arm
/// sends no effort, so each model reasons at its default. Red on the
/// parent, whose `gpt-6.1-sol` carries none.
#[test]
fn each_latest_row_carries_its_pages_request_contract() {
    let registry = ModelRegistry::with_defaults();
    let all = RequestContract {
        no_sampling_parameters: true,
        thinking_always_on: true,
        adaptive_thinking_only: true,
        no_forced_tool_choice: true,
        summarized_thinking: true,
    };
    let haiku = RequestContract {
        thinking_always_on: false,
        no_forced_tool_choice: false,
        ..all
    };
    let openai = RequestContract {
        no_sampling_parameters: true,
        ..RequestContract::NONE
    };
    for (id, contract) in [
        ("claude-fable-5-2", all),
        ("claude-opus-5-5", all),
        ("claude-sonnet-5-5", all),
        ("claude-haiku-5-5", haiku),
        ("gpt-6-astra", openai),
        ("gpt-6.1-sol", openai),
        ("gpt-6.1-terra", openai),
        ("gpt-6-luna", openai),
    ] {
        assert_eq!(row(&registry, id).request_contract, contract, "{id}");
    }
}

/// Haiku 5.5's long-prompt rate (the pricing page: "a request whose prompt
/// is over 100,000 tokens pays higher prices. A request's prompt length
/// counts all of its input tokens, including cache reads and cache
/// writes"): a request of 100,000 input tokens, 60,000 of them cache reads
/// and 30,000 cache writes, pays the standard rate; one more token and the
/// whole request, cache included, pays $0.50 / $2.50 (reads $0.05, 5-minute
/// writes $0.625). Red on the parent, which has no Haiku 5.5 row.
#[test]
fn haiku_5_5_pays_its_upper_rates_once_the_prompt_passes_100000_tokens() {
    let tracker = crate::routing::cost_tracker::CostTracker::new(ModelRegistry::with_defaults());
    let cost = |input, reads, writes| {
        tracker.calculate_cost_with_cache("claude-haiku-5-5", input, 1_000_000, writes, reads)
    };
    // At the threshold: 10,000 uncached at $0.10, 30,000 written at $0.125,
    // 60,000 read at $0.01, a million out at $0.50.
    let at = 10_000.0 * 0.10 + 30_000.0 * 0.125 + 60_000.0 * 0.01 + 1_000_000.0 * 0.50;
    let got = cost(100_000, 60_000, 30_000);
    assert!(
        (got - at / 1_000_000.0).abs() < EPS,
        "at 100,000 the standard rate holds: {got}"
    );
    // Past it: the same, one more uncached token, at the upper rates.
    let above = 10_001.0 * 0.50 + 30_000.0 * 0.625 + 60_000.0 * 0.05 + 1_000_000.0 * 2.50;
    let got = cost(100_001, 60_000, 30_000);
    assert!(
        (got - above / 1_000_000.0).abs() < EPS,
        "past 100,000 the whole request pays the upper rates: {got}"
    );
}

/// CACHE-MULT and CACHE-WRITE (the owner's items 9 and 10, 2026-11-14):
/// every compiled row prices a cache read and a cache write as its page
/// does, both given here in dollars per million and re-read on 2026-11-14
/// for MODELS-LATEST's rows.
///
/// Anthropic (platform.claude.com/docs/en/about-claude/pricing, "Model
/// pricing"): reads ("Cache hits and refreshes") $0.20 on Fable 5.2 and
/// Opus 5.5, $0.10 on Sonnet 5.5, $0.01 on Haiku 5.5; writes at the "5m
/// cache writes" column, the lifetime the arm's `cache_control: {"type":
/// "ephemeral"}` gets ("By default, the cache has a 5-minute lifetime",
/// .../build-with-claude/prompt-caching).
///
/// OpenAI (developers.openai.com/api/docs/pricing and each model's page):
/// "Cached input" $1, $0.10, $0.05 and $0.01, "Cache writes" $12.50, $2.50,
/// $1.25 and $0.125, the default `"30m"` lifetime the arm gets by sending
/// no `prompt_cache_options` (.../guides/prompt-caching, "Cache lifetime").
///
/// The probe is 10,000 tokens, below every long-prompt threshold (Haiku
/// 5.5's 100,000, the OpenAI rows' 272,000).
#[test]
fn every_row_prices_its_cache_reads_and_writes_as_its_page_does() {
    // (id, cache read, cache write), $ per million tokens.
    const PAGES: [(&str, f64, f64); 8] = [
        ("claude-fable-5-2", 0.20, 10.00),
        ("claude-opus-5-5", 0.20, 5.00),
        ("claude-sonnet-5-5", 0.10, 2.50),
        ("claude-haiku-5-5", 0.01, 0.125),
        ("gpt-6-astra", 1.00, 12.50),
        ("gpt-6.1-sol", 0.10, 2.50),
        ("gpt-6.1-terra", 0.05, 1.25),
        ("gpt-6-luna", 0.01, 0.125),
    ];
    let registry = ModelRegistry::with_defaults();
    let mut compiled = registry.model_ids();
    compiled.sort();
    let mut pinned: Vec<String> = PAGES.iter().map(|(id, ..)| id.to_string()).collect();
    pinned.sort();
    assert_eq!(compiled, pinned, "every compiled row is pinned here");

    let tracker = crate::routing::cost_tracker::CostTracker::new(ModelRegistry::with_defaults());
    let tokens = 10_000;
    for (id, read, write) in PAGES {
        let got = tracker.calculate_cost_with_cache(id, tokens, 0, 0, tokens);
        assert!(
            (got - read / 100.0).abs() < EPS,
            "{id}: a cache read costs ${read} a million, not ${}",
            got * 100.0
        );
        let got = tracker.calculate_cost_with_cache(id, tokens, 0, tokens, 0);
        assert!(
            (got - write / 100.0).abs() < EPS,
            "{id}: a cache write costs ${write} a million, not ${}",
            got * 100.0
        );
    }
}

/// The seeded `llm.toml` and every compiled default name a compiled row,
/// with the values of the design's §4.9.5: `[orchestrator] model`
/// `claude-haiku-5-5`, its `fallback_models` `claude-sonnet-5-5`, the
/// Anthropic default `claude-sonnet-5-5` and the OpenAI default
/// `gpt-6-luna` (the deprecations page names it GPT-4o mini's
/// replacement), and `LlmRuntimeConfig::default`'s two cloud rows. Red on
/// the parent, whose seed names four retired ids.
#[test]
fn the_seeded_llm_toml_and_the_compiled_defaults_name_latest_rows() {
    const TEMPLATE: &str = include_str!("../../../../../scripts/release/templates/config/llm.toml");
    let config: crate::config::LlmRouterConfig =
        toml::from_str(TEMPLATE).expect("the seeded template parses");
    let registry = ModelRegistry::with_defaults();
    let compiled = |id: &str| {
        assert!(
            registry.get_model_info(id).is_some(),
            "{id} is a compiled row"
        );
        id.to_string()
    };

    let orchestrator = config.orchestrator.as_ref().expect("[orchestrator]");
    assert_eq!(compiled(&orchestrator.model), "claude-haiku-5-5");
    let fallbacks: Vec<String> = orchestrator
        .fallback_models
        .iter()
        .flatten()
        .map(|id| compiled(id))
        .collect();
    assert_eq!(fallbacks, ["claude-sonnet-5-5"]);
    let providers = config.providers.as_ref().expect("providers");
    let seeded = |name: &str| {
        providers[name]
            .default_model
            .clone()
            .expect("a seeded default_model")
    };
    assert_eq!(compiled(&seeded("anthropic")), "claude-sonnet-5-5");
    assert_eq!(compiled(&seeded("openai")), "gpt-6-luna");
    // Ollama's is empty: "whatever is installed".
    assert_eq!(seeded("ollama"), "");

    let defaults = crate::config::LlmRuntimeConfig::default();
    let runtime = |name: &str| defaults.provider_defaults[name].default_model.clone();
    assert_eq!(compiled(&runtime("anthropic")), "claude-sonnet-5-5");
    assert_eq!(compiled(&runtime("openai")), "gpt-6-luna");
    // The orchestrator default the router, the settings service and the CLI
    // fill in (ML-8).
    assert_eq!(
        compiled(crate::config::DEFAULT_ORCHESTRATOR_MODEL),
        "claude-sonnet-5-5"
    );
    for id in RETIRED_ROWS {
        let quoted = format!("\"{id}\"");
        assert!(!TEMPLATE.contains(&quoted), "the seed names {id}");
    }
}
