//! Shared helpers for recording LLM usage and agent history after agentic loop execution.

use crate::bus::EventBus;
use crate::events::SystemEvent;
use crate::runner::{LoopFinishReason, LoopResult};
use chrono::Utc;
use openalpaca_llm::LlmRouter;
use openalpaca_storage::Database;
use uuid::Uuid;

/// Record LLM usage metrics (cost, tokens, latency) to the database and emit an event.
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
    let default_model = router.default_model();
    let actual_model = loop_result
        .model_used
        .as_deref()
        .or(model_override)
        .unwrap_or(&default_model);
    let resolved_provider = router
        .model_registry()
        .resolve_provider(actual_model)
        .map(|p| p.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let call_cost = router.cost_tracker.calculate_cost(
        actual_model,
        loop_result.total_input_tokens,
        loop_result.total_output_tokens,
    );

    let call_status = match &loop_result.finish_reason {
        LoopFinishReason::Complete | LoopFinishReason::MaxRounds | LoopFinishReason::Truncated => "success",
        LoopFinishReason::CostExceeded => "cost_exceeded",
        LoopFinishReason::Cancelled => "cancelled",
        LoopFinishReason::Error(_) => "error",
    };
    let call_error = match &loop_result.finish_reason {
        LoopFinishReason::Error(msg) => Some(msg.as_str()),
        _ => None,
    };

    if let Some(db) = db {
        let usage_repo = openalpaca_storage::repository::LlmUsageRepository::new(db);
        if let Err(e) = usage_repo.record_and_log(
            agent_id,
            Some(task_id),
            &resolved_provider,
            actual_model,
            loop_result.total_input_tokens as i32,
            loop_result.total_output_tokens as i32,
            call_cost,
            latency_ms,
            call_status,
            call_error,
        ) {
            tracing::warn!("Failed to persist LLM usage: {e}");
        }
    }

    bus.publish(SystemEvent::LlmCallCompleted {
        agent_id: agent_id.to_string(),
        model: actual_model.to_string(),
        input_tokens: loop_result.total_input_tokens,
        output_tokens: loop_result.total_output_tokens,
        cost_usd: call_cost,
        timestamp: Utc::now(),
    });
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
