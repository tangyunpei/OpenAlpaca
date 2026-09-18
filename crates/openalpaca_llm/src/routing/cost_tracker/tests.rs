use super::*;

fn make_tracker() -> CostTracker {
    CostTracker::new(ModelRegistry::with_defaults())
}

#[test]
fn test_calculate_cost_known_model() {
    let tracker = make_tracker();
    // claude-sonnet: $3/1M input, $15/1M output
    let cost = tracker.calculate_cost("claude-sonnet-4-5-20250929", 1_000_000, 100_000);
    let expected = 3.0 + 1.5; // 1M * $3/1M + 100K * $15/1M
    assert!(
        (cost - expected).abs() < 0.01,
        "cost={}, expected={}",
        cost,
        expected
    );
}

#[test]
fn test_calculate_cost_unknown_model_fallback() {
    let tracker = make_tracker();
    let cost = tracker.calculate_cost("unknown-model", 1_000_000, 100_000);
    let expected = 3.0 + 1.5; // fallback matches sonnet pricing
    assert!((cost - expected).abs() < 0.01);
}

// ── L8: one catalogue, shared with the router ───────────────────────────────

use crate::keys::key_pool::ProviderType;
use crate::routing::model_registry::ModelInfo;
use std::sync::Arc;

fn local_model(context_window: u32) -> ModelInfo {
    ModelInfo {
        provider: ProviderType::Ollama,
        input_price_per_million: 0.0,
        output_price_per_million: 0.0,
        context_window,
        discovered: true,
        supports_image: false,
        supports_audio: false,
        supports_document: false,
        supports_reasoning: false,
        supports_tools: true,
        declared: true,
    }
}

/// A discovered local model is free, and the caps are not spent on fiction.
#[test]
fn a_discovered_local_model_costs_nothing() {
    let registry = Arc::new(ModelRegistry::new(std::collections::HashMap::new()));
    registry.register("qwen3:27b".to_string(), local_model(262_144));

    let tracker = make_tracker();
    tracker.use_registry(Arc::clone(&registry));

    assert_eq!(tracker.calculate_cost("qwen3:27b", 1_000_000, 100_000), 0.0);
    assert_eq!(
        tracker.calculate_cost_with_cache("qwen3:27b", 1_000_000, 100_000, 0, 0),
        0.0
    );
}

/// A `[models]` price is honoured — the tracker used to own a registry that
/// config rows never reached.
#[test]
fn a_declared_price_is_honoured() {
    let registry = Arc::new(ModelRegistry::new(std::collections::HashMap::new()));
    let mut declared = std::collections::HashMap::new();
    declared.insert(
        "priced-local".to_string(),
        crate::config::ModelConfigEntry {
            provider: "ollama".to_string(),
            input_price: Some(0.5),
            output_price: Some(1.5),
            context: Some(8192),
            supports_image: None,
            supports_audio: None,
            supports_document: None,
            supports_reasoning: None,
            supports_tools: None,
        },
    );
    registry.reload_from_config(&declared, &std::collections::HashSet::new());

    let tracker = make_tracker();
    tracker.use_registry(registry);

    let cost = tracker.calculate_cost("priced-local", 1_000_000, 1_000_000);
    assert!((cost - 2.0).abs() < 1e-9, "cost={cost}");
}

/// A model registered after the tracker was built is priced correctly: the
/// registry is shared, not copied.
#[test]
fn a_registry_reload_reaches_the_tracker() {
    let registry = Arc::new(ModelRegistry::new(std::collections::HashMap::new()));
    let tracker = make_tracker();
    tracker.use_registry(Arc::clone(&registry));

    // Nothing in the catalogue yet: the conservative fallback applies.
    let before = tracker.calculate_cost("late-model", 1_000_000, 0);
    assert!((before - 3.0).abs() < 1e-9, "before={before}");

    registry.register("late-model".to_string(), local_model(8192));
    assert_eq!(tracker.calculate_cost("late-model", 1_000_000, 0), 0.0);
}

/// An id the catalogue has never met costs nothing on a local-only install and
/// keeps the conservative rate anywhere else.
#[test]
fn an_unknown_model_is_free_only_where_every_model_is_local() {
    let local_only = Arc::new(ModelRegistry::new(std::collections::HashMap::new()));
    local_only.register("qwen3:27b".to_string(), local_model(262_144));
    let tracker = make_tracker();
    tracker.use_registry(local_only);
    assert_eq!(tracker.calculate_cost("never-seen", 1_000_000, 100_000), 0.0);

    // A catalogue with a cloud model in it keeps today's conservative fallback.
    let tracker = make_tracker(); // with_defaults(): Anthropic + OpenAI
    let cost = tracker.calculate_cost("never-seen", 1_000_000, 100_000);
    assert!((cost - 4.5).abs() < 0.01, "cost={cost}");
}

#[tokio::test]
async fn test_record_agent_usage() {
    let tracker = make_tracker();
    let record = CallRecord {
        agent_id: "agent1".to_string(),
        task_id: None,
        model: "claude-sonnet-4-5-20250929".to_string(),
        input_tokens: 100,
        output_tokens: 50,
        cost_usd: 0.001,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
    };
    tracker.record(&record).await;

    let usage = tracker.get_agent_usage("agent1").await.unwrap();
    assert_eq!(usage.total_requests, 1);
    assert_eq!(usage.total_input_tokens, 100);
    assert_eq!(usage.total_output_tokens, 50);
    assert!((usage.total_cost_usd - 0.001).abs() < 0.0001);
}

#[tokio::test]
async fn test_record_task_usage() {
    let tracker = make_tracker();
    let record = CallRecord {
        agent_id: "agent1".to_string(),
        task_id: Some("task1".to_string()),
        model: "gpt-4o".to_string(),
        input_tokens: 200,
        output_tokens: 100,
        cost_usd: 0.002,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
    };
    tracker.record(&record).await;

    let usage = tracker.get_task_usage("task1").await.unwrap();
    assert_eq!(usage.total_requests, 1);
    assert_eq!(usage.total_input_tokens, 200);
}

#[tokio::test]
async fn test_check_task_budget_within() {
    let tracker = make_tracker();
    let record = CallRecord {
        agent_id: "agent1".to_string(),
        task_id: Some("task1".to_string()),
        model: "gpt-4o".to_string(),
        input_tokens: 100,
        output_tokens: 50,
        cost_usd: 0.50,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
    };
    tracker.record(&record).await;
    assert!(tracker.check_task_budget("task1", 1.00).await);
}

#[tokio::test]
async fn test_check_task_budget_exceeded() {
    let tracker = make_tracker();
    let record = CallRecord {
        agent_id: "agent1".to_string(),
        task_id: Some("task1".to_string()),
        model: "gpt-4o".to_string(),
        input_tokens: 100,
        output_tokens: 50,
        cost_usd: 1.50,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
    };
    tracker.record(&record).await;
    assert!(!tracker.check_task_budget("task1", 1.00).await);
}

#[tokio::test]
async fn test_check_task_budget_no_usage() {
    let tracker = make_tracker();
    assert!(tracker.check_task_budget("unknown_task", 1.00).await);
}

#[tokio::test]
async fn test_multiple_records_accumulate() {
    let tracker = make_tracker();
    for i in 0..3 {
        let record = CallRecord {
            agent_id: "agent1".to_string(),
            task_id: Some("task1".to_string()),
            model: "gpt-4o".to_string(),
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.1 * (i + 1) as f64,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        };
        tracker.record(&record).await;
    }

    let usage = tracker.get_agent_usage("agent1").await.unwrap();
    assert_eq!(usage.total_requests, 3);
    assert_eq!(usage.total_input_tokens, 300);
    assert_eq!(usage.total_output_tokens, 150);
}

#[tokio::test]
async fn test_cache_hit_ratio_calculation() {
    let tracker = make_tracker();
    let record = CallRecord {
        agent_id: "agent1".to_string(),
        task_id: None,
        model: "claude-sonnet-4-5-20250929".to_string(),
        input_tokens: 1000,
        output_tokens: 200,
        cost_usd: 0.01,
        cache_creation_tokens: 100,
        cache_read_tokens: 800,
    };
    tracker.record(&record).await;

    let ratio = tracker.cache_hit_ratio().await;
    assert!((ratio - 0.8).abs() < 0.001, "ratio={}", ratio);
}

#[tokio::test]
async fn test_cache_hit_ratio_no_calls() {
    let tracker = make_tracker();
    let ratio = tracker.cache_hit_ratio().await;
    assert!((ratio - 0.0).abs() < 0.001);
}

#[tokio::test]
async fn test_cache_stats_aggregation() {
    let tracker = make_tracker();

    tracker
        .record(&CallRecord {
            agent_id: "agent1".to_string(),
            task_id: None,
            model: "claude-sonnet-4-5-20250929".to_string(),
            input_tokens: 1000,
            output_tokens: 200,
            cost_usd: 0.01,
            cache_creation_tokens: 100,
            cache_read_tokens: 800,
        })
        .await;

    tracker
        .record(&CallRecord {
            agent_id: "agent1".to_string(),
            task_id: None,
            model: "claude-sonnet-4-5-20250929".to_string(),
            input_tokens: 600,
            output_tokens: 100,
            cost_usd: 0.005,
            cache_creation_tokens: 0,
            cache_read_tokens: 500,
        })
        .await;

    let stats = tracker.cache_stats().await;
    assert_eq!(stats.total_cache_creation_tokens, 100);
    assert_eq!(stats.total_cache_read_tokens, 1300);
    assert_eq!(stats.total_input_tokens, 1600);
    assert!(
        (stats.hit_ratio - 0.8125).abs() < 0.001,
        "ratio={}",
        stats.hit_ratio
    );
}
