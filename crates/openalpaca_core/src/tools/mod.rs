pub mod builtins;
pub mod config;
pub mod mcp;
pub mod platform;
pub mod registry;
pub mod stats;
pub mod url_validation;

pub use registry::ToolContext;
pub use registry::ToolRegistry;

use crate::agent::SubAgent;
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

/// Resolve tools for an agent based on its declared capabilities.
/// Uses capability intersection: a tool is included if any of its
/// `provides_capabilities` matches any of the agent's capabilities,
/// and none of the tool's capabilities are in the agent's denied list.
pub fn resolve_agent_tools(
    agent: &SubAgent,
    tool_registry: &Arc<ToolRegistry>,
) -> Vec<ToolDefinition> {
    let caps: Vec<String> = agent.capabilities.iter().map(|c| c.name.clone()).collect();
    let denied: Vec<String> = agent.constraints.denied_capabilities.clone();
    tool_registry.tools_for_capabilities_with_deny(&caps, &denied)
}
