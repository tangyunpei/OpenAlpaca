//! Shared helpers for recording LLM usage and agent history after agentic loop execution.

use crate::bus::EventBus;
use crate::events::SystemEvent;
use crate::runner::{LoopFinishReason, LoopResult};
use chrono::Utc;
use openalpaca_llm::LlmRouter;
use openalpaca_storage::Database;
use uuid::Uuid;

/// One loop's usage, resolved once for persistence, events, and the turn result.
pub(crate) struct LoopUsage<'a> {
    pub model: String,
    provider: String,
    result: &'a LoopResult,
    pub cost: f64,
}

impl<'a> LoopUsage<'a> {
    pub fn new(router: &LlmRouter, result: &'a LoopResult, model_override: Option<&str>) -> Self {
        let model = result
            .model_used
            .clone()
            .or_else(|| model_override.map(str::to_owned))
            .unwrap_or_else(|| router.default_model());
        let provider = router
            .model_registry()
            .resolve_provider(&model)
            .map(|p| p.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        Self {
            model,
            provider,
            result,
            cost: result.estimated_cost,
        }
    }

    pub fn record(
        &self,
        agent_id: &str,
        task_id: Option<&str>,
        latency_ms: i64,
        db: Option<&Database>,
        bus: &EventBus,
    ) {
        let (status, error) = match &self.result.finish_reason {
            LoopFinishReason::Complete
            | LoopFinishReason::MaxRounds
            | LoopFinishReason::Truncated => ("success", None),
            LoopFinishReason::CostExceeded => ("cost_exceeded", None),
            LoopFinishReason::Cancelled => ("cancelled", None),
            LoopFinishReason::Error(msg) => ("error", Some(msg.as_str())),
        };
        if let Some(db) = db {
            let repo = openalpaca_storage::repository::LlmUsageRepository::new(db);
            if let Err(e) = repo.record_and_log(
                agent_id,
                task_id,
                &self.provider,
                &self.model,
                self.result.total_input_tokens as i32,
                self.result.total_output_tokens as i32,
                self.cost,
                latency_ms,
                status,
                error,
            ) {
                tracing::warn!("Failed to persist LLM usage: {e}");
            }
        }
        bus.publish(SystemEvent::LlmCallCompleted {
            agent_id: agent_id.to_string(),
            model: self.model.clone(),
            input_tokens: self.result.total_input_tokens,
            output_tokens: self.result.total_output_tokens,
            cost_usd: self.cost,
            task_id: task_id.map(str::to_owned),
            timestamp: Utc::now(),
        });
    }
}

/// Workflows keep the loop's accumulated cost, including model changes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_llm_usage(
    router: &LlmRouter,
    loop_result: &LoopResult,
    model_override: Option<&str>,
    agent_id: &str,
    task_id: &str,
    latency_ms: i64,
    db: Option<&Database>,
    bus: &EventBus,
) {
    LoopUsage::new(router, loop_result, model_override).record(
        agent_id,
        Some(task_id),
        latency_ms,
        db,
        bus,
    );
}

impl super::super::Orchestrator {
    pub(in crate::orchestrator) fn record_turn_usage(
        &self,
        router: &LlmRouter,
        result: &LoopResult,
        latency_ms: i64,
        turn: &mut crate::gateway::HandleResult,
    ) -> f64 {
        let mut usage = LoopUsage::new(router, result, self.loop_config.model.as_deref());
        // Preserve the interactive paths' existing policy: price the aggregate
        // tokens using the final model, rather than changing billing in a refactor.
        usage.cost = router.cost_tracker.calculate_cost(
            &usage.model,
            result.total_input_tokens,
            result.total_output_tokens,
        );
        usage.record(
            crate::orchestrator::MAIN_LOOP_AGENT_ID,
            None,
            latency_ms,
            self.db.as_ref(),
            &self.bus,
        );
        turn.model = Some(usage.model);
        turn.tokens_in = Some(result.total_input_tokens);
        turn.tokens_out = Some(result.total_output_tokens);
        usage.cost
    }
}

/// Record per-agent task history and increment success/failure counters.
pub(crate) fn record_agent_history(
    db: &Database,
    template_id: &str,
    task_id: &str,
    role: &str,
    success: bool,
    runtime_secs: i64,
) {
    let subagent_repo = openalpaca_storage::SubAgentRepository::new(db);
    let history_entry = openalpaca_storage::AgentTaskHistory {
        id: Uuid::new_v4().to_string(),
        agent_id: template_id.to_string(),
        task_id: task_id.to_string(),
        role: role.to_string(),
        status: if success { "completed" } else { "failed" }.to_string(),
        runtime_seconds: Some(runtime_secs),
        completed_at: Utc::now(),
    };
    if let Err(e) = subagent_repo.add_history(&history_entry) {
        tracing::warn!("Failed to record agent task history: {e}");
    }
    if success {
        let _ = subagent_repo.increment_completed(template_id, runtime_secs);
    } else {
        let _ = subagent_repo.increment_failed(template_id);
    }
}
