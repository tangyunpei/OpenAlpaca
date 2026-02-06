//! Cost tracking: per-agent and per-task usage and budget enforcement.

use crate::model_registry::ModelRegistry;
use std::collections::HashMap;
use tokio::sync::RwLock;

/// A record of a single LLM API call.
#[derive(Debug, Clone)]
pub struct CallRecord {
    pub agent_id: String,
    pub task_id: Option<String>,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_usd: f64,
}

/// Aggregated usage statistics for a single entity (agent or task).
#[derive(Debug, Clone, Default)]
pub struct UsageStats {
    pub total_requests: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub by_model: HashMap<String, ModelUsageStats>,
}

/// Per-model usage statistics.
#[derive(Debug, Clone, Default)]
pub struct ModelUsageStats {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

/// Tracks costs across agents and tasks.
pub struct CostTracker {
    model_registry: ModelRegistry,
    agent_usage: RwLock<HashMap<String, UsageStats>>,
    task_usage: RwLock<HashMap<String, UsageStats>>,
}

impl CostTracker {
    pub fn new(model_registry: ModelRegistry) -> Self {
        Self {
            model_registry,
            agent_usage: RwLock::new(HashMap::new()),
            task_usage: RwLock::new(HashMap::new()),
        }
    }

    /// Calculate the cost for a given model and token counts.
    /// Falls back to default pricing if model is unknown.
    pub fn calculate_cost(&self, model: &str, input_tokens: u32, output_tokens: u32) -> f64 {
        match self.model_registry.get_pricing(model) {
            Some(pricing) => {
                (input_tokens as f64 * pricing.input_price_per_million / 1_000_000.0)
                    + (output_tokens as f64 * pricing.output_price_per_million / 1_000_000.0)
            }
            None => {
                // Fallback: $3/1M input, $15/1M output (Sonnet-like)
                (input_tokens as f64 * 3.0 / 1_000_000.0)
                    + (output_tokens as f64 * 15.0 / 1_000_000.0)
            }
        }
    }

    /// Record a completed API call's usage.
    pub async fn record(&self, record: &CallRecord) {
        // Update agent usage
        {
            let mut usage = self.agent_usage.write().await;
            let stats = usage.entry(record.agent_id.clone()).or_default();
            stats.total_requests += 1;
            stats.total_input_tokens += record.input_tokens as u64;
            stats.total_output_tokens += record.output_tokens as u64;
            stats.total_cost_usd += record.cost_usd;

            let model_stats = stats.by_model.entry(record.model.clone()).or_default();
            model_stats.requests += 1;
            model_stats.input_tokens += record.input_tokens as u64;
            model_stats.output_tokens += record.output_tokens as u64;
            model_stats.cost_usd += record.cost_usd;
        }

        // Update task usage if task_id is present
        if let Some(ref task_id) = record.task_id {
            let mut usage = self.task_usage.write().await;
            let stats = usage.entry(task_id.clone()).or_default();
            stats.total_requests += 1;
            stats.total_input_tokens += record.input_tokens as u64;
            stats.total_output_tokens += record.output_tokens as u64;
            stats.total_cost_usd += record.cost_usd;

            let model_stats = stats.by_model.entry(record.model.clone()).or_default();
            model_stats.requests += 1;
            model_stats.input_tokens += record.input_tokens as u64;
            model_stats.output_tokens += record.output_tokens as u64;
            model_stats.cost_usd += record.cost_usd;
        }
    }

    /// Check if a task is still within budget.
    pub async fn check_task_budget(&self, task_id: &str, max_cost: f64) -> bool {
        let usage = self.task_usage.read().await;
        match usage.get(task_id) {
            Some(stats) => stats.total_cost_usd < max_cost,
            None => true, // No usage yet
        }
    }

    /// Get usage stats for an agent.
    pub async fn get_agent_usage(&self, agent_id: &str) -> Option<UsageStats> {
        let usage = self.agent_usage.read().await;
        usage.get(agent_id).cloned()
    }

    /// Get usage stats for a task.
    pub async fn get_task_usage(&self, task_id: &str) -> Option<UsageStats> {
        let usage = self.task_usage.read().await;
        usage.get(task_id).cloned()
    }

    /// Get the total cost across all agents.
    pub async fn total_cost(&self) -> f64 {
        self.agent_usage
            .read()
            .await
            .values()
            .map(|s| s.total_cost_usd)
            .sum()
    }
}

#[cfg(test)]
mod tests {
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
        assert!((cost - expected).abs() < 0.01, "cost={}, expected={}", cost, expected);
    }

    #[test]
    fn test_calculate_cost_unknown_model_fallback() {
        let tracker = make_tracker();
        let cost = tracker.calculate_cost("unknown-model", 1_000_000, 100_000);
        let expected = 3.0 + 1.5; // fallback matches sonnet pricing
        assert!((cost - expected).abs() < 0.01);
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
            };
            tracker.record(&record).await;
        }

        let usage = tracker.get_agent_usage("agent1").await.unwrap();
        assert_eq!(usage.total_requests, 3);
        assert_eq!(usage.total_input_tokens, 300);
        assert_eq!(usage.total_output_tokens, 150);
    }
}
