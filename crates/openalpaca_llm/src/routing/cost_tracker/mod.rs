//! Cost tracking: per-agent and per-task usage and budget enforcement.

use crate::routing::model_registry::ModelRegistry;
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

/// Tracks costs across agents, tasks, and providers.
pub struct CostTracker {
    model_registry: ModelRegistry,
    agent_usage: RwLock<HashMap<String, UsageStats>>,
    task_usage: RwLock<HashMap<String, UsageStats>>,
    provider_usage: RwLock<HashMap<String, UsageStats>>,
}

impl CostTracker {
    pub fn new(model_registry: ModelRegistry) -> Self {
        Self {
            model_registry,
            agent_usage: RwLock::new(HashMap::new()),
            task_usage: RwLock::new(HashMap::new()),
            provider_usage: RwLock::new(HashMap::new()),
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

        // Update provider usage (resolve model → provider)
        {
            let provider_name = self
                .model_registry
                .resolve_provider_name(&record.model)
                .unwrap_or_else(|| "unknown".to_string());

            let mut usage = self.provider_usage.write().await;
            let stats = usage.entry(provider_name).or_default();
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

    /// Get usage stats for a provider.
    pub async fn get_provider_usage(&self, provider: &str) -> Option<UsageStats> {
        let usage = self.provider_usage.read().await;
        usage.get(provider).cloned()
    }

    /// Get usage stats for all providers.
    pub async fn all_provider_usage(&self) -> HashMap<String, UsageStats> {
        self.provider_usage.read().await.clone()
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

    /// Flush in-memory cost data to the database for persistence across restarts.
    ///
    /// TODO: The daemon should call this periodically (e.g., every 60s) and on
    /// graceful shutdown to persist accumulated cost data to the `llm_usage_daily`
    /// table. This method is a stub because `openalpaca_llm` does not depend on
    /// `openalpaca_storage` — the actual DB writes should be performed by the
    /// daemon layer (e.g., in `openalpacad`) which has access to both crates.
    ///
    /// Suggested integration pattern:
    /// 1. Daemon reads agent/task/provider usage snapshots via the existing
    ///    `get_agent_usage()` / `get_task_usage()` / `all_provider_usage()` methods.
    /// 2. Daemon writes rows to `llm_usage_daily` via `openalpaca_storage`.
    /// 3. After successful flush, daemon calls `reset_flushed()` (not yet implemented)
    ///    or tracks high-water marks to avoid double-counting.
    pub async fn snapshot_for_flush(&self) -> CostSnapshot {
        CostSnapshot {
            agent_usage: self.agent_usage.read().await.clone(),
            task_usage: self.task_usage.read().await.clone(),
            provider_usage: self.provider_usage.read().await.clone(),
        }
    }

    /// Load persisted cost data from the database on daemon startup.
    ///
    /// TODO: The daemon should call this at startup to restore cost data from the
    /// `llm_usage_daily` table so that budget enforcement is accurate across
    /// restarts. Similar to `snapshot_for_flush`, the actual DB reads should be
    /// performed by the daemon layer which has access to `openalpaca_storage`.
    ///
    /// Suggested integration pattern:
    /// 1. Daemon reads today's rows from `llm_usage_daily` via `openalpaca_storage`.
    /// 2. Daemon calls this method with the loaded data to seed the in-memory tracker.
    pub async fn load_snapshot(&self, snapshot: CostSnapshot) {
        *self.agent_usage.write().await = snapshot.agent_usage;
        *self.task_usage.write().await = snapshot.task_usage;
        *self.provider_usage.write().await = snapshot.provider_usage;
    }
}

/// A point-in-time snapshot of all cost tracking data, suitable for
/// serialization and persistence to the database.
#[derive(Debug, Clone, Default)]
pub struct CostSnapshot {
    pub agent_usage: HashMap<String, UsageStats>,
    pub task_usage: HashMap<String, UsageStats>,
    pub provider_usage: HashMap<String, UsageStats>,
}

#[cfg(test)]
mod tests;
