//! Shared helpers for recording LLM usage and agent history after agentic loop execution.

use crate::bus::EventBus;
use crate::events::SystemEvent;
use crate::runner::{LoopFinishReason, LoopResult};
use chrono::Utc;
use openalpaca_llm::LlmRouter;
use openalpaca_storage::Database;
use uuid::Uuid;

/// Record LLM usage metrics (cost, tokens, latency) to the database and emit an event:
/// a workflow's lead row and each subagent's, priced from
/// `LoopResult::requests_cost` (UPR-1).
#[expect(
    clippy::too_many_arguments,
    reason = "the call's figures and the two places they are written"
)]
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
    record_llm_usage_with_status(
        router,
        loop_result,
        model_override,
        agent_id,
        task_id,
        latency_ms,
        db,
        bus,
        None,
    );
}

/// [`record_llm_usage`], its row's status `status_override` when one is
/// given, else the word the loop's finish reason reads (SA-4, Y-15 (3)):
/// a typed child's failure — whatever its loop's finish — is recorded
/// `result_rejected`, so a reader can count typed failures as one word.
#[expect(
    clippy::too_many_arguments,
    reason = "the call's figures, the two places they are written and the status"
)]
pub(crate) fn record_llm_usage_with_status(
    router: &LlmRouter,
    loop_result: &LoopResult,
    model_override: Option<&str>,
    agent_id: &str,
    task_id: &str,
    latency_ms: i64,
    db: Option<&Database>,
    bus: &EventBus,
    status_override: Option<&'static str>,
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
    // UPR-1 (the advisor's (b), which the owner took on 2026-11-17): what
    // the loop's own requests cost, each as it was counted, the request
    // that ended the loop included — the figure a chat turn's row carries
    // (USAGE-PER-REQUEST). Not `estimated_cost`, the agent's tracker total
    // read at the top of the last round, which leaves that request out and,
    // for a singleton's reused instance id, holds its earlier runs.
    let call_cost = loop_result.requests_cost;

    let call_status = status_override.unwrap_or(match &loop_result.finish_reason {
        LoopFinishReason::Complete
        | LoopFinishReason::MaxRounds
        | LoopFinishReason::Truncated
        | LoopFinishReason::ResultSubmitted => "success",
        LoopFinishReason::CostExceeded => "cost_exceeded",
        LoopFinishReason::ResultRejected => "result_rejected",
        LoopFinishReason::Cancelled => "cancelled",
        LoopFinishReason::Error(_) => "error",
    });
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
        // The one LLM path that runs inside a workflow (GAP-10).
        task_id: Some(task_id.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{RECORDED_MODEL, RecordingProvider, router_recording};

    /// **UPR-1** (the advisor's (b), which the owner took on 2026-11-17).
    /// The lead's row and each subagent's are written from what the loop's
    /// own requests cost, each as it was counted (`requests_cost`), the
    /// figure a chat turn's row has carried since USAGE-PER-REQUEST — never
    /// from `estimated_cost`, the agent's tracker total read at the top of
    /// the last round, which leaves out the request that ended the loop
    /// and, for a reused instance id, holds its earlier runs. On the parent
    /// the row and the event carried the stale $0.25.
    #[test]
    fn a_workflow_loops_row_carries_what_its_requests_cost() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let router = router_recording(RecordingProvider::new("unused"));
        let result = LoopResult {
            final_content: "done".to_string(),
            rounds_used: 1,
            total_input_tokens: 600,
            total_output_tokens: 100,
            tool_calls_made: 0,
            finish_reason: LoopFinishReason::Complete,
            model_used: Some(RECORDED_MODEL.to_string()),
            elapsed: std::time::Duration::from_secs(1),
            estimated_cost: 0.25,
            cap_spend: 0.0,
            requests_cost: 0.0011,
            last_tool_error: None,
            working_state: None,
            spent_fallback_rung: false,
        };

        record_llm_usage(
            &router,
            &result,
            None,
            "research_agent",
            "task-1",
            1_000,
            Some(&db),
            &bus,
        );

        let rows = openalpaca_storage::repository::LlmUsageRepository::new(&db)
            .get_agent_usage("research_agent", 10)
            .unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(
            (rows[0].cost_usd - 0.0011).abs() < 1e-12,
            "the row: ${}",
            rows[0].cost_usd
        );
        match rx.try_recv() {
            Ok(SystemEvent::LlmCallCompleted { cost_usd, .. }) => {
                assert!((cost_usd - 0.0011).abs() < 1e-12, "the event: ${cost_usd}");
            }
            other => panic!("no LlmCallCompleted: {other:?}"),
        }
    }
}
