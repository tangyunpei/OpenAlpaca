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
