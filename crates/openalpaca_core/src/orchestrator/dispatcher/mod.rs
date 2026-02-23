//! Task dispatcher: creates tasks, assigns agents, starts task lanes.

mod core;
mod dag;
pub(crate) mod decision;
mod lead_agent;
mod pipeline;
#[cfg(test)]
mod tests;
pub(crate) mod usage;

use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::daemon_config::DaemonConfig;
use crate::events::SystemEvent;
use crate::lane::LaneManager;
use crate::security::gate::SecurityGate;
use arc_swap::ArcSwap;
use chrono::Utc;
use openalpaca_llm::LlmRouter;
use openalpaca_storage::Database;
use std::sync::Arc;
use uuid::Uuid;

use crate::tools::ToolRegistry;

use super::skill_matcher::{SkillMatch, SkillMatcher};
use super::task_planner::TaskPlan;
use crate::memory::scope_context::MemoryScopeContext;
use crate::memory::task_extraction::{TaskExtractionParams, extract_task_memories};
use openalpaca_storage::repository::MemoryRepository;
use openalpaca_storage::repository::dispatch_decision::{
    DispatchDecisionRecord, DispatchDecisionRepository,
};

/// Retrieve relevant user memories as a formatted block for agent prompts.
/// Mirrors the retrieval pattern used in `handle_simple_query()`.
///
/// When `scope_ctx` is provided, uses cascading search (Workspace → Global).
/// When `None`, falls back to unscoped global search (backward compatibility for
/// pipeline and lead_agent contexts that don't yet carry scope context).
pub(super) async fn retrieve_memory_block(
    db: &Database,
    embedder: Option<&Arc<dyn openalpaca_llm::Embedder>>,
    owner_id: &str,
    query: &str,
    top_k: usize,
    scope_ctx: Option<&MemoryScopeContext>,
    access_boost: f64,
) -> Option<String> {
    let repo = MemoryRepository::new(db);
    let query_embedding = if let Some(embedder) = embedder {
        match embedder.embed(&[query]).await {
            Ok(v) => v.into_iter().next(),
            Err(e) => {
                tracing::warn!("Memory embedding failed, falling back to text-only search: {e}");
                None
            }
        }
    } else {
        None
    };
    let memories = if let Some(ctx) = scope_ctx {
        let cascade_scopes = ctx.cascade_scopes();
        match repo.search_hybrid_cascade(
            owner_id,
            query,
            query_embedding.as_deref(),
            top_k,
            None,
            &cascade_scopes,
        ) {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!("Memory cascade search failed: {e}");
                Vec::new()
            }
        }
    } else {
        match repo.search_hybrid(
            owner_id,
            query,
            query_embedding.as_deref(),
            top_k,
            None,
            None,
            None,
        ) {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!("Memory search failed: {e}");
                Vec::new()
            }
        }
    };

    if memories.is_empty() {
        return None;
    }

    // Track access for importance decay + boost
    let ids: Vec<i64> = memories.iter().map(|m| m.id).collect();
    if let Err(e) = repo.touch_accessed(&ids, access_boost) {
        tracing::warn!("Failed to track memory access: {e}");
    }

    let mut inner = String::new();
    let mut budget = 2000usize;
    for m in &memories {
        let entry = format!(
            "- [{}] {}\n",
            m.kind.as_str(),
            m.content.chars().take(300).collect::<String>()
        );
        if entry.len() > budget {
            break;
        }
        budget -= entry.len();
        inner.push_str(&entry);
    }
    Some(super::wrap_untrusted_context(&inner, "retrieved_memory", "retrieved"))
}

/// Spawn a background task to extract memories from a completed task output.
/// Fire-and-forget: does not block the caller. Only runs for successful tasks.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_task_memory_extraction(
    db: &Database,
    router: &Arc<LlmRouter>,
    embedder: &Option<Arc<dyn openalpaca_llm::Embedder>>,
    daemon_config: &Arc<ArcSwap<DaemonConfig>>,
    owner_id: &str,
    task_id: &str,
    task_description: &str,
    task_output: &str,
    source_path: &str,
    success: bool,
    workspace_id: Option<String>,
) {
    if !success {
        return;
    }
    let dcfg = daemon_config.load();
    if !dcfg.orchestrator.costs.task_extract_enabled {
        return;
    }

    let params = TaskExtractionParams {
        owner_id: owner_id.to_string(),
        task_id: task_id.to_string(),
        task_description: task_description.to_string(),
        task_output: task_output.to_string(),
        source_path: source_path.to_string(),
        workspace_id,
    };
    let db = db.clone();
    let router = router.clone();
    let embedder = embedder.clone();
    let daemon_config = daemon_config.clone();

    let task_id_for_log = params.task_id.clone();
    let handle = tokio::spawn(async move {
        extract_task_memories(params, db, router, embedder, daemon_config).await;
    });
    // Separate lightweight task to catch panics from the extraction task
    tokio::spawn(async move {
        if let Err(e) = handle.await {
            tracing::warn!(
                "Fire-and-forget memory extraction failed for task '{}': {e}",
                task_id_for_log,
            );
        }
    });
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
        }
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
                    timestamp: chrono::Utc::now(),
                });
                None
            }
        }
    }

    // ── Shared decision recording helpers ──────────────────────────────

    /// Record a dispatch decision (event + DB). Returns row ID for task_id backfill.
    fn record_decision(
        &self,
        request_id: &str,
        dd: &decision::DispatchDecision,
    ) -> Option<i64> {
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
    ) -> Result<String, String> {
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
        if let Ok(ref task_id) = result {
            self.backfill_decision_task_id(decision_row_id, task_id);
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
    ) -> Result<String, String> {
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
        let result =
            self.dispatch_lead_agent(description, title, created_by, lane_key, source, workspace_id);
        if let Ok(ref task_id) = result {
            self.backfill_decision_task_id(decision_row_id, task_id);
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
    ) -> Result<String, String> {
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
            if let Ok(ref task_id) = result {
                self.backfill_decision_task_id(decision_row_id, task_id);
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
            if let Ok(ref task_id) = result {
                self.backfill_decision_task_id(decision_row_id, task_id);
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
            if let Ok(ref task_id) = result {
                self.backfill_decision_task_id(decision_row_id, task_id);
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
        if let Ok(ref task_id) = result {
            self.backfill_decision_task_id(decision_row_id, task_id);
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
    if result.len() > 50 {
        format!("{}...", &result[..47])
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

/// Update task status in registry + DB + emit event for a completed or failed task.
pub(super) fn finalize_task(
    ctx: &crate::context::SharedContext,
    bus: &crate::bus::EventBus,
    db: Option<&openalpaca_storage::Database>,
    task_id: &str,
    summary: &str,
    success: bool,
) {
    let now = chrono::Utc::now();
    if success {
        ctx.task_registry
            .update_status(task_id, crate::context::TaskEntryStatus::Completed);
        if let Some(db) = db {
            let repo = openalpaca_storage::repository::TaskRepository::new(db);
            if let Err(e) = repo.update_status(task_id, openalpaca_storage::TaskStatus::Completed) {
                tracing::warn!(
                    "finalize_task: failed to update status for task '{}': {e}",
                    task_id
                );
            }
            if let Err(e) = repo.set_result(task_id, summary) {
                tracing::warn!(
                    "finalize_task: failed to set result for task '{}': {e}",
                    task_id
                );
            }
        }
        bus.publish(crate::events::SystemEvent::TaskCompleted {
            task_id: task_id.to_string(),
            result_summary: Some(summary.to_string()),
            timestamp: now,
        });
    } else {
        ctx.task_registry
            .update_status(task_id, crate::context::TaskEntryStatus::Failed);
        if let Some(db) = db {
            let repo = openalpaca_storage::repository::TaskRepository::new(db);
            if let Err(e) = repo.update_status(task_id, openalpaca_storage::TaskStatus::Failed) {
                tracing::warn!(
                    "finalize_task: failed to update status for task '{}': {e}",
                    task_id
                );
            }
            if let Err(e) = repo.set_result(task_id, summary) {
                tracing::warn!(
                    "finalize_task: failed to set result for task '{}': {e}",
                    task_id
                );
            }
        }
        bus.publish(crate::events::SystemEvent::TaskFailed {
            task_id: task_id.to_string(),
            error: summary.to_string(),
            timestamp: now,
        });
    }
}

/// Persist a task result as a conversation message.
#[allow(clippy::too_many_arguments)]
pub(super) fn persist_conversation(
    db: &openalpaca_storage::Database,
    lane_key: &str,
    source: &str,
    content: String,
    model: Option<String>,
    tokens_in: i64,
    tokens_out: i64,
    runtime_secs: i64,
) {
    let conv_repo = openalpaca_storage::ConversationRepository::new(db);
    let _ = conv_repo.get_or_create_conversation(lane_key, source);
    let _ = conv_repo.insert(&openalpaca_storage::ConversationMessage {
        id: 0,
        lane_key: lane_key.to_string(),
        role: "assistant".to_string(),
        content,
        source: Some(source.to_string()),
        model,
        tokens_in: Some(tokens_in),
        tokens_out: Some(tokens_out),
        duration_ms: Some(runtime_secs * 1000),
        created_at: String::new(),
    });
    let _ = conv_repo.increment_message_count(lane_key);
}
