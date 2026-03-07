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
use prompt::{build_messages, has_predictable_structure};
#[cfg(test)]
use std::collections::HashSet;

use crate::agent::subagent::SubAgent;
use openalpaca_llm::{ChatMessage, LlmRouter};

pub struct TaskPlanner;

impl TaskPlanner {
    /// Hierarchical planning: decompose a complex task into a DAG of sub-tasks.
    /// Falls back to flat assignment if DAG planning fails or returns simple_query.
    #[allow(clippy::too_many_arguments)]
    pub async fn plan_hierarchical(
        router: &LlmRouter,
        user_message: &str,
        idle_agents: &[SubAgent],
        history: &[ChatMessage],
        session_summary: Option<&str>,
        active_tasks_block: Option<&str>,
        limits: PlannerLimits,
        dag_prefer_predictable: bool,
    ) -> Result<TaskPlan, PlanError> {
        let system_prompt =
            prompt::build_hierarchical_prompt(idle_agents, limits.plan_protocol_v2_enabled);
        let mut messages = prompt::build_messages(
            &system_prompt,
            user_message,
            history,
            session_summary,
            active_tasks_block,
        );

        // If enabled, inject a system hint before the final user message
        // when the message contains predictable parallel structure.
        if dag_prefer_predictable && prompt::has_predictable_structure(user_message) {
            let hint = ChatMessage::system(
                "[SYSTEM HINT: This message contains enumerated or parallel sub-tasks. \
                 Prefer a DAG with parallel nodes if all steps are known upfront. \
                 Set use_lead_agent to false when using DAG.]",
            );
            // Insert before the last message (the user message)
            let last_idx = messages.len().saturating_sub(1);
            messages.insert(last_idx, hint);
        }

        response_parser::plan_inner(router, messages, limits, idle_agents).await
    }

    /// Build the hierarchical planning prompt with DAG support.
    #[cfg(test)]
    fn build_hierarchical_prompt(idle_agents: &[SubAgent], plan_protocol_v2: bool) -> String {
        prompt::build_hierarchical_prompt(idle_agents, plan_protocol_v2)
    }
}
