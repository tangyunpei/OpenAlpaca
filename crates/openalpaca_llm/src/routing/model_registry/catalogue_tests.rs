//! The compiled catalogue against the providers' own pages, read on
//! 2026-11-12 (C7: the subagent design's M-3c, and O-17 (a)).
//!
//! Anthropic: `platform.claude.com/docs/en/about-claude/pricing`, the models
//! overview (`/docs/en/models/overview`) and the model-deprecations page.
//! OpenAI: `developers.openai.com/api/docs/pricing`, `.../models/all`, each
//! model's own page and `.../deprecations`.

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

/// M-3c: the two rows whose compiled price was not the page's. Red on the
/// parent's $15 / $75 and $10 / $40.
#[test]
fn the_catalogue_prices_opus_4_6_and_o3_as_their_pages_do() {
    let registry = ModelRegistry::with_defaults();
    assert_price(
        &row(&registry, "claude-opus-4-6"),
        "claude-opus-4-6",
        5.0,
        25.0,
    );
    assert_price(&row(&registry, "o3"), "o3", 2.0, 8.0);
    // The windows stay as listed until step 19's live check (O-17 (a)).
    assert_eq!(row(&registry, "claude-opus-4-6").context_window, 200_000);
    assert_eq!(row(&registry, "o3").context_window, 200_000);
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

/// The rows the pages mark deprecated but still answering are kept
/// (characterization): `gpt-4o-mini` (shut down January 14, 2027; O-18 (c)
/// added it), `o3`, `gpt-5-mini` and `gpt-5-nano` (December 11, 2026) and
/// `claude-sonnet-4-5-20250929` (retiring November 30, 2026).
#[test]
fn the_deprecated_rows_that_still_answer_are_kept() {
    let registry = ModelRegistry::with_defaults();
    for id in [
        "gpt-4o-mini",
        "o3",
        "gpt-5-mini",
        "gpt-5-nano",
        "claude-sonnet-4-5-20250929",
    ] {
        assert!(registry.get_model_info(id).is_some(), "{id} is kept");
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

/// C7a: only a row whose page, read on 2026-11-12, says the model refuses a
/// part of the arm's request carries a contract — today `claude-opus-5-5`
/// alone, which refuses all four. The pages: the Opus 5.5 migration guide
/// (sampling parameters, `disabled` and `enabled` thinking, a forced
/// `tool_choice`); `.../build-with-claude/thinking-troubleshooting`'s
/// per-model table (Opus 4.6 and Sonnet 4.6 reject no thinking type; Haiku
/// 4.5 and Sonnet 4.5 reject only `adaptive`, which the arm's callers never
/// send); `.../build-with-claude/thinking` ("Limits": non-default sampling
/// refused on Claude 4.7 and later, forced tool use on Opus 5.5, Sonnet 5.5
/// and Fable and Mythos 5.1 and 5.2 — neither on the rows below).
#[test]
fn only_opus_5_5_carries_a_request_contract() {
    let registry = ModelRegistry::with_defaults();
    let all = RequestContract {
        no_sampling_parameters: true,
        thinking_always_on: true,
        adaptive_thinking_only: true,
        no_forced_tool_choice: true,
    };
    assert_eq!(row(&registry, "claude-opus-5-5").request_contract, all);
    for id in [
        "claude-opus-4-6",
        "claude-sonnet-4-6",
        "claude-sonnet-4-5-20250929",
        "claude-haiku-4-5-20251001",
        "gpt-5.2",
        "gpt-5-mini",
        "gpt-5-nano",
        "gpt-4o-mini",
        "gpt-6.1-sol",
        "o3",
    ] {
        assert_eq!(
            row(&registry, id).request_contract,
            RequestContract::NONE,
            "{id}"
        );
    }
}

/// CACHE-MULT and CACHE-WRITE (the owner's items 9 and 10, 2026-11-14):
/// every compiled row prices a cache read and a cache write as its page
/// does, both read on 2026-11-14 and given here in dollars per million.
///
/// Anthropic (platform.claude.com/docs/en/about-claude/pricing, "Model
/// pricing"): reads ("Cache hits and refreshes") $0.20 on Opus 5.5 — "0.05x
/// the base input price" — and the "standard 0.1x multiplier" on the rest;
/// writes at the "5m cache writes" column, the lifetime the arm's
/// `cache_control: {"type": "ephemeral"}` gets ("By default, the cache has
/// a 5-minute lifetime", .../build-with-claude/prompt-caching).
///
/// OpenAI (developers.openai.com/api/docs/pricing and each model's page):
/// `gpt-6.1-sol` reads $0.10 ("5% of the uncached input token rate") and
/// writes $2.50 ("Cache writes are billed at 1.25x"), the default `"30m"`
/// lifetime the arm gets by sending no `prompt_cache_options`
/// (.../guides/prompt-caching, "Cache lifetime"); the earlier models read
/// at their "Cached input" price and have "No additional cache-write
/// charge" (the same guide's "Summary of model differences"), so a write
/// costs their input rate.
///
/// Red on the parent, which bills every row's reads at 0.1x and writes at
/// 1.25x its input rate.
#[test]
fn every_row_prices_its_cache_reads_and_writes_as_its_page_does() {
    // (id, cache read, cache write), $ per million tokens.
    const PAGES: [(&str, f64, f64); 11] = [
        ("claude-opus-5-5", 0.20, 5.00),
        ("claude-opus-4-6", 0.50, 6.25),
        ("claude-sonnet-4-6", 0.30, 3.75),
        ("claude-sonnet-4-5-20250929", 0.30, 3.75),
        ("claude-haiku-4-5-20251001", 0.10, 1.25),
        ("gpt-6.1-sol", 0.10, 2.50),
        ("gpt-5.2", 0.175, 1.75),
        ("gpt-5-mini", 0.025, 0.25),
        ("gpt-5-nano", 0.005, 0.05),
        ("gpt-4o-mini", 0.075, 0.15),
        ("o3", 0.50, 2.00),
    ];
    let registry = ModelRegistry::with_defaults();
    let mut compiled = registry.model_ids();
    compiled.sort();
    let mut pinned: Vec<String> = PAGES.iter().map(|(id, ..)| id.to_string()).collect();
    pinned.sort();
    assert_eq!(compiled, pinned, "every compiled row is pinned here");

    // 100,000 tokens, a tenth of the per-million price: below every
    // long-prompt threshold (`gpt-6.1-sol`'s 272,000).
    let tracker = crate::routing::cost_tracker::CostTracker::new(ModelRegistry::with_defaults());
    let tokens = 100_000;
    for (id, read, write) in PAGES {
        let got = tracker.calculate_cost_with_cache(id, tokens, 0, 0, tokens);
        assert!(
            (got - read / 10.0).abs() < EPS,
            "{id}: a cache read costs ${read} a million, not ${}",
            got * 10.0
        );
        let got = tracker.calculate_cost_with_cache(id, tokens, 0, tokens, 0);
        assert!(
            (got - write / 10.0).abs() < EPS,
            "{id}: a cache write costs ${write} a million, not ${}",
            got * 10.0
        );
    }
}
