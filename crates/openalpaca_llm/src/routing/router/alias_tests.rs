//! The model aliases `fast`, `balanced` and `strong` (C7; the subagent
//! design's M-2…M-5): a compiled id per provider, pinnable per provider in
//! `llm.toml`, resolved against the parent's model and never moved to
//! another provider by the router itself.
//!
//! Every router here is built by hand over stand-in providers that answer
//! nothing: resolution reads the catalogue and the loaded set, never a
//! provider's API.

use super::*;
use crate::config::{LlmRouterConfig, LlmRuntimeConfig};
use crate::error::LlmError;
use crate::routing::model_alias::{ALIAS_TABLE, ModelAlias, ModelChoice};
use crate::routing::model_registry::ModelInfo;
use crate::types::{ChatRequest, ChatResponse};
use async_trait::async_trait;
use std::collections::HashSet;

/// A loaded provider that is never called.
struct Standing(&'static str);

#[async_trait]
impl LlmProvider for Standing {
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

fn name_of(provider: &ProviderType) -> &'static str {
    match provider {
        ProviderType::Anthropic => "anthropic",
        ProviderType::OpenAI => "openai",
        ProviderType::Ollama => "ollama",
        ProviderType::Plugin(_) => "plugin",
    }
}

/// A router with `loaded` switched on, each provider's compiled rows in the
/// catalogue (the disabled ones contribute none, R58b), and `default` the
/// `[orchestrator] model`.
fn router(loaded: &[ProviderType], default: &str) -> LlmRouter {
    let mut providers = HashMap::new();
    for provider in loaded {
        let key_pool = if provider.is_local() {
            KeyPool::new(vec![], SelectionStrategy::RoundRobin)
        } else {
            KeyPool::new(
                vec![ApiKey::new(
                    "test".to_string(),
                    provider.clone(),
                    "sk-test-not-a-key".to_string(),
                )],
                SelectionStrategy::RoundRobin,
            )
        };
        providers.insert(
            provider.clone(),
            ProviderEntry {
                provider: Arc::new(Standing(name_of(provider))),
                key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
            },
        );
    }
    let disabled: HashSet<ProviderType> = ProviderType::all()
        .iter()
        .filter(|p| !loaded.contains(p))
        .cloned()
        .collect();
    LlmRouter::new(
        providers,
        ModelRegistry::with_defaults_and_config(&HashMap::new(), &disabled),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        default.to_string(),
    )
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
        },
    );
}

/// What a load or a hot reload of this `llm.toml` hands the router.
fn load(router: &LlmRouter, llm_toml: &str) {
    let config: LlmRouterConfig = toml::from_str(llm_toml).expect("llm.toml parses");
    router.reload_runtime_config(LlmRuntimeConfig::from(&config));
}

fn resolved(router: &LlmRouter, alias: ModelAlias, parent: Option<&str>) -> String {
    router
        .resolve_alias(alias, parent)
        .unwrap_or_else(|| panic!("{alias} resolves"))
        .model
}

fn lines_containing(lines: &[&str], needle: &str) -> usize {
    lines.iter().filter(|l| l.contains(needle)).count()
}

// ── the words and the table ─────────────────────────────────────────────────

/// M-2: a template's or a call's value parses case-insensitively; no key, an
/// empty value and `inherit` are `inherit`; anything else is an id, kept as
/// written. `haiku`, `sonnet` and `opus` are ids, not synonyms (M-2a).
#[test]
fn a_model_value_parses_into_inherit_an_alias_or_an_id() {
    assert_eq!(ModelChoice::parse("inherit"), ModelChoice::Inherit);
    assert_eq!(ModelChoice::parse(" Inherit "), ModelChoice::Inherit);
    assert_eq!(ModelChoice::parse(""), ModelChoice::Inherit);
    assert_eq!(
        ModelChoice::parse("FAST"),
        ModelChoice::Alias(ModelAlias::Fast)
    );
    assert_eq!(
        ModelChoice::parse("Balanced"),
        ModelChoice::Alias(ModelAlias::Balanced)
    );
    assert_eq!(
        ModelChoice::parse("strong"),
        ModelChoice::Alias(ModelAlias::Strong)
    );
    assert_eq!(
        ModelChoice::parse("claude-opus-5-5"),
        ModelChoice::Id("claude-opus-5-5".to_string())
    );
    assert_eq!(
        ModelChoice::parse("sonnet"),
        ModelChoice::Id("sonnet".to_string())
    );
    for alias in ModelAlias::ALL {
        assert_eq!(ModelAlias::parse(alias.as_str()), Some(alias));
        assert_eq!(alias.to_string(), alias.as_str());
    }
}

/// §4.3: every id the table names has a catalogue row of the same provider
/// and can be given tools (a subagent needs them). It also holds no row
/// O-17 (a) removed.
#[test]
fn every_alias_table_id_is_in_the_catalogue_and_supports_tools() {
    let registry = ModelRegistry::with_defaults();
    for row in ALIAS_TABLE.iter() {
        for alias in ModelAlias::ALL {
            let id = row.model(alias);
            let info = registry
                .get_model_info(id)
                .unwrap_or_else(|| panic!("{alias} on {}: {id} is catalogued", row.provider));
            assert_eq!(info.provider, row.provider, "{id}");
            assert!(info.supports_tools, "{id} can be given tools");
        }
    }
    let providers: Vec<_> = ALIAS_TABLE.iter().map(|r| r.provider.clone()).collect();
    assert_eq!(
        providers,
        vec![ProviderType::Anthropic, ProviderType::OpenAI]
    );
}

// ── M-4: resolution ─────────────────────────────────────────────────────────

/// Step 2: the parent's provider's compiled id — on Anthropic, Haiku 4.5,
/// Sonnet 4.6 and the latest Opus (O-13 (a)).
#[test]
fn an_alias_resolves_to_the_parent_providers_model() {
    let router = router(&[ProviderType::Anthropic], "claude-haiku-4-5-20251001");
    let parent = Some("claude-haiku-4-5-20251001");
    assert_eq!(
        resolved(&router, ModelAlias::Fast, parent),
        "claude-haiku-4-5-20251001"
    );
    assert_eq!(
        resolved(&router, ModelAlias::Balanced, parent),
        "claude-sonnet-4-6"
    );
    let strong = router
        .resolve_alias(ModelAlias::Strong, parent)
        .expect("strong resolves");
    assert_eq!(strong.model, "claude-opus-5-5");
    assert!(strong.passed_over.is_empty());
}

/// OpenAI's row as built: the holding row until C7b and O-16 (M-3a).
#[test]
fn an_alias_on_an_openai_only_router_resolves_to_openais() {
    let router = router(&[ProviderType::OpenAI], "gpt-4o-mini");
    assert_eq!(resolved(&router, ModelAlias::Fast, None), "gpt-5-nano");
    assert_eq!(resolved(&router, ModelAlias::Balanced, None), "gpt-5-mini");
    assert_eq!(resolved(&router, ModelAlias::Strong, None), "gpt-5.2");
}

/// Two keyed providers loaded: the parent's provider's row wins, whatever
/// the main model is.
#[test]
fn an_alias_prefers_the_parent_providers_row() {
    let router = router(
        &[ProviderType::Anthropic, ProviderType::OpenAI],
        "claude-haiku-4-5-20251001",
    );
    assert_eq!(
        resolved(&router, ModelAlias::Fast, Some("gpt-5-mini")),
        "gpt-5-nano"
    );
    assert_eq!(
        resolved(&router, ModelAlias::Strong, Some("claude-sonnet-4-6")),
        "claude-opus-5-5"
    );
}

/// Step 3 on a mixed install: the parent runs on Ollama, which has no row,
/// and OpenAI, which has one, is loaded. The alias stays on the parent's
/// model — no step to another provider — and that is said once per
/// (alias, provider) pair at INFO.
#[tokio::test]
#[tracing_test::traced_test]
async fn an_alias_never_leaves_the_parents_provider() {
    let router = router(&[ProviderType::Ollama, ProviderType::OpenAI], "qwen3:8b");
    pulled(&router, "qwen3:8b");
    for _ in 0..3 {
        let resolution = router
            .resolve_alias(ModelAlias::Balanced, Some("qwen3:8b"))
            .expect("balanced resolves");
        assert_eq!(resolution.model, "qwen3:8b");
        assert!(resolution.passed_over.is_empty());
    }
    assert!(router.substitutions_logged.is_empty(), "not a substitution");
    logs_assert(|lines: &[&str]| {
        let said = lines
            .iter()
            .filter(|l| {
                l.contains("INFO")
                    && l.contains(
                        "Model alias 'balanced' has no model on ollama: it runs on qwen3:8b, \
                         the parent's model",
                    )
                    && l.contains("[providers.ollama.model_aliases]")
            })
            .count();
        match said {
            1 => Ok(()),
            n => Err(format!("announced once at INFO, not {n} times")),
        }
    });
}

/// Step 3 where no loaded provider has a row (Ollama only): every alias is
/// the parent's model, and nothing is logged — it is the table's own answer.
/// With no parent it is the effective default.
#[tokio::test]
#[tracing_test::traced_test]
async fn an_alias_no_provider_maps_collapses_to_the_parent_model() {
    let router = router(&[ProviderType::Ollama], "qwen3:8b");
    pulled(&router, "qwen3:8b");
    pulled(&router, "qwen3:4b");
    for alias in ModelAlias::ALL {
        assert_eq!(resolved(&router, alias, Some("qwen3:4b")), "qwen3:4b");
        assert_eq!(resolved(&router, alias, None), "qwen3:8b");
    }
    assert!(router.substitutions_logged.is_empty());
    assert!(!logs_contain("Model alias"));
}

/// Step 3 with a parent that is not routable while something else is: the
/// alias answers the effective default, as `inherit` would — whether the
/// parent has no catalogue row (an Anthropic id on an Ollama-only install)
/// or has one whose provider is not loaded (a pulled tag once Ollama is
/// off).
#[test]
fn an_alias_on_a_parent_that_is_not_routable_answers_the_effective_default() {
    let ollama_only = router(&[ProviderType::Ollama], "qwen3:8b");
    pulled(&ollama_only, "qwen3:8b");
    assert!(!ollama_only.is_routable("claude-sonnet-4-6"));
    for alias in ModelAlias::ALL {
        let resolution = ollama_only
            .resolve_alias(alias, Some("claude-sonnet-4-6"))
            .unwrap_or_else(|| panic!("{alias} resolves"));
        assert_eq!(resolution.model, "qwen3:8b");
        assert!(resolution.passed_over.is_empty());
    }

    let anthropic_only = router(&[ProviderType::Anthropic], "claude-haiku-4-5-20251001");
    pulled(&anthropic_only, "qwen3:8b");
    assert!(!anthropic_only.is_routable("qwen3:8b"));
    for alias in ModelAlias::ALL {
        assert_eq!(
            resolved(&anthropic_only, alias, Some("qwen3:8b")),
            "claude-haiku-4-5-20251001",
            "{alias} on a parent whose provider is off"
        );
    }
}

/// M-5's `INFO` names the model step 3 answers for what it is: the
/// parent's model where the parent is routable, the default where it is
/// not (a pulled tag once Ollama is off, Anthropic loaded) — never the
/// default called the parent's model. Each once per (alias, provider).
#[tokio::test]
#[tracing_test::traced_test]
async fn a_collapse_line_calls_the_default_the_default_not_the_parents_model() {
    let mixed = router(
        &[ProviderType::Ollama, ProviderType::Anthropic],
        "claude-haiku-4-5-20251001",
    );
    pulled(&mixed, "qwen3:8b");
    assert_eq!(
        resolved(&mixed, ModelAlias::Fast, Some("qwen3:8b")),
        "qwen3:8b"
    );

    let ollama_off = router(&[ProviderType::Anthropic], "claude-haiku-4-5-20251001");
    pulled(&ollama_off, "qwen3:8b");
    for _ in 0..3 {
        assert_eq!(
            resolved(&ollama_off, ModelAlias::Balanced, Some("qwen3:8b")),
            "claude-haiku-4-5-20251001"
        );
    }

    logs_assert(|lines: &[&str]| {
        let parents = lines
            .iter()
            .filter(|l| {
                l.contains("INFO")
                    && l.contains(
                        "Model alias 'fast' has no model on ollama: it runs on qwen3:8b, \
                         the parent's model",
                    )
            })
            .count();
        if parents != 1 {
            return Err(format!("the parent's model said once, not {parents} times"));
        }
        let balanced: Vec<&&str> = lines
            .iter()
            .filter(|l| l.contains("Model alias 'balanced'"))
            .collect();
        if let Some(line) = balanced.iter().find(|l| l.contains("the parent's model")) {
            return Err(format!("the default called the parent's model: {line}"));
        }
        let defaults = balanced
            .iter()
            .filter(|l| {
                l.contains("INFO")
                    && l.contains(
                        "Model alias 'balanced' has no model on ollama: its parent's model \
                         qwen3:8b is not available, so it runs on claude-haiku-4-5-20251001, \
                         the default model",
                    )
                    && l.contains("[providers.ollama.model_aliases]")
            })
            .count();
        match (defaults, balanced.len()) {
            (1, 1) => Ok(()),
            (d, n) => Err(format!("the default said once at INFO, not {d} of {n}")),
        }
    });
}

/// Step 4: nothing routable resolves nothing, with or without a parent.
#[test]
fn nothing_routable_resolves_no_alias() {
    let router = router(&[], "claude-haiku-4-5-20251001");
    for alias in ModelAlias::ALL {
        assert_eq!(router.resolve_alias(alias, None), None);
        assert_eq!(router.resolve_alias(alias, Some("claude-opus-5-5")), None);
    }
}

// ── M-3: the per-provider pins ──────────────────────────────────────────────

/// Step 1: the parent's provider's pin, when routable, before the compiled
/// row.
#[test]
fn a_providers_pin_wins_when_routable() {
    let router = router(&[ProviderType::Anthropic], "claude-haiku-4-5-20251001");
    load(
        &router,
        r#"
[providers.anthropic.model_aliases]
fast = "claude-sonnet-4-6"
"#,
    );
    assert_eq!(
        resolved(&router, ModelAlias::Fast, None),
        "claude-sonnet-4-6"
    );
    assert_eq!(
        resolved(&router, ModelAlias::Balanced, None),
        "claude-sonnet-4-6",
        "an alias with no pin keeps the compiled row"
    );
}

/// "or the user decides": a pin of `strong` replaces the latest Opus.
#[test]
fn a_strong_pin_overrides_the_compiled_row() {
    let router = router(&[ProviderType::Anthropic], "claude-haiku-4-5-20251001");
    load(
        &router,
        r#"
[providers.anthropic.model_aliases]
strong = "claude-sonnet-4-6"
"#,
    );
    let strong = router
        .resolve_alias(ModelAlias::Strong, None)
        .expect("strong resolves");
    assert_eq!(strong.model, "claude-sonnet-4-6");
    assert!(strong.passed_over.is_empty());
}

/// A pin may name another provider's model: the one way an alias leaves
/// its parent's provider, and the person's choice.
#[test]
fn a_pin_naming_another_providers_model_is_used() {
    let router = router(&[ProviderType::Ollama, ProviderType::Anthropic], "qwen3:8b");
    pulled(&router, "qwen3:8b");
    load(
        &router,
        r#"
[providers.ollama.model_aliases]
balanced = "claude-sonnet-4-6"
"#,
    );
    assert_eq!(
        resolved(&router, ModelAlias::Balanced, Some("qwen3:8b")),
        "claude-sonnet-4-6"
    );
    assert_eq!(
        resolved(&router, ModelAlias::Fast, Some("qwen3:8b")),
        "qwen3:8b",
        "an alias Ollama does not pin stays on the parent"
    );
}

/// Only the parent's provider's pin is read: Ollama's pin does not move a
/// parent on OpenAI.
#[test]
fn another_providers_pin_does_not_move_the_parents_resolution() {
    let router = router(&[ProviderType::Ollama, ProviderType::OpenAI], "gpt-5.2");
    pulled(&router, "qwen3:4b");
    load(
        &router,
        r#"
[providers.ollama.model_aliases]
fast = "qwen3:4b"
"#,
    );
    assert_eq!(
        resolved(&router, ModelAlias::Fast, Some("gpt-5.2")),
        "gpt-5-nano"
    );
    assert_eq!(
        resolved(&router, ModelAlias::Fast, Some("qwen3:4b")),
        "qwen3:4b"
    );
}

/// M-5: a pin that is not routable is passed over — the compiled row
/// answers — and that is said once per (pin, answer) pair at WARN.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_pin_that_is_not_routable_is_passed_over_and_announced_once() {
    let router = router(&[ProviderType::Anthropic], "claude-haiku-4-5-20251001");
    load(
        &router,
        r#"
[providers.anthropic.model_aliases]
fast = "claude-not-installed"
"#,
    );
    for _ in 0..3 {
        let fast = router
            .resolve_alias(ModelAlias::Fast, None)
            .expect("fast resolves");
        assert_eq!(fast.model, "claude-haiku-4-5-20251001");
        assert_eq!(fast.passed_over, vec!["claude-not-installed".to_string()]);
    }
    logs_assert(|lines: &[&str]| {
        let warned = lines
            .iter()
            .filter(|l| {
                l.contains("WARN")
                    && l.contains(
                        "Model alias 'fast' would use claude-not-installed, which is not \
                         available — answering with claude-haiku-4-5-20251001",
                    )
                    && l.contains("[providers.anthropic.model_aliases]")
            })
            .count();
        match warned {
            1 => Ok(()),
            n => Err(format!("announced once, not {n} times")),
        }
    });
}

/// M-5: a compiled id that is not routable (its row withdrawn) is passed
/// over too, the alias collapsing to the parent, announced once.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_compiled_id_that_is_not_routable_is_passed_over_and_announced_once() {
    let router = router(&[ProviderType::Anthropic], "claude-haiku-4-5-20251001");
    assert!(router.model_registry().remove("claude-opus-5-5"));
    for _ in 0..2 {
        let strong = router
            .resolve_alias(ModelAlias::Strong, Some("claude-sonnet-4-6"))
            .expect("strong resolves");
        assert_eq!(strong.model, "claude-sonnet-4-6");
        assert_eq!(strong.passed_over, vec!["claude-opus-5-5".to_string()]);
    }
    logs_assert(|lines: &[&str]| {
        match lines_containing(
            lines,
            "Model alias 'strong' would use claude-opus-5-5, which is not available",
        ) {
            1 => Ok(()),
            n => Err(format!("announced once, not {n} times")),
        }
    });
}

/// No `[providers.<name>.model_aliases]` table: the compiled table answers.
#[test]
fn an_llm_toml_without_model_aliases_reads_the_compiled_table() {
    let router = router(&[ProviderType::Anthropic], "claude-haiku-4-5-20251001");
    load(
        &router,
        r#"
[orchestrator]
model = "claude-haiku-4-5-20251001"

[providers.anthropic]
enabled = true
default_model = "claude-sonnet-4-6"
"#,
    );
    assert!(
        router.runtime_config().provider_defaults["anthropic"]
            .model_aliases
            .is_empty()
    );
    assert_eq!(
        resolved(&router, ModelAlias::Strong, None),
        "claude-opus-5-5"
    );
}

/// A hot reload takes a pin, and a later one that drops it gives the
/// compiled row back: the pins live in the runtime config the watcher swaps.
#[test]
fn a_hot_reload_takes_a_pin() {
    let router = router(&[ProviderType::Anthropic], "claude-haiku-4-5-20251001");
    assert_eq!(
        resolved(&router, ModelAlias::Strong, None),
        "claude-opus-5-5"
    );
    load(
        &router,
        r#"
[providers.anthropic.model_aliases]
strong = "claude-opus-4-6"
"#,
    );
    assert_eq!(
        resolved(&router, ModelAlias::Strong, None),
        "claude-opus-4-6"
    );
    load(&router, "[providers.anthropic]\nenabled = true\n");
    assert_eq!(
        resolved(&router, ModelAlias::Strong, None),
        "claude-opus-5-5"
    );
}

// ── M-8's list ──────────────────────────────────────────────────────────────

/// The routable set, sorted: catalogued ids whose provider is loaded, and
/// nothing else.
#[test]
fn routable_models_are_sorted_and_only_routable() {
    let router = router(&[ProviderType::OpenAI], "gpt-4o-mini");
    pulled(&router, "qwen3:8b"); // Ollama is not loaded
    let routable = router.routable_models();
    let mut sorted = routable.clone();
    sorted.sort();
    assert_eq!(routable, sorted);
    assert!(routable.contains(&"gpt-5.2".to_string()));
    assert!(routable.contains(&"gpt-6.1-sol".to_string()));
    assert!(!routable.contains(&"qwen3:8b".to_string()));
    assert!(!routable.iter().any(|id| id.starts_with("claude-")));
    assert!(routable.iter().all(|id| router.is_routable(id)));
}
