//! LLM-based task planning: replaces keyword heuristics with a single LLM call
//! that classifies intent, generates a title, and assigns agents.
//!
//! Supports hierarchical planning: for complex tasks, the planner can decompose
//! the objective into a DAG of sub-tasks with dependencies.

mod prompt;
mod response_parser;
pub mod types;

#[cfg(test)]
mod tests;

pub(crate) use response_parser::extract_json_block;
pub use types::*;

// Re-export internal items needed by tests
#[cfg(test)]
use prompt::has_predictable_structure;
#[cfg(test)]
use std::collections::HashSet;

use crate::agent::subagent::SubAgent;
use crate::compose::{
    AgentConfig, ComposeEngine, ComposeOverrides, ComposeRequest, DynamicContextInput,
    DynamicContextMode, HistoryInput, HistoryMode, PersonaInput, PersonaMode, StaticPromptInput,
    StaticPromptMode, SummaryWrapMode,
};
use crate::middleware::prompt::SystemPersona;
use crate::prompt_ctx::{AgentSummary, ContextBundle, ExecutionPath};
use openalpaca_llm::{ChatMessage, LlmRouter};
use std::sync::Arc;

pub struct TaskPlanner;

impl TaskPlanner {
    /// Hierarchical planning: decompose a complex task into a DAG of sub-tasks.
    /// Falls back to flat assignment if DAG planning fails or returns simple_query.
    ///
    /// Routes prompt assembly through the layered compose engine (Phase 4
    /// Commit 3). The previous `build_hierarchical_prompt` helper was absorbed
    /// into `compose::static_prompt::build_planner_hierarchical`; this method
    /// serializes the structured inputs (idle agents, session summary,
    /// active tasks, history tail, current user turn) into the four layer
    /// inputs before calling `compose_engine.compose(...)`.
    #[allow(clippy::too_many_arguments)]
    pub async fn plan_hierarchical(
        router: &LlmRouter,
        compose_engine: &ComposeEngine,
        user_message: &str,
        idle_agents: &[SubAgent],
        history: &[ChatMessage],
        session_summary: Option<&str>,
        active_tasks_block: Option<&str>,
        limits: PlannerLimits,
        dag_prefer_predictable: bool,
    ) -> Result<TaskPlan, PlanError> {
        // History windowing — pre-migration logic from
        // `prompt::build_messages` (lines 106-110 pre-deletion).
        let history_tail: Vec<ChatMessage> = if history.len() > PLANNING_HISTORY_LIMIT {
            history[history.len() - PLANNING_HISTORY_LIMIT..].to_vec()
        } else {
            history.to_vec()
        };

        // Build Layer 4 recent_messages:
        //   [active_tasks_block_wrapped (optional), ...history_tail]
        // Summary is handled separately via Layer 4's SummaryWrapMode::UntrustedWrap.
        // Pre-migration ordering was:
        //   system | summary? | active_tasks? | history_tail... | current_user_turn
        // Layer 4 renders as `[summary?, ...recent_messages, current_user_turn?]`,
        // so placing `active_tasks_block` FIRST in recent_messages (before the
        // history tail) reproduces the pre-migration order exactly.
        let mut recent: Vec<ChatMessage> = Vec::with_capacity(1 + history_tail.len());
        if let Some(tasks_block) = active_tasks_block {
            recent.push(ChatMessage::user(
                &crate::orchestrator::wrap_untrusted_context(
                    tasks_block,
                    "active_tasks",
                    "user_derived",
                ),
            ));
        }
        recent.extend(history_tail);

        // Session summary — pre-migration code capped to
        // PLANNING_SUMMARY_MAX_CHARS chars AND prefixed the capped text with
        // "### SESSION SUMMARY ###\n" BEFORE wrapping. Layer 4's UntrustedWrap
        // takes the Arc<str> verbatim and wraps it, so we pass the prefixed
        // capped text through.
        let summary_wrapped: Option<Arc<str>> = session_summary.map(|s| {
            let capped: String = s.chars().take(PLANNING_SUMMARY_MAX_CHARS).collect();
            Arc::<str>::from(format!("### SESSION SUMMARY ###\n{}", capped))
        });

        let history_input = HistoryInput {
            lane_tip_fingerprint: [0u8; 32],
            summary: summary_wrapped,
            summary_wrap_mode: SummaryWrapMode::UntrustedWrap,
            recent_messages: Arc::new(recent),
            current_user_turn: Some(ChatMessage::user(user_message)),
            mode: HistoryMode::Default,
        };

        // Persona: Minimal with empty docs — planner's system prompt is built
        // entirely by Layer 2 and does not surface persona blocks. Layer 1 is
        // still required so the compose pipeline keeps a uniform cache shape.
        let persona_input = PersonaInput {
            system_persona: Arc::new(SystemPersona::default()),
            user_document: Arc::new(None),
            identity_document: Arc::new(None),
            persona_version: 0,
            mode: PersonaMode::Minimal,
            identity_budget: None,
            user_budget: None,
        };
        let persona_output = Arc::new(crate::compose::persona::compute(&persona_input));

        // Clone idle_agents into the AgentConfig (= SubAgent) slot that Layer 2
        // reads via `planner_agents`.
        let planner_agents: Arc<Vec<AgentConfig>> = Arc::new(idle_agents.to_vec());

        let static_prompt_input = StaticPromptInput {
            persona_output,
            agent_persona: None,
            agent_config_fingerprint: [0u8; 32],
            skill_block: None,
            skills_catalog: None,
            bootstrap: None,
            tools: Arc::new(Vec::new()),
            connector_status: Arc::new(Vec::new()),
            send_tool_context: None,
            message_source: None,
            raw_blocks: Vec::new(),
            planner_agents: Some(planner_agents),
            planner_protocol_v2: limits.plan_protocol_v2_enabled,
            mode: StaticPromptMode::PlannerHierarchical,
            model_window: 8192,
        };

        let dynamic_context_input = DynamicContextInput {
            context_bundle: Arc::new(ContextBundle::empty()),
            query: Arc::from(user_message),
            memory_retrieval_hash: [0u8; 32],
            path: ExecutionPath::SimpleQuery,
            reserved_tokens: 0,
            mode: DynamicContextMode::Skip,
        };

        // Build the AgentSummary list for the ComposeRequest::Planner variant.
        // Note: Layer 2 reads `planner_agents` (in StaticPromptInput) for the
        // actual `<agents>` XML block — the `idle_agents` field on the request
        // is currently informational / for fingerprint routing.
        let idle_summaries: Arc<Vec<AgentSummary>> = Arc::new(
            idle_agents
                .iter()
                .map(|a| AgentSummary {
                    name: a.name.clone(),
                    role: a.description.clone().unwrap_or_default(),
                    step: 0,
                })
                .collect(),
        );

        let request = ComposeRequest::Planner {
            idle_agents: idle_summaries,
            user_message: user_message.to_string(),
            active_tasks_block: active_tasks_block.map(Arc::<str>::from),
            overrides: ComposeOverrides::default(),
        };

        let composed = compose_engine.compose(
            &request,
            persona_input,
            static_prompt_input,
            dynamic_context_input,
            history_input,
            8192,
            Arc::new(Vec::new()),
            None, // no event_bus in this scope — acceptable
            None, // planner has no conversation lane
        );

        let mut messages: Vec<ChatMessage> = composed.messages.as_ref().clone();

        // Preserve the DAG-predictability hint injection (pre-migration
        // task_planner/mod.rs:53-64). Inserts a system hint before the final
        // user message when the user's content contains enumerated or
        // parallel sub-tasks.
        if dag_prefer_predictable && prompt::has_predictable_structure(user_message) {
            let hint = ChatMessage::system(
                "[SYSTEM HINT: This message contains enumerated or parallel sub-tasks. \
                 Prefer a DAG with parallel nodes if all steps are known upfront. \
                 Set use_lead_agent to false when using DAG.]",
            );
            let last_idx = messages.len().saturating_sub(1);
            messages.insert(last_idx, hint);
        }

        response_parser::plan_inner(router, messages, limits, idle_agents).await
    }
}
