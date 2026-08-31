//! Task dispatcher: creates tasks, assigns agents, starts task lanes.

pub(crate) mod decision;
mod lead_agent;
pub(crate) mod memory;
pub(crate) mod outcome;
#[cfg(test)]
mod tests;
pub(crate) mod usage;

use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::daemon_config::DaemonConfig;
use crate::events::SystemEvent;
use crate::lane::LaneManager;
use crate::orchestrator::ConnectorStatusProvider;
use crate::prompt_ctx::ContextManager;
use crate::security::gate::SecurityGate;
use arc_swap::ArcSwap;
use chrono::Utc;
use openalpaca_llm::LlmRouter;
use openalpaca_storage::Database;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::Instant;

use crate::tools::ToolRegistry;

use openalpaca_storage::repository::dispatch_decision::{
    DispatchDecisionRecord, DispatchDecisionRepository,
};

// Re-export for use by child modules (lead_agent)
pub(super) use memory::spawn_task_memory_extraction;
use outcome::{finalize_task_with_outcome, persist_conversation, update_state_with_retry};
#[cfg(test)]
use outcome::{build_task_outcome, finalize_task};

/// Result of a successful dispatch: the created task's identity plus the
/// human-readable ack for the chat reply. Callers that record analytics
/// must use `task_id` — never the ack prose.
#[derive(Debug, Clone)]
pub struct DispatchOutcome {
    pub task_id: String,
    pub title: String,
    pub ack: String,
}

/// Dispatches complex tasks by matching skills to agents and creating task lanes.
pub struct TaskDispatcher {
    shared_context: Arc<SharedContext>,
    lane_manager: Arc<LaneManager>,
    bus: EventBus,
    llm_router: Option<Arc<LlmRouter>>,
    _security_gate: Arc<SecurityGate>,
    tool_registry: Arc<ToolRegistry>,
    db: Option<Database>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    connector_status: Arc<RwLock<Option<Arc<dyn ConnectorStatusProvider>>>>,
    /// Cached connector guidance with 10s TTL (Opt-8b).
    cached_connector_guidance: std::sync::Mutex<(String, Instant)>,
    /// Optional confirmation broker for interactive tool approval in agent pipelines.
    pub(crate) confirmation_broker: Arc<RwLock<Option<Arc<crate::security::confirmation::ConfirmationBroker>>>>,
    /// Optional follow-up runner for autostarting queued follow-ups after a
    /// workflow finalizes (Routing V2; set post-construction).
    followup_runner: Arc<RwLock<Option<Arc<dyn crate::orchestrator::FollowupRunner>>>>,
    /// Context manager for distilling parent context into sub-agent packages.
    pub(crate) context_manager: Arc<ContextManager>,
    /// Layered compose engine. Shared with the owning `Orchestrator` so
    /// prompt caches are global across conversation + task paths (Phase 4).
    pub(crate) compose_engine: Arc<crate::compose::ComposeEngine>,
}

impl TaskDispatcher {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        shared_context: Arc<SharedContext>,
        lane_manager: Arc<LaneManager>,
        bus: EventBus,
        llm_router: Option<Arc<LlmRouter>>,
        security_gate: Arc<SecurityGate>,
        tool_registry: Arc<ToolRegistry>,
        db: Option<Database>,
        embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
        connector_status: Arc<RwLock<Option<Arc<dyn ConnectorStatusProvider>>>>,
        context_manager: Arc<ContextManager>,
        compose_engine: Arc<crate::compose::ComposeEngine>,
    ) -> Self {
        Self {
            shared_context,
            lane_manager,
            bus,
            llm_router,
            _security_gate: security_gate,
            tool_registry,
            db,
            embedder,
            daemon_config,
            connector_status,
            cached_connector_guidance: std::sync::Mutex::new((String::new(), Instant::now())),
            confirmation_broker: Arc::new(RwLock::new(None)),
            followup_runner: Arc::new(RwLock::new(None)),
            context_manager,
            compose_engine,
        }
    }

    /// Set the follow-up runner (post-construction, same pattern as the
    /// confirmation broker injection).
    pub fn set_followup_runner(&self, runner: Arc<dyn crate::orchestrator::FollowupRunner>) {
        if let Ok(mut guard) = self.followup_runner.write() {
            *guard = Some(runner);
        }
    }

    /// Snapshot connector statuses for prompt injection (10s TTL cache).
    fn connector_guidance_block(&self) -> String {
        // Fast path: return cached value if fresh (< 10s old)
        if let Ok(cache) = self.cached_connector_guidance.lock()
            && cache.1.elapsed() < std::time::Duration::from_secs(10)
            && !cache.0.is_empty()
        {
            return cache.0.clone();
        }

        // Slow path: compute fresh
        let result = if let Ok(guard) = self.connector_status.read()
            && let Some(ref provider) = *guard
        {
            let statuses = provider.list_status();
            crate::middleware::prompt::format_connector_guidance(&statuses, None)
        } else {
            String::new()
        };

        if let Ok(mut cache) = self.cached_connector_guidance.lock() {
            *cache = (result.clone(), Instant::now());
        }
        result
    }

    /// Get the LLM router or fail the task with a helpful error.
    fn require_router(&self, task_id: &str) -> Option<Arc<LlmRouter>> {
        match &self.llm_router {
            Some(r) => Some(r.clone()),
            None => {
                tracing::error!(
                    "No LLM router configured — cannot execute task '{}'",
                    task_id
                );
                self.shared_context
                    .task_registry
                    .update_status(task_id, crate::context::TaskEntryStatus::Failed);
                if let Some(ref db) = self.db {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    if let Err(e) =
                        repo.update_status(task_id, openalpaca_storage::TaskStatus::Failed)
                    {
                        tracing::warn!(
                            "require_router: failed to update DB status for task '{}': {e}",
                            task_id
                        );
                    }
                }
                self.bus.publish(crate::events::SystemEvent::TaskFailed {
                    task_id: task_id.to_string(),
                    error: "No LLM router configured".to_string(),
                    outcome_kind: None,
                    timestamp: chrono::Utc::now(),
                });
                None
            }
        }
    }

    // ── Decision recording ─────────────────────────────────────────────

    /// Record the tool-path dispatch decision (Routing V2 Phase 3).
    ///
    /// UNCONDITIONAL — the model's `start_workflow` call IS the routing
    /// decision, so every dispatch is recorded with the real task id
    /// (no backfill step: the task already exists when this runs).
    pub(crate) fn record_tool_dispatch_decision(&self, request_id: &str, task_id: &str) {
        let mode = decision::DispatchMode::LeadAgent.to_string();
        let reason = decision::DecisionReason::ModelToolCall.to_string();

        tracing::info!(
            mode = %mode,
            reason = %reason,
            task_id = %task_id,
            "DispatchDecision (tool path)"
        );

        self.bus.publish(SystemEvent::DispatchDecision {
            request_id: request_id.to_string(),
            task_id: Some(task_id.to_string()),
            mode: mode.clone(),
            reason: reason.clone(),
            agent_count: 0,
            dag_node_count: None,
            predictability_score: None,
            error_message: None,
            timestamp: Utc::now(),
        });

        if let Some(ref db) = self.db {
            let repo = DispatchDecisionRepository::new(db);
            if let Err(e) = repo.record(&DispatchDecisionRecord {
                id: None,
                request_id: request_id.to_string(),
                task_id: Some(task_id.to_string()),
                mode,
                reason,
                agent_count: 0,
                dag_node_count: None,
                predictability_score: None,
                planner_requested_mode: None,
                error_message: None,
                timestamp: None,
            }) {
                tracing::warn!("Failed to persist tool-path dispatch decision: {e}");
            }
        }
    }
}

/// Generate a concise task title from a description by stripping filler prefixes
/// and truncating to a reasonable length.
/// `pub(crate)` so `StartWorkflowTool` can default a missing title (Routing V2).
pub(crate) fn generate_title(description: &str) -> String {
    let lower = description.to_lowercase();
    // Strip filler prefixes
    let cleaned = lower
        .trim_start_matches("can you ")
        .trim_start_matches("could you ")
        .trim_start_matches("please ")
        .trim_start_matches("help me ")
        .trim_start_matches("i need to ")
        .trim_start_matches("i want to ");
    // Capitalize first letter
    let mut chars = cleaned.chars();
    let title: String = match chars.next() {
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
        None => description.to_string(),
    };
    // Take first 8 words or 50 chars
    let words: Vec<&str> = title.split_whitespace().take(8).collect();
    let result = words.join(" ");
    if result.chars().count() > 50 {
        // Truncate on a char boundary, not a byte index — a CJK description
        // (3 bytes/char, no spaces) would otherwise panic on `&result[..47]`.
        let truncated: String = result.chars().take(47).collect();
        format!("{}...", truncated)
    } else if words.len() == 8 && title.split_whitespace().count() > 8 {
        format!("{}...", result)
    } else {
        result
    }
}

/// Format a task result for display in the chat conversation.
pub(super) fn format_task_result(title: &str, summary: &str, is_success: bool) -> String {
    if is_success {
        format!("**Task completed: {}**\n\n{}", title, summary)
    } else {
        format!("**Task failed: {}**\n\n{}", title, summary)
    }
}
