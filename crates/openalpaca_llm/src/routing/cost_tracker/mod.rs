//! Cost tracking: per-agent and per-task usage and budget enforcement.

use crate::routing::model_registry::ModelRegistry;
use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::Arc;
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
    pub cache_creation_tokens: u32,
    pub cache_read_tokens: u32,
}

/// Aggregated usage statistics for a single entity (agent or task).
#[derive(Debug, Clone, Default)]
pub struct UsageStats {
    pub total_requests: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub by_model: HashMap<String, ModelUsageStats>,
    pub total_cache_creation_tokens: u64,
    pub total_cache_read_tokens: u64,
}

/// Per-model usage statistics.
#[derive(Debug, Clone, Default)]
pub struct ModelUsageStats {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
}

/// Tracks costs across agents, tasks, and providers.
pub struct CostTracker {
    /// The catalogue prices are read from.
    ///
    /// Swappable because the router hands the tracker *its own* registry at
    /// construction: one source of truth — compiled defaults, `[models]` rows
    /// and discovered models — so a free local model is billed at 0 and a
    /// registry reload reaches billing without a second copy to keep in step
    /// (L8). Before that, the tracker owned a `with_defaults()` registry no
    /// config or discovery ever touched, and a local turn was billed at
    /// Sonnet rates against the $1 per-turn and $5 per-workflow caps.
    model_registry: ArcSwap<ModelRegistry>,
    agent_usage: RwLock<HashMap<String, UsageStats>>,
    task_usage: RwLock<HashMap<String, UsageStats>>,
    provider_usage: RwLock<HashMap<String, UsageStats>>,
}

impl CostTracker {
    pub fn new(model_registry: ModelRegistry) -> Self {
        Self {
            model_registry: ArcSwap::from_pointee(model_registry),
            agent_usage: RwLock::new(HashMap::new()),
            task_usage: RwLock::new(HashMap::new()),
            provider_usage: RwLock::new(HashMap::new()),
        }
    }

    /// Price from this registry from now on.
    ///
    /// Called by every `LlmRouter` constructor with the router's own registry,
    /// so the tracker and the routing decision always read the same catalogue.
    pub fn use_registry(&self, model_registry: Arc<ModelRegistry>) {
        self.model_registry.store(model_registry);
    }

    /// The (input, output) price per million tokens to bill this model at.
    fn pricing_for(&self, model: &str) -> (f64, f64) {
        let registry = self.model_registry.load();
        if let Some(pricing) = registry.get_pricing(model) {
            return (
                pricing.input_price_per_million,
                pricing.output_price_per_million,
            );
        }
        // An id the catalogue has never met. Where the whole catalogue is
        // local, no call can have cost money, and billing this one at cloud
        // rates would abort a free run on fictional spend.
        if registry.is_local_only() {
            return (0.0, 0.0);
        }
        // Otherwise stay conservative: $3/1M input, $15/1M output (Sonnet-like).
        (3.0, 15.0)
    }

    /// Calculate the cost for a given model and token counts.
    /// Falls back to default pricing if model is unknown.
    pub fn calculate_cost(&self, model: &str, input_tokens: u32, output_tokens: u32) -> f64 {
        let (input_price, output_price) = self.pricing_for(model);
        (input_tokens as f64 * input_price / 1_000_000.0)
            + (output_tokens as f64 * output_price / 1_000_000.0)
    }

    /// Calculate cost including Anthropic prompt-cache pricing.
    ///
    /// `input_tokens` is the folded total reported by the provider (cache tokens
    /// are a subset of it, per `Usage`). Non-cached input bills at the base input
    /// rate, cache-write tokens at 1.25x, and cache-read tokens at 0.1x — so a
    /// mostly-cached prompt is no longer charged up to ~10x its real cost.
    pub fn calculate_cost_with_cache(
        &self,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
        cache_creation_tokens: u32,
        cache_read_tokens: u32,
    ) -> f64 {
        let (input_price, output_price) = self.pricing_for(model);
        let cached = cache_creation_tokens as u64 + cache_read_tokens as u64;
        let non_cached_input = (input_tokens as u64).saturating_sub(cached);
        (non_cached_input as f64 * input_price / 1_000_000.0)
            + (cache_creation_tokens as f64 * input_price * 1.25 / 1_000_000.0)
            + (cache_read_tokens as f64 * input_price * 0.10 / 1_000_000.0)
            + (output_tokens as f64 * output_price / 1_000_000.0)
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
            stats.total_cache_creation_tokens += record.cache_creation_tokens as u64;
            stats.total_cache_read_tokens += record.cache_read_tokens as u64;

            let model_stats = stats.by_model.entry(record.model.clone()).or_default();
            model_stats.requests += 1;
            model_stats.input_tokens += record.input_tokens as u64;
            model_stats.output_tokens += record.output_tokens as u64;
            model_stats.cost_usd += record.cost_usd;
            model_stats.cache_creation_tokens += record.cache_creation_tokens as u64;
            model_stats.cache_read_tokens += record.cache_read_tokens as u64;
        }

        // Update task usage if task_id is present
        if let Some(ref task_id) = record.task_id {
            let mut usage = self.task_usage.write().await;
            let stats = usage.entry(task_id.clone()).or_default();
            stats.total_requests += 1;
            stats.total_input_tokens += record.input_tokens as u64;
            stats.total_output_tokens += record.output_tokens as u64;
            stats.total_cost_usd += record.cost_usd;
            stats.total_cache_creation_tokens += record.cache_creation_tokens as u64;
            stats.total_cache_read_tokens += record.cache_read_tokens as u64;

            let model_stats = stats.by_model.entry(record.model.clone()).or_default();
            model_stats.requests += 1;
            model_stats.input_tokens += record.input_tokens as u64;
            model_stats.output_tokens += record.output_tokens as u64;
            model_stats.cost_usd += record.cost_usd;
            model_stats.cache_creation_tokens += record.cache_creation_tokens as u64;
            model_stats.cache_read_tokens += record.cache_read_tokens as u64;
        }

        // Update provider usage (resolve model → provider)
        {
            let provider_name = self
                .model_registry
                .load()
                .resolve_provider_name(&record.model)
                .unwrap_or_else(|| "unknown".to_string());

            let mut usage = self.provider_usage.write().await;
            let stats = usage.entry(provider_name).or_default();
            stats.total_requests += 1;
            stats.total_input_tokens += record.input_tokens as u64;
            stats.total_output_tokens += record.output_tokens as u64;
            stats.total_cost_usd += record.cost_usd;
            stats.total_cache_creation_tokens += record.cache_creation_tokens as u64;
            stats.total_cache_read_tokens += record.cache_read_tokens as u64;

            let model_stats = stats.by_model.entry(record.model.clone()).or_default();
            model_stats.requests += 1;
            model_stats.input_tokens += record.input_tokens as u64;
            model_stats.output_tokens += record.output_tokens as u64;
            model_stats.cost_usd += record.cost_usd;
            model_stats.cache_creation_tokens += record.cache_creation_tokens as u64;
            model_stats.cache_read_tokens += record.cache_read_tokens as u64;
        }
    }

    /// Convenience method to record usage from a ChatResponse's Usage struct.
    /// Calculates cost automatically using the model registry.
    pub async fn record_usage(
        &self,
        agent_id: &str,
        task_id: Option<&str>,
        model: &str,
        usage: &crate::types::Usage,
    ) {
        let cost = self.calculate_cost_with_cache(
            model,
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_creation_input_tokens,
            usage.cache_read_input_tokens,
        );
        let record = CallRecord {
            agent_id: agent_id.to_string(),
            task_id: task_id.map(|s| s.to_string()),
            model: model.to_string(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cost_usd: cost,
            cache_creation_tokens: usage.cache_creation_input_tokens,
            cache_read_tokens: usage.cache_read_input_tokens,
        };
        self.record(&record).await;
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

    /// Produce a point-in-time snapshot of all cost tracking data.
    ///
    /// Per-call persistence is already handled by `record_and_log()` in the
    /// storage layer, so periodic flushing is unnecessary. The daemon calls
    /// this on graceful shutdown as defense-in-depth, writing the cumulative
    /// totals via `replace_daily_usage()` (overwrite, not additive).
    pub async fn snapshot_for_flush(&self) -> CostSnapshot {
        CostSnapshot {
            agent_usage: self.agent_usage.read().await.clone(),
            task_usage: self.task_usage.read().await.clone(),
            provider_usage: self.provider_usage.read().await.clone(),
        }
    }

    /// Load persisted cost data from the database on daemon startup.
    ///
    /// Called by `restore_cost_tracker()` in the daemon to seed the in-memory
    /// tracker from today's `llm_usage_daily` rows, ensuring budget enforcement
    /// is accurate across restarts.
    pub async fn load_snapshot(&self, snapshot: CostSnapshot) {
        *self.agent_usage.write().await = snapshot.agent_usage;
        *self.task_usage.write().await = snapshot.task_usage;
        *self.provider_usage.write().await = snapshot.provider_usage;
    }

    /// Cache hit ratio across all agents: cache_read_tokens / total_input_tokens.
    /// Returns 0.0 if no input tokens have been recorded.
    pub async fn cache_hit_ratio(&self) -> f64 {
        let usage = self.agent_usage.read().await;
        let total_input: u64 = usage.values().map(|s| s.total_input_tokens).sum();
        let total_cache_read: u64 = usage.values().map(|s| s.total_cache_read_tokens).sum();
        if total_input == 0 {
            0.0
        } else {
            total_cache_read as f64 / total_input as f64
        }
    }

    /// Aggregated cache statistics across all agents.
    pub async fn cache_stats(&self) -> CacheStats {
        let usage = self.agent_usage.read().await;
        let total_input: u64 = usage.values().map(|s| s.total_input_tokens).sum();
        let total_cache_read: u64 = usage.values().map(|s| s.total_cache_read_tokens).sum();
        let total_cache_creation: u64 =
            usage.values().map(|s| s.total_cache_creation_tokens).sum();
        CacheStats {
            total_cache_read_tokens: total_cache_read,
            total_cache_creation_tokens: total_cache_creation,
            total_input_tokens: total_input,
            hit_ratio: if total_input == 0 {
                0.0
            } else {
                total_cache_read as f64 / total_input as f64
            },
        }
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

/// Aggregated cache performance statistics.
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub total_cache_read_tokens: u64,
    pub total_cache_creation_tokens: u64,
    pub total_input_tokens: u64,
    pub hit_ratio: f64,
}

#[cfg(test)]
mod tests;
