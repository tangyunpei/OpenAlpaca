//! Task dispatcher: creates tasks, assigns agents, starts task lanes.

mod core;
mod dag;
pub(crate) mod decision;
mod lead_agent;
pub(crate) mod memory;
pub(crate) mod outcome;
mod pipeline;
mod pipeline_step;
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
use uuid::Uuid;

use crate::tools::ToolRegistry;

use super::skill_matcher::{SkillMatch, SkillMatcher};
use super::task_planner::TaskPlan;
use openalpaca_storage::repository::dispatch_decision::{
    DispatchDecisionRecord, DispatchDecisionRepository,
};

// Re-export for use by child modules (pipeline, dag, lead_agent)
pub(super) use memory::{retrieve_memory_block, spawn_task_memory_extraction};
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
    skill_matcher: SkillMatcher,
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
            skill_matcher: SkillMatcher,
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

    // ── Shared decision recording helpers ──────────────────────────────

    /// Record a dispatch decision (event + DB). Returns row ID for task_id backfill.
    fn record_decision(&self, request_id: &str, dd: &decision::DispatchDecision) -> Option<i64> {
        let dcfg = self.daemon_config.load();
        if !dcfg.execution.planner.dispatch_analysis_enabled {
            return None;
        }

        tracing::info!(
            mode = %dd.mode,
            reason = %dd.reason,
            agent_count = dd.agent_count,
            dag_node_count = ?dd.dag_node_count,
            predictability_score = ?dd.predictability_score,
            "DispatchDecision analysis"
        );

        self.bus.publish(SystemEvent::DispatchDecision {
            request_id: request_id.to_string(),
            task_id: None,
            mode: dd.mode.to_string(),
            reason: dd.reason.to_string(),
            agent_count: dd.agent_count,
            dag_node_count: dd.dag_node_count,
            predictability_score: dd.predictability_score,
            error_message: dd.error_message.clone(),
            timestamp: Utc::now(),
        });

        if let Some(ref db) = self.db {
            let repo = DispatchDecisionRepository::new(db);
            match repo.record(&DispatchDecisionRecord {
                id: None,
                request_id: request_id.to_string(),
                task_id: None,
                mode: dd.mode.to_string(),
                reason: dd.reason.to_string(),
                agent_count: dd.agent_count,
                dag_node_count: dd.dag_node_count,
                predictability_score: dd.predictability_score,
                planner_requested_mode: dd.planner_requested_mode.clone(),
                error_message: dd.error_message.clone(),
                timestamp: None,
            }) {
                Ok(id) => return Some(id),
                Err(e) => tracing::warn!("Failed to persist dispatch decision: {e}"),
            }
        }
        None
    }

    /// Backfill task_id after task creation.
    fn backfill_decision_task_id(&self, decision_id: Option<i64>, task_id: &str) {
        if let (Some(id), Some(db)) = (decision_id, &self.db) {
            let repo = DispatchDecisionRepository::new(db);
            if let Err(e) = repo.update_task_id(id, task_id) {
                tracing::warn!("Failed to backfill decision task_id: {e}");
            }
        }
    }

    // ── Dispatch methods ────────────────────────────────────────────────

    /// Dispatch a complex task using heuristic skill matching:
    /// Matches required skills to idle agents, then delegates to dispatch_core.
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch(
        &self,
        request_id: Uuid,
        source: &str,
        description: &str,
        required_skills: &[String],
        created_by: &str,
        lane_key: &str,
        workspace_id: Option<String>,
    ) -> Result<DispatchOutcome, String> {
        let matches = match self
            .skill_matcher
            .match_skills(required_skills, &self.shared_context.agent_registry)
        {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    required_skills = ?required_skills,
                    description_len = description.len(),
                    source = source,
                    "Heuristic skill matching failed: {e}"
                );
                // Record the failed attempt so it appears in analytics
                let dd = decision::DispatchDecision {
                    mode: decision::DispatchMode::SequentialPipeline,
                    reason: decision::DecisionReason::HeuristicMatchFailed,
                    agent_count: 0,
                    dag_node_count: None,
                    predictability_score: None,
                    planner_requested_mode: None,
                    error_message: Some(e.clone()),
                    timestamp: Utc::now(),
                };
                self.record_decision(&request_id.to_string(), &dd);
                return Err(e);
            }
        };

        // Record heuristic dispatch decision
        let dd = decision::DispatchDecision {
            mode: decision::DispatchMode::SequentialPipeline,
            reason: decision::DecisionReason::HeuristicFallback,
            agent_count: matches.len(),
            dag_node_count: None,
            predictability_score: None,
            planner_requested_mode: None,
            error_message: None,
            timestamp: Utc::now(),
        };
        let decision_row_id = self.record_decision(&request_id.to_string(), &dd);

        let title = generate_title(description);
        let result = self.dispatch_core(
            description,
            title,
            matches,
            created_by,
            lane_key,
            source,
            workspace_id,
        );
        if let Ok(ref outcome) = result {
            self.backfill_decision_task_id(decision_row_id, &outcome.task_id);
        }
        result
    }

    /// Dispatch a task directly to the lead agent (heuristic fallback).
    /// Used when heuristic skill matching fails for ComplexTask intents.
    pub fn dispatch_lead_agent_heuristic(
        &self,
        request_id: Uuid,
        description: &str,
        created_by: &str,
        lane_key: &str,
        source: &str,
        workspace_id: Option<String>,
    ) -> Result<DispatchOutcome, String> {
        // Record heuristic lead-agent dispatch decision
        let dd = decision::DispatchDecision {
            mode: decision::DispatchMode::LeadAgent,
            reason: decision::DecisionReason::HeuristicFallback,
            agent_count: 0,
            dag_node_count: None,
            predictability_score: None,
            planner_requested_mode: None,
            error_message: None,
            timestamp: Utc::now(),
        };
        let decision_row_id = self.record_decision(&request_id.to_string(), &dd);

        let title = generate_title(description);
        let result = self.dispatch_lead_agent(
            description,
            title,
            created_by,
            lane_key,
            source,
            workspace_id,
        );
        if let Ok(ref outcome) = result {
            self.backfill_decision_task_id(decision_row_id, &outcome.task_id);
        }
        result
    }

    /// Dispatch a complex task using an LLM-generated plan.
    /// Validates that assigned agents exist and are idle, then delegates to dispatch_core.
    /// If `plan.use_lead_agent` is true, routes to the Lead Agent orchestration path.
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_planned(
        &self,
        request_id: Uuid,
        description: &str,
        plan: TaskPlan,
        created_by: &str,
        lane_key: &str,
        source: &str,
        workspace_id: Option<String>,
    ) -> Result<DispatchOutcome, String> {
        // ── Dispatch Analysis (Phase 2) ────────────────────────────────
        let dd = decision::analyze_plan(&plan);
        let decision_row_id = self.record_decision(&request_id.to_string(), &dd);
        // ── End Dispatch Analysis ──────────────────────────────────────

        let plan_classification = plan.classification.clone();
        let plan_title_ref = plan.title.as_deref().unwrap_or("<none>").to_string();

        // 1. Lead Agent path: dynamic orchestration for complex/exploratory tasks
        if plan.use_lead_agent {
            tracing::info!(
                classification = %plan_classification,
                plan_title = %plan_title_ref,
                description_len = description.len(),
                "dispatch_planned: use_lead_agent=true, routing to lead agent"
            );
            let title = plan
                .title
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| generate_title(description));
            let result = self.dispatch_lead_agent(
                description,
                title,
                created_by,
                lane_key,
                source,
                workspace_id,
            );
            if let Ok(ref outcome) = result {
                self.backfill_decision_task_id(decision_row_id, &outcome.task_id);
            }
            return result;
        }

        // 2. DAG path: planner emits assignments=[] with agent info in dag.nodes[].agent_id.
        //    Must check DAG presence BEFORE the empty-assignments fallback, otherwise
        //    DAG plans are silently rerouted to lead agent.
        if let Some(dag) = plan.dag {
            tracing::info!(
                classification = %plan_classification,
                plan_title = %plan_title_ref,
                dag_nodes = dag.nodes.len(),
                has_assignments = !plan.assignments.is_empty(),
                description_len = description.len(),
                "dispatch_planned: DAG present, routing to DAG-parallel execution"
            );
            let title = plan
                .title
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| generate_title(description));
            let result = self.dispatch_dag_planned(
                description,
                title,
                dag,
                created_by,
                lane_key,
                source,
                workspace_id,
            );
            if let Ok(ref outcome) = result {
                self.backfill_decision_task_id(decision_row_id, &outcome.task_id);
            }
            return result;
        }

        // 3. No DAG and no assignments — fallback to lead agent
        if plan.assignments.is_empty() {
            tracing::info!(
                classification = %plan_classification,
                plan_title = %plan_title_ref,
                description_len = description.len(),
                "dispatch_planned: no agent assignments and no DAG, falling back to lead agent"
            );
            let title = plan
                .title
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| generate_title(description));
            let result = self.dispatch_lead_agent(
                description,
                title,
                created_by,
                lane_key,
                source,
                workspace_id,
            );
            if let Ok(ref outcome) = result {
                self.backfill_decision_task_id(decision_row_id, &outcome.task_id);
            }
            return result;
        }

        // 4. Sequential pipeline: assignments provided, no DAG
        tracing::info!(
            classification = %plan_classification,
            plan_title = %plan_title_ref,
            assignment_count = plan.assignments.len(),
            description_len = description.len(),
            "dispatch_planned: routing to sequential pipeline"
        );
        let matches: Vec<SkillMatch> = plan
            .assignments
            .iter()
            .map(|a| SkillMatch {
                agent_id: a.agent_id.clone(),
                agent_name: a.agent_name.clone(),
                matched_skills: a.matched_skills.clone(),
                role_description: a.role_description.clone(),
            })
            .collect();

        let title = plan
            .title
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| generate_title(description));

        let result = self.dispatch_core(
            description,
            title,
            matches,
            created_by,
            lane_key,
            source,
            workspace_id,
        );
        if let Ok(ref outcome) = result {
            self.backfill_decision_task_id(decision_row_id, &outcome.task_id);
        }
        result
    }
}

/// Generate a concise task title from a description by stripping filler prefixes
/// and truncating to a reasonable length.
pub(super) fn generate_title(description: &str) -> String {
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
