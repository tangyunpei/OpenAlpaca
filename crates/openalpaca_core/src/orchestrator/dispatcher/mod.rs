//! Task dispatcher: creates tasks, assigns agents, starts task lanes.

mod core;
mod dag;
mod lead_agent;
mod pipeline;
#[cfg(test)]
mod tests;

use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::daemon_config::DaemonConfig;
use arc_swap::ArcSwap;
use crate::lane::LaneManager;
use crate::security::gate::SecurityGate;
use openalpaca_llm::LlmRouter;
use openalpaca_storage::Database;
use std::sync::Arc;
use uuid::Uuid;

use crate::tools::ToolRegistry;

use crate::memory::scope_context::MemoryScopeContext;
use crate::memory::task_extraction::{TaskExtractionParams, extract_task_memories};
use openalpaca_storage::repository::MemoryRepository;
use super::skill_matcher::{SkillMatch, SkillMatcher};
use super::task_planner::TaskPlan;

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

    let mut block = String::from("### RETRIEVED MEMORY ###\n");
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
        block.push_str(&entry);
    }
    Some(block)
}

/// Spawn a background task to extract memories from a completed task output.
/// Fire-and-forget: does not block the caller. Only runs for successful tasks.
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

    tokio::spawn(async move {
        extract_task_memories(params, db, router, embedder, daemon_config).await;
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

    /// Dispatch a complex task using heuristic skill matching:
    /// Matches required skills to idle agents, then delegates to dispatch_core.
    pub fn dispatch(
        &self,
        _request_id: Uuid,
        source: &str,
        description: &str,
        required_skills: &[String],
        created_by: &str,
        lane_key: &str,
        workspace_id: Option<String>,
    ) -> Result<String, String> {
        let matches = self
            .skill_matcher
            .match_skills(required_skills, &self.shared_context.agent_registry)?;
        let title = generate_title(description);
        self.dispatch_core(description, title, matches, created_by, lane_key, source, None, workspace_id)
    }

    /// Dispatch a complex task using an LLM-generated plan.
    /// Validates that assigned agents exist and are idle, then delegates to dispatch_core.
    /// If `plan.use_lead_agent` is true, routes to the Lead Agent orchestration path.
    pub fn dispatch_planned(
        &self,
        description: &str,
        plan: TaskPlan,
        created_by: &str,
        lane_key: &str,
        source: &str,
        workspace_id: Option<String>,
    ) -> Result<String, String> {
        // Lead Agent path: dynamic orchestration for complex/exploratory tasks
        if plan.use_lead_agent {
            let title = plan
                .title
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| generate_title(description));
            return self.dispatch_lead_agent(
                description, title, created_by, lane_key, source, workspace_id,
            );
        }

        if plan.assignments.is_empty() {
            return Err("No agents assigned by planner".to_string());
        }

        // Build matches from plan assignments — availability is checked
        // atomically via try_claim() inside dispatch_core().
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

        let dag = plan.dag;
        let title = plan
            .title
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| generate_title(description));

        self.dispatch_core(description, title, matches, created_by, lane_key, source, dag, workspace_id)
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
