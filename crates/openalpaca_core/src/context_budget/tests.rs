use super::budget::{ContextBudgetManager, RenderedSection};
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

#[test]
fn test_budget_computation_basic() {
    let config = ContextBudgetConfig::default();
    let mgr = ContextBudgetManager::new(200_000, &config);
    assert_eq!(mgr.model_context_window(), 200_000);
    assert_eq!(mgr.autocompact_buffer(), 33_000);
    assert_eq!(mgr.fixed_zone_tokens(), 0);
    assert_eq!(mgr.free_zone_capacity(), 167_000);
}

#[test]
fn test_budget_with_fixed_sections() {
    let config = ContextBudgetConfig::default();
    let mut mgr = ContextBudgetManager::new(200_000, &config);
    mgr.register_section("system_prompt", 5_000);
    mgr.register_section("tools", 3_000);
    mgr.register_section("memory", 1_000);
    assert_eq!(mgr.fixed_zone_tokens(), 9_000);
    assert_eq!(mgr.free_zone_capacity(), 158_000);
}

#[test]
fn test_budget_various_models() {
    let config = ContextBudgetConfig::default();
    assert_eq!(
        ContextBudgetManager::new(8_192, &config).autocompact_buffer(),
        1_351
    );
    assert_eq!(
        ContextBudgetManager::new(128_000, &config).autocompact_buffer(),
        21_120
    );
    assert_eq!(
        ContextBudgetManager::new(200_000, &config).autocompact_buffer(),
        33_000
    );
}

#[test]
fn test_compaction_trigger_threshold() {
    let config = ContextBudgetConfig::default();
    let mut mgr = ContextBudgetManager::new(200_000, &config);
    mgr.register_section("system_prompt", 5_000);
    assert!(!mgr.should_compact(161_999));
    assert!(mgr.should_compact(162_000));
    assert!(mgr.should_compact(170_000));
}

#[test]
fn test_compaction_not_triggered_below_threshold() {
    let config = ContextBudgetConfig::default();
    let mgr = ContextBudgetManager::new(200_000, &config);
    assert!(!mgr.should_compact(0));
    assert!(!mgr.should_compact(100_000));
    assert!(!mgr.should_compact(166_999));
}

#[test]
fn test_autocompact_buffer_ratio_config() {
    let mut config = ContextBudgetConfig::default();
    config.autocompact_buffer_ratio = 0.25;
    let mgr = ContextBudgetManager::new(100_000, &config);
    assert_eq!(mgr.autocompact_buffer(), 25_000);
    assert_eq!(mgr.free_zone_capacity(), 75_000);
}

#[test]
fn test_fixed_zone_overflow_warning() {
    let config = ContextBudgetConfig::default();
    let mut mgr = ContextBudgetManager::new(10_000, &config);
    mgr.register_section("huge_prompt", 6_000);
    assert!(mgr.is_fixed_zone_oversized());
}

#[test]
fn test_section_breakdown() {
    let config = ContextBudgetConfig::default();
    let mut mgr = ContextBudgetManager::new(200_000, &config);
    mgr.register_section("system_prompt", 5_000);
    mgr.register_section("tools", 3_000);
    let breakdown = mgr.section_breakdown();
    assert_eq!(breakdown.len(), 2);
    assert_eq!(breakdown[0], ("system_prompt", 5_000));
    assert_eq!(breakdown[1], ("tools", 3_000));
}
