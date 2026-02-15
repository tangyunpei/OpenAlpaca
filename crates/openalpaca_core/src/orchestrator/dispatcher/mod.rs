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

use crate::memory::task_extraction::{TaskExtractionParams, extract_task_memories};
use openalpaca_storage::repository::MemoryRepository;
use super::skill_matcher::{SkillMatch, SkillMatcher};
use super::task_planner::TaskPlan;

/// Retrieve relevant user memories as a formatted block for agent prompts.
/// Mirrors the retrieval pattern used in `handle_simple_query()`.
pub(super) async fn retrieve_memory_block(
    db: &Database,
    embedder: Option<&Arc<dyn openalpaca_llm::Embedder>>,
    owner_id: &str,
    query: &str,
    top_k: usize,
) -> Option<String> {
    let repo = MemoryRepository::new(db);
    let query_embedding = if let Some(embedder) = embedder {
        embedder
            .embed(&[query])
            .await
            .ok()
            .and_then(|v| v.into_iter().next())
    } else {
        None
    };
    let memories = repo
        .search_hybrid(
            owner_id,
            query,
            query_embedding.as_deref(),
            top_k,
            None,
            None,
            None,
        )
        .unwrap_or_default();

    if memories.is_empty() {
        return None;
    }

    // Track access for importance decay
    let ids: Vec<i64> = memories.iter().map(|m| m.id).collect();
    if let Err(e) = repo.touch_accessed(&ids) {
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
    ) -> Result<String, String> {
        let matches = self
            .skill_matcher
            .match_skills(required_skills, &self.shared_context.agent_registry)?;
        let title = generate_title(description);
        self.dispatch_core(description, title, matches, created_by, lane_key, source, None)
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
    ) -> Result<String, String> {
        // Lead Agent path: dynamic orchestration for complex/exploratory tasks
        if plan.use_lead_agent {
            let title = plan
                .title
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| generate_title(description));
            return self.dispatch_lead_agent(
                description, title, created_by, lane_key, source,
            );
        }

        if plan.assignments.is_empty() {
            return Err("No agents assigned by planner".to_string());
        }

        // Validate ALL planned agents are available (pipeline requires every step)
        let mut unavailable: Vec<String> = Vec::new();
        let mut matches: Vec<SkillMatch> = Vec::new();

        for a in &plan.assignments {
            let is_available = self
                .shared_context
                .agent_registry
                .get(&a.agent_id)
                .map(|agent| agent.status.is_available())
                .unwrap_or(false);

            if is_available {
                matches.push(SkillMatch {
                    agent_id: a.agent_id.clone(),
                    agent_name: a.agent_name.clone(),
                    matched_skills: a.matched_skills.clone(),
                    role_description: a.role_description.clone(),
                });
            } else {
                unavailable.push(format!("{} ({})", a.agent_name, a.agent_id));
            }
        }

        if !unavailable.is_empty() {
            return Err(format!(
                "Cannot start pipeline — these agents are unavailable: {}. All agents must be available for a sequential pipeline.",
                unavailable.join(", ")
            ));
        }

        if matches.is_empty() {
            return Err("No agents assigned by planner".to_string());
        }

        let dag = plan.dag;
        let title = plan
            .title
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| generate_title(description));

        self.dispatch_core(description, title, matches, created_by, lane_key, source, dag)
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
