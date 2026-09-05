pub mod builtins;
pub mod config;
pub mod extensions;
pub mod mcp;
pub mod platform;
pub mod registry;
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
///
/// **S4 moment 2** (extension design §7.2, §7.5): a declared capability that no
/// longer resolves — wholly or partly — is announced with an attributed `warn!`
/// and `ExtensionCapabilityWithheld { Moment::SurfaceAssembly }`, and is *not*
/// surfaced in chat: the user did not name this subagent, and interrupting a
/// running workflow to report a template's declaration is noise.
///
/// `ctx` is what the announcement is deduped on. Keyed on the task, a lead that
/// spawns eight subagents from one template announces **once** (§7.4).
pub fn resolve_agent_tools(
    agent: &SubAgent,
    tool_registry: &Arc<ToolRegistry>,
    ctx: Option<&ToolContext>,
) -> Vec<ToolDefinition> {
    let caps: Vec<String> = agent.capabilities.iter().map(|c| c.name.clone()).collect();
    let denied: Vec<String> = agent.constraints.denied_capabilities.clone();
    let resolution = tool_registry.resolve_capabilities(&caps, &denied);
    tool_registry.announce_withheld(&resolution, ctx, None);
    resolution.defs
}
