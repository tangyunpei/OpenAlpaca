use super::*;

#[test]
fn test_parse_hierarchical_provider_keys() {
    let toml_str = r#"
[orchestrator]
model = "claude-sonnet-4-5-20250929"

[providers.anthropic]
enabled = true

[[providers.anthropic.keys]]
id = "key1"
secret_env = "ANTHROPIC_API_KEY"
"#;
    let config: LlmRouterConfig = toml::from_str(toml_str).unwrap();
    let key = &config.providers.as_ref().unwrap()["anthropic"]
        .keys
        .as_ref()
        .unwrap()[0];
    assert_eq!(key.secret_env.as_deref(), Some("ANTHROPIC_API_KEY"));
}

#[test]
fn test_parse_router_config() {
    let toml_str = r#"
[orchestrator]
model = "claude-sonnet-4-5-20250929"
fallback_models = ["gpt-4o"]

[providers.anthropic]
enabled = true
strategy = "round_robin"

[[providers.anthropic.keys]]
id = "key1"
secret_env = "ANTHROPIC_API_KEY"
tier = "tier1"

[models.custom-model]
provider = "anthropic"
input_price = 5.0
output_price = 25.0
context = 100000

[fallback_chains]
"claude-sonnet-4-5-20250929" = ["gpt-4o"]
"#;
    let config: LlmRouterConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(
        config.orchestrator.as_ref().unwrap().model,
        "claude-sonnet-4-5-20250929"
    );
    assert!(config.providers.as_ref().unwrap().contains_key("anthropic"));
    assert!(config.models.as_ref().unwrap().contains_key("custom-model"));

    let key = &config.providers.as_ref().unwrap()["anthropic"]
        .keys
        .as_ref()
        .unwrap()[0];
    assert_eq!(key.id, "key1");
    assert_eq!(key.tier.as_deref(), Some("tier1"));
}

#[test]
fn test_parse_provider_type_fn() {
    assert_eq!(
        parse_provider_type("anthropic"),
        Some(ProviderType::Anthropic)
    );
    assert_eq!(parse_provider_type("openai"), Some(ProviderType::OpenAI));
    assert_eq!(parse_provider_type("ollama"), Some(ProviderType::Ollama));
    assert_eq!(parse_provider_type("unknown"), None);
}

// ── R58(b): a disabled provider contributes no models ───────────────────────

/// A provider the owner turned off must not be in the catalogue after a
/// restart either — neither its `[models]` rows nor its compiled defaults.
/// Otherwise `GET /v1/models` re-lists it, the picker offers it, and the call
/// fails with `ProviderNotConfigured` (or worse, reaches the provider's CLI
/// backend).
#[test]
fn a_disabled_provider_contributes_no_models_at_boot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("llm.toml");
    std::fs::write(
        &path,
        r#"[orchestrator]
model = "claude-haiku-4-5-20251001"

[providers.openai]
enabled = false

[models."gpt-hand-written"]
provider = "openai"
context = 128000
"#,
    )
    .unwrap();

    let router = build_router(&path).expect("router");
    let registry = router.model_registry();

    assert_eq!(
        registry.resolve_provider("gpt-hand-written"),
        None,
        "the disabled provider's own [models] row must stay out"
    );
    assert_eq!(
        registry.resolve_provider("gpt-5.2"),
        None,
        "and so must its compiled defaults"
    );
    assert!(
        registry.resolve_provider("claude-haiku-4-5-20251001").is_some(),
        "the enabled providers are untouched"
    );
}
