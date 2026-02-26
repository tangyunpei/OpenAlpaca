use super::*;

#[test]
fn test_resolve_provider_anthropic() {
    let registry = ModelRegistry::with_defaults();
    assert_eq!(
        registry.resolve_provider("claude-sonnet-4-5-20250929"),
        Some(ProviderType::Anthropic)
    );
}

#[test]
fn test_resolve_provider_openai() {
    let registry = ModelRegistry::with_defaults();
    assert_eq!(
        registry.resolve_provider("gpt-5.2"),
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
    let pricing = registry.get_pricing("claude-sonnet-4-5-20250929").unwrap();
    assert!((pricing.input_price_per_million - 3.0).abs() < 0.01);
    assert!((pricing.output_price_per_million - 15.0).abs() < 0.01);
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
        },
    );
    let registry = ModelRegistry::new(models);
    assert_eq!(
        registry.resolve_provider("my-model"),
        Some(ProviderType::Ollama)
    );
    assert_eq!(registry.resolve_provider("gpt-5.2"), None);
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
        "gpt-5.2".to_string(),
        ModelInfo {
            provider: ProviderType::Ollama,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 0,
            discovered: false,
            supports_image: false,
            supports_audio: false,
            supports_document: false,
        },
    );
    assert_eq!(
        registry.resolve_provider("gpt-5.2"),
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
    let info = registry.get_model_info("gpt-5.2").unwrap();
    assert!(!info.discovered);

    // register_discovered marks existing model as discovered, preserves pricing
    registry.register_discovered(
        "gpt-5.2".to_string(),
        ModelInfo {
            provider: ProviderType::OpenAI,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 0,
            discovered: true,
            supports_image: false,
            supports_audio: false,
            supports_document: false,
        },
    );
    let info = registry.get_model_info("gpt-5.2").unwrap();
    assert!(info.discovered);
    // Pricing preserved from defaults
    assert!((info.input_price_per_million - 1.75).abs() < 0.01);

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
        "gpt-5.2".to_string(),
        ModelInfo {
            provider: ProviderType::OpenAI,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 0,
            discovered: true,
            supports_image: false,
            supports_audio: false,
            supports_document: false,
        },
    );

    let discovered = registry.list_discovered_models();
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].id, "gpt-5.2");

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
