use super::budget::RenderedSection;
use crate::daemon_config::ContextBudgetConfig;

#[test]
fn test_rendered_section_creation() {
    let section = RenderedSection::new("Hello world".to_string());
    assert_eq!(section.content, "Hello world");
    assert_eq!(section.token_estimate, 2);
}

#[test]
fn test_rendered_section_empty() {
    let section = RenderedSection::new(String::new());
    assert_eq!(section.token_estimate, 0);
}

#[test]
fn test_rendered_section_with_explicit_tokens() {
    let section = RenderedSection::with_token_estimate("content".to_string(), 500);
    assert_eq!(section.token_estimate, 500);
    assert_eq!(section.content, "content");
}

#[test]
fn test_context_budget_config_defaults() {
    let config = ContextBudgetConfig::default();
    assert!((config.autocompact_buffer_ratio - 0.165).abs() < f64::EPSILON);
    assert!((config.compaction_target_ratio - 0.50).abs() < f64::EPSILON);
    assert_eq!(config.compaction_model, None);
    assert_eq!(config.max_extractions_per_compaction, 10);
    assert_eq!(config.min_recent_messages, 4);
}

#[test]
fn test_context_budget_config_from_toml() {
    let toml_str = r#"
        autocompact_buffer_ratio = 0.20
        compaction_target_ratio = 0.60
        compaction_model = "claude-haiku-4-5-20251001"
        max_extractions_per_compaction = 5
        min_recent_messages = 6
    "#;
    let config: ContextBudgetConfig = toml::from_str(toml_str).unwrap();
    assert!((config.autocompact_buffer_ratio - 0.20).abs() < f64::EPSILON);
    assert_eq!(
        config.compaction_model.as_deref(),
        Some("claude-haiku-4-5-20251001")
    );
    assert_eq!(config.max_extractions_per_compaction, 5);
}
