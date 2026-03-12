pub mod builtins;
pub mod config;
pub mod contextual_executor;
pub mod executor;
pub mod platform;
pub mod registry;
pub mod url_validation;

pub use contextual_executor::{ContextualToolExecutor, ScriptExecutionContext, ToolExecutionContext};
pub use executor::RegistryToolExecutor;
pub use registry::ToolRegistry;

use crate::agent::SubAgent;
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

/// Resolve tool definitions for an agent based on its assigned skills.
///
/// Centralises the pattern that was previously duplicated across
/// `dag_executor`, `lead_agent`, and `pipeline`.
///
/// * Looks up tools matching the agent's skill names.
/// * Appends workspace tool definitions so agents can collaborate.
/// * Optionally adds `memory_search` if the agent has the `memory_search`
///   skill assigned (rather than force-adding it for every agent).
pub fn resolve_agent_tools(
    agent: &SubAgent,
    tool_registry: &Arc<ToolRegistry>,
) -> Vec<ToolDefinition> {
    let skill_names: Vec<String> = agent.capabilities.iter().map(|s| s.name.clone()).collect();
    let mut tools = tool_registry.definitions_for_skills(&skill_names);

    // Add workspace tools when the agent has them as skills. Template-based agents
    // always have these injected by to_subagent(); this check is a safety net for
    // agents constructed via other paths (e.g. programmatic SubAgent creation).
    if skill_names
        .iter()
        .any(|s| s == "workspace_read" || s == "workspace_write")
    {
        tools.extend(builtins::workspace_tool_definitions());
    }

    // Add memory_search only when the agent explicitly lists it as a skill,
    // respecting least-privilege (previously it was force-added for all agents).
    if skill_names.iter().any(|s| s == "memory_search")
        && !tools.iter().any(|t| t.name == "memory_search")
        && let Some(mem_tool) = tool_registry.get("memory_search")
    {
        tools.push(mem_tool.definition.clone());
    }

    tools
}
