use super::*;

#[test]
fn test_resolve_provider_anthropic() {
    let registry = ModelRegistry::with_defaults();
    assert_eq!(
        registry.resolve_provider("claude-sonnet-5-5"),
        Some(ProviderType::Anthropic)
    );
}

#[test]
fn test_resolve_provider_openai() {
    let registry = ModelRegistry::with_defaults();
    assert_eq!(
        registry.resolve_provider("gpt-6.1-terra"),
        Some(ProviderType::OpenAI)
    );
}

#[test]
fn test_resolve_unknown() {
    let registry = ModelRegistry::with_defaults();
    assert_eq!(registry.resolve_provider("unknown-model"), None);
}

#[test]
fn test_get_pricing() {
    let registry = ModelRegistry::with_defaults();
    let pricing = registry.get_pricing("claude-sonnet-5-5").unwrap();
    assert!((pricing.input_price_per_million - 2.0).abs() < 0.01);
    assert!((pricing.output_price_per_million - 10.0).abs() < 0.01);
}

#[test]
fn test_custom_registry() {
    let mut models = HashMap::new();
    models.insert(
        "my-model".to_string(),
        ModelInfo {
            provider: ProviderType::Ollama,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 4096,
            discovered: false,
            supports_image: false,
            supports_audio: false,
            supports_document: false,
            supports_reasoning: false,
            supports_tools: true,
            declared: true,
            long_prompt: None,
            cache_rates: None,
            request_contract: RequestContract::NONE,
        },
    );
    let registry = ModelRegistry::new(models);
    assert_eq!(
        registry.resolve_provider("my-model"),
        Some(ProviderType::Ollama)
    );
    assert_eq!(registry.resolve_provider("gpt-6.1-terra"), None);
}

#[test]
fn test_register_model() {
    let registry = ModelRegistry::new(HashMap::new());
    registry.register(
        "new-model".to_string(),
        ModelInfo {
            provider: ProviderType::OpenAI,
            input_price_per_million: 1.0,
            output_price_per_million: 2.0,
            context_window: 32_000,
            discovered: false,
            supports_image: false,
            supports_audio: false,
            supports_document: false,
            supports_reasoning: false,
            supports_tools: true,
            declared: true,
            long_prompt: None,
            cache_rates: None,
            request_contract: RequestContract::NONE,
        },
    );
    assert_eq!(
        registry.resolve_provider("new-model"),
        Some(ProviderType::OpenAI)
    );
}

#[test]
fn test_register_if_absent() {
    let registry = ModelRegistry::with_defaults();
    // Should not overwrite existing
    registry.register_if_absent(
        "gpt-6.1-terra".to_string(),
        ModelInfo {
            provider: ProviderType::Ollama,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 0,
            discovered: false,
            supports_image: false,
            supports_audio: false,
            supports_document: false,
            supports_reasoning: false,
            supports_tools: true,
            declared: true,
            long_prompt: None,
            cache_rates: None,
            request_contract: RequestContract::NONE,
        },
    );
    assert_eq!(
        registry.resolve_provider("gpt-6.1-terra"),
        Some(ProviderType::OpenAI)
    );

    // Should add new
    registry.register_if_absent(
        "new-model".to_string(),
        ModelInfo {
            provider: ProviderType::Ollama,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 4096,
            discovered: false,
            supports_image: false,
            supports_audio: false,
            supports_document: false,
            supports_reasoning: false,
            supports_tools: true,
            declared: true,
            long_prompt: None,
            cache_rates: None,
            request_contract: RequestContract::NONE,
        },
    );
    assert_eq!(
        registry.resolve_provider("new-model"),
        Some(ProviderType::Ollama)
    );
}

#[test]
fn test_register_discovered() {
    let registry = ModelRegistry::with_defaults();

    // Existing default model should not be discovered
    let info = registry.get_model_info("gpt-6.1-terra").unwrap();
    assert!(!info.discovered);

    // register_discovered marks existing model as discovered, preserves pricing
    registry.register_discovered(
        "gpt-6.1-terra".to_string(),
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
            declared: true,
            long_prompt: None,
            cache_rates: None,
            request_contract: RequestContract::NONE,
        },
    );
    let info = registry.get_model_info("gpt-6.1-terra").unwrap();
    assert!(info.discovered);
    // Pricing preserved from defaults
    assert!((info.input_price_per_million - 1.0).abs() < 0.01);

    // register_discovered inserts new model with discovered=true
    registry.register_discovered(
        "new-api-model".to_string(),
        ModelInfo {
            provider: ProviderType::Anthropic,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 0,
            discovered: false, // even if passed false, method forces true
            supports_image: false,
            supports_audio: false,
            supports_document: false,
            supports_reasoning: false,
            supports_tools: true,
            declared: true,
            long_prompt: None,
            cache_rates: None,
            request_contract: RequestContract::NONE,
        },
    );
    let info = registry.get_model_info("new-api-model").unwrap();
    assert!(info.discovered);
}

#[test]
fn test_list_discovered_models() {
    let registry = ModelRegistry::with_defaults();

    // No models discovered yet → empty
    let discovered = registry.list_discovered_models();
    assert!(discovered.is_empty());

    // Mark one model as discovered
    registry.register_discovered(
        "gpt-6.1-terra".to_string(),
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
            declared: true,
            long_prompt: None,
            cache_rates: None,
            request_contract: RequestContract::NONE,
        },
    );

    let discovered = registry.list_discovered_models();
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].id, "gpt-6.1-terra");

    // list_models still returns all
    let all = registry.list_models();
    assert!(all.len() > 1);
}

#[test]
fn test_list_models() {
    let registry = ModelRegistry::with_defaults();
    let models = registry.list_models();
    assert!(!models.is_empty());
    // Should be sorted by provider then id
    for w in models.windows(2) {
        assert!(
            w[0].provider <= w[1].provider
                || (w[0].provider == w[1].provider && w[0].id <= w[1].id)
        );
    }
}

/// The OpenAI rows report reasoning tokens ("Reasoning token support" on
/// each model's page, read 2026-11-14); the Anthropic rows do not record it.
#[test]
fn the_openai_rows_report_reasoning_tokens() {
    let registry = ModelRegistry::with_defaults();
    for id in ["gpt-6-astra", "gpt-6.1-sol", "gpt-6.1-terra", "gpt-6-luna"] {
        let info = registry.get_model_info(id).expect("a compiled row");
        assert_eq!(info.provider, ProviderType::OpenAI);
        assert!(info.supports_reasoning, "{id} reports reasoning tokens");
    }
    let claude = registry.get_model_info("claude-sonnet-5-5").unwrap();
    assert!(!claude.supports_reasoning);
}

/// `remove` forgets that a `[models]` row of the person's wrote the id: a
/// compiled row put back after it is the compiled row only again, which a
/// provider built to call another server does not route (COMPAT-SERVER).
/// Red with `remove`'s person-row cleanup taken out.
#[test]
fn a_removed_persons_row_leaves_the_restored_compiled_row_compiled_only() {
    let config = HashMap::from([(
        "gpt-6-luna".to_string(),
        crate::config::ModelConfigEntry {
            provider: "openai".to_string(),
            input_price: None,
            output_price: None,
            context: Some(32_768),
            supports_image: None,
            supports_audio: None,
            supports_document: None,
            supports_reasoning: None,
            supports_tools: None,
        },
    )]);
    let registry = ModelRegistry::with_defaults_and_config(&config, &HashSet::new());
    assert!(!registry.is_compiled_only("gpt-6-luna"), "the person's row");

    assert!(registry.remove("gpt-6-luna"));
    assert_eq!(
        registry.restore_defaults_for_provider(&ProviderType::OpenAI),
        1
    );
    assert!(registry.is_compiled_only("gpt-6-luna"));
}

#[test]
fn restoring_a_providers_defaults_puts_back_only_what_is_missing() {
    let registry = ModelRegistry::with_defaults();
    // A discovered entry the owner's daemon learned from the API.
    let mut discovered = registry.get_model_info("gpt-6.1-terra").unwrap();
    discovered.discovered = true;
    discovered.input_price_per_million = 999.0;
    registry.register("gpt-6.1-terra".to_string(), discovered);

    let anthropic_before = registry.model_ids().len() - {
        let removed = registry.remove_by_provider(&ProviderType::OpenAI);
        assert!(removed.contains(&"gpt-6.1-terra".to_string()));
        removed.len()
    };
    assert!(registry.resolve_provider("gpt-6.1-terra").is_none());

    let restored = registry.restore_defaults_for_provider(&ProviderType::OpenAI);
    assert!(restored > 0);
    assert_eq!(
        registry.resolve_provider("gpt-6.1-terra"),
        Some(ProviderType::OpenAI)
    );
    // Anthropic was never touched by either half.
    assert!(registry.resolve_provider("claude-haiku-5-5").is_some());
    assert!(anthropic_before > 0);

    // A second restore is a no-op: what is already there wins, so a
    // rediscovered price is not overwritten by its compiled default.
    let mut priced = registry.get_model_info("gpt-6.1-terra").unwrap();
    priced.input_price_per_million = 42.0;
    registry.register("gpt-6.1-terra".to_string(), priced);
    assert_eq!(registry.restore_defaults_for_provider(&ProviderType::OpenAI), 0);
    assert_eq!(
        registry.get_model_info("gpt-6.1-terra").unwrap().input_price_per_million,
        42.0
    );
}

/// A row a keyed provider's discovery registers for an id the catalogue
/// lacks: unpriced, bare, tool-capable (`refresh_models_for`).
fn bare_discovered(provider: ProviderType) -> ModelInfo {
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
    }
}

/// ML-7 (the design's §4.9.9): the ladder's last rung on a provider is its
/// `fast` row, never the flagship the id order would put first
/// (`claude-fable-5-2` sorts before `claude-haiku-5-5`, `gpt-6-astra`
/// before `gpt-6-luna`) — without discovery and with a discovery that lists
/// it. Red on the parent, whose rung is the id-ascending first.
#[test]
fn the_last_rung_is_the_providers_fast_row_without_discovery_or_listed() {
    let registry = ModelRegistry::with_defaults();
    assert_eq!(
        registry
            .first_model_for_provider(&ProviderType::Anthropic)
            .as_deref(),
        Some("claude-haiku-5-5")
    );
    assert_eq!(
        registry
            .first_model_for_provider(&ProviderType::OpenAI)
            .as_deref(),
        Some("gpt-6-luna")
    );

    // A key and a successful discovery: every listed id is discovered,
    // the flagship and an id sorting before everything among them.
    for id in ["claude-aaa-test", "claude-fable-5-2", "claude-haiku-5-5"] {
        registry.register_discovered(id.to_string(), bare_discovered(ProviderType::Anthropic));
    }
    for id in ["gpt-aaa-test", "gpt-6-astra", "gpt-6-luna"] {
        registry.register_discovered(id.to_string(), bare_discovered(ProviderType::OpenAI));
    }
    assert_eq!(
        registry
            .first_model_for_provider(&ProviderType::Anthropic)
            .as_deref(),
        Some("claude-haiku-5-5")
    );
    assert_eq!(
        registry
            .first_model_for_provider(&ProviderType::OpenAI)
            .as_deref(),
        Some("gpt-6-luna")
    );
}

/// ML-7's bound: a discovery that does not list the `fast` row — a person's
/// own OpenAI-compatible server behind `[providers.openai]`, or an Anthropic
/// list without Haiku 5.5 — keeps a model the provider confirmed ahead of
/// the compiled row it does not serve, as the parent did; among the
/// confirmed ids the id order still holds.
#[test]
fn a_discovery_without_the_fast_row_keeps_a_confirmed_model_first() {
    let registry = ModelRegistry::with_defaults();
    registry.register_discovered(
        "qwen3-8b".to_string(),
        bare_discovered(ProviderType::OpenAI),
    );
    assert_eq!(
        registry
            .first_model_for_provider(&ProviderType::OpenAI)
            .as_deref(),
        Some("qwen3-8b")
    );
    for id in ["claude-aaa-test", "claude-fable-5-2"] {
        registry.register_discovered(id.to_string(), bare_discovered(ProviderType::Anthropic));
    }
    assert_eq!(
        registry
            .first_model_for_provider(&ProviderType::Anthropic)
            .as_deref(),
        Some("claude-aaa-test")
    );
}

/// ML-7's other half (characterization): a provider whose `fast` row is not
/// registered, or that has none (Ollama), keeps the order it had —
/// discovered, then tool-capable, then id ascending.
#[test]
fn without_its_fast_row_a_provider_keeps_the_id_order() {
    let registry = ModelRegistry::new(HashMap::new());
    for id in ["claude-zzz-test", "claude-aaa-test"] {
        registry.register_discovered(id.to_string(), bare_discovered(ProviderType::Anthropic));
    }
    assert_eq!(
        registry
            .first_model_for_provider(&ProviderType::Anthropic)
            .as_deref(),
        Some("claude-aaa-test")
    );
    let mut no_tools = bare_discovered(ProviderType::Ollama);
    no_tools.supports_tools = false;
    registry.register_discovered("a-embed:latest".to_string(), no_tools);
    registry.register_discovered(
        "qwen3:8b".to_string(),
        bare_discovered(ProviderType::Ollama),
    );
    assert_eq!(
        registry
            .first_model_for_provider(&ProviderType::Ollama)
            .as_deref(),
        Some("qwen3:8b")
    );
}
