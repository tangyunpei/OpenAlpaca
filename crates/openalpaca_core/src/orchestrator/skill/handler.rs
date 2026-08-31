//! Skill invocation entry point with lifecycle event emission and telemetry.

use crate::events::SystemEvent;
use crate::memory::scope_context::MemoryScopeContext;
use crate::orchestrator::{ConversationContext, Orchestrator};
use crate::runner::LoopFinishReason;
use chrono::Utc;
use openalpaca_storage::repository::SkillExecutionRepository;
use openalpaca_storage::SkillExecutionEntry;
use uuid::Uuid;

/// Result of a skill invocation, carrying LLM metadata alongside the output content.
pub(crate) struct SkillInvocationResult {
    pub content: String,
    pub finish_reason: LoopFinishReason,
    pub rounds_used: usize,
    pub tool_calls_made: usize,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_usd: f64,
    pub model_used: Option<String>,
    pub repair_attempted: bool,
    pub repair_succeeded: bool,
    pub validation_failures: Vec<String>,
}

impl Orchestrator {
    /// Handle a skill invocation: load full SKILL.md, inject as context, run agentic loop.
    ///
    /// Mirrors `handle_simple_query()` with an extra `### SKILL CONTEXT ###` block.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::orchestrator) async fn handle_skill_invocation(
        &self,
        request_id: Uuid,
        source: &str,
        skill_name: &str,
        query: &str,
        lane_key: &str,
        ctx: &ConversationContext,
        owner_id: Option<&str>,
        scope_ctx: &MemoryScopeContext,
        route_score: Option<f64>,
        was_auto_selected: bool,
        stream_id: Option<&str>,
    ) -> Result<String, String> {
        let invocation_start = std::time::Instant::now();

        // Emit invocation started event
        self.bus.publish(SystemEvent::SkillInvocationStarted {
            request_id,
            skill_id: skill_name.to_string(),
            query_preview: query.chars().take(100).collect(),
            timestamp: Utc::now(),
        });

        let result = self
            .handle_skill_invocation_inner(
                request_id, source, skill_name, query, lane_key, ctx, owner_id, scope_ctx,
                stream_id,
            )
            .await;

        let duration_ms = invocation_start.elapsed().as_millis() as u64;

        // Emit SkillCompleted or SkillFailed based on result
        match &result {
            Ok(invocation_result) => {
                self.bus.publish(SystemEvent::SkillCompleted {
                    request_id,
                    skill_id: skill_name.to_string(),
                    duration_ms,
                    output_preview: invocation_result.content.chars().take(200).collect(),
                    timestamp: Utc::now(),
                });
            }
            Err(error) => {
                self.bus.publish(SystemEvent::SkillFailed {
                    request_id,
                    skill_id: skill_name.to_string(),
                    error: error.clone(),
                    timestamp: Utc::now(),
                });
            }
        }

        // Persist telemetry (Phase 3)
        if let Some(ref db) = self.db {
            let store_preview = self.daemon_config.load().telemetry.store_query_preview;
            let entry = SkillExecutionEntry {
                id: None,
                request_id: request_id.to_string(),
                skill_id: skill_name.to_string(),
                agent_id: "orchestrator".to_string(),
                status: match &result {
                    Ok(_) => "success".to_string(),
                    Err(_) => "error".to_string(),
                },
                finish_reason: result
                    .as_ref()
                    .ok()
                    .map(|r| finish_reason_to_string(&r.finish_reason).to_string()),
                error_message: result.as_ref().err().cloned(),
                validation_failures: result.as_ref().ok().and_then(|r| {
                    if r.validation_failures.is_empty() {
                        None
                    } else {
                        serde_json::to_string(&r.validation_failures).ok()
                    }
                }),
                duration_ms: duration_ms as i64,
                rounds_used: result.as_ref().ok().map(|r| r.rounds_used as i32),
                tool_calls_made: result.as_ref().ok().map(|r| r.tool_calls_made as i32),
                input_tokens: result.as_ref().ok().map(|r| r.input_tokens as i32).unwrap_or(0),
                output_tokens: result.as_ref().ok().map(|r| r.output_tokens as i32).unwrap_or(0),
                cost_usd: result.as_ref().ok().map(|r| r.cost_usd).unwrap_or(0.0),
                model_used: result.as_ref().ok().and_then(|r| r.model_used.clone()),
                query_preview: if store_preview {
                    Some(query.chars().take(200).collect())
                } else {
                    None
                },
                route_score,
                was_auto_selected,
                repair_attempted: result.as_ref().ok().map(|r| r.repair_attempted).unwrap_or(false),
                repair_succeeded: result.as_ref().ok().map(|r| r.repair_succeeded).unwrap_or(false),
                timestamp: None,
            };
            if let Err(e) = SkillExecutionRepository::new(db).record(&entry) {
                tracing::warn!("Failed to persist skill telemetry: {e}");
            }
        }

        result.map(|r| r.content)
    }
}

fn finish_reason_to_string(reason: &LoopFinishReason) -> &'static str {
    match reason {
        LoopFinishReason::Complete => "complete",
        LoopFinishReason::MaxRounds => "max_rounds",
        LoopFinishReason::CostExceeded => "cost_exceeded",
        LoopFinishReason::Truncated => "truncated",
        LoopFinishReason::Cancelled => "cancelled",
        LoopFinishReason::Error(_) => "error",
    }
}
