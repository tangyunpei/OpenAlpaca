use super::*;

#[test]
fn test_config_from_toml() {
    let toml_str = r#"
provider = "anthropic"
model = "claude-sonnet-4-5-20250929"
max_tokens = 4096
"#;
    let config: LlmConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.provider, "anthropic");
    assert_eq!(config.model.as_deref(), Some("claude-sonnet-4-5-20250929"));
    assert_eq!(config.max_tokens, Some(4096));
    assert!(config.api_key.is_none());
}

#[test]
fn test_resolve_api_key_env() {
    let config = LlmConfig {
        provider: "anthropic".to_string(),
        model: None,
        api_key: None,
        base_url: None,
        max_tokens: None,
    };
    // Without env var set, resolve returns None (unless env is set externally)
    // We just verify the method doesn't panic
    let _ = config.resolve_api_key();
}

#[test]
fn test_resolve_api_key_config_value() {
    let config = LlmConfig {
        provider: "anthropic".to_string(),
        model: None,
        api_key: Some("sk-test-key".to_string()),
        base_url: None,
        max_tokens: None,
    };
    assert_eq!(config.resolve_api_key(), Some("sk-test-key".to_string()));
}

#[test]
fn test_build_provider_unknown() {
    let config = LlmConfig {
        provider: "unknown_provider".to_string(),
        model: None,
        api_key: None,
        base_url: None,
        max_tokens: None,
    };
    let result = build_provider(&config);
    assert!(result.is_err());
    let err = result.err().unwrap();
    match err {
        LlmError::UnknownProvider(name) => assert_eq!(name, "unknown_provider"),
        other => panic!("Expected UnknownProvider, got: {:?}", other),
    }
}

#[test]
fn test_detect_legacy_format() {
    let toml_str = r#"
provider = "anthropic"
model = "claude-sonnet-4-5-20250929"
max_tokens = 4096
"#;
    let raw: toml::Value = toml::from_str(toml_str).unwrap();
    assert!(raw.get("providers").is_none());
}

#[test]
fn test_detect_hierarchical_format() {
    let toml_str = r#"
[orchestrator]
model = "claude-sonnet-4-5-20250929"

[providers.anthropic]
enabled = true

[[providers.anthropic.keys]]
id = "key1"
secret_env = "ANTHROPIC_API_KEY"
"#;
    let raw: toml::Value = toml::from_str(toml_str).unwrap();
    assert!(raw.get("providers").is_some());

    // Verify it parses as LlmRouterConfig
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
monthly_budget = 100.0

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
