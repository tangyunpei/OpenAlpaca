//! Tool registry construction and custom tool loading.

use arc_swap::ArcSwap;
use openalpaca_core::{
    bus::EventBus,
    tools::builtins::{ConnectorSendLock, PersonaToolContext},
};
use openalpaca_storage::Database;
use std::path::Path;
use std::sync::Arc;
use tracing::{info, warn};

#[allow(clippy::too_many_arguments)]
pub(super) async fn build_tool_registry(
    config_base_dir: &Path,
    db: &Database,
    embedder: &Option<Arc<dyn openalpaca_llm::Embedder>>,
    soul_path: &Path,
    user_path: &Path,
    identity_path: &Path,
    bus: &EventBus,
    daemon_config: &Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    web_search_config: &Arc<ArcSwap<openalpaca_llm::WebSearchConfig>>,
) -> anyhow::Result<(
    Arc<openalpaca_core::tools::ToolRegistry>,
    ConnectorSendLock,
    Arc<crate::managers::mcp::McpSupervisor>,
)> {
    let tool_registry = openalpaca_core::tools::ToolRegistry::new()
        .map_err(|e| anyhow::anyhow!(e))?;

    // Register built-in tools (including update_persona)
    let persona_ctx = PersonaToolContext {
        soul_path: soul_path.to_path_buf(),
        user_path: user_path.to_path_buf(),
        identity_path: identity_path.to_path_buf(),
        backup_dir: config_base_dir.join("orchestrator").join("backups"),
        bus: bus.clone(),
        max_backups: Some(10),
    };
    // Capture workspace root once at startup for file tools, avoiding
    // reliance on the process-global current_dir() at tool execution time.
    let workspace_root = std::env::current_dir().ok();
    // Create the shared lock for the send tool's connector send provider.
    // The actual provider is set post-construction in main.rs after ConnectorSendBridge is created.
    let connector_send_lock: ConnectorSendLock = Arc::new(std::sync::RwLock::new(None));
    for tool in openalpaca_core::tools::builtins::builtin_tools_with_persona_context(
        Some(db.clone()),
        embedder.clone(),
        persona_ctx,
        Some(daemon_config.clone()),
        Some(web_search_config.clone()),
        workspace_root,
        Some(connector_send_lock.clone()),
    ) {
        // Built-in tools have known-good names — unwrap is safe.
        tool_registry.register(tool).unwrap();
    }

    // Load user tools from config/tools/*.toml
    let tools_config_dir = config_base_dir.join("tools");

    // Collect names of all registered built-in tools dynamically
    let builtin_names: std::collections::HashSet<String> = tool_registry
        .registered_tool_names()
        .into_iter()
        .collect();

    // Runtime-injected tools not in the registry but still protected
    // (e.g., spawn_subagent and its variants are provided by LeadAgentToolExecutor
    // at runtime, not the global registry, but must still be protected from
    // TOML name collisions).
    let runtime_protected: std::collections::HashSet<&str> = [
        "spawn_subagent",
        "spawn_subagents_batch",
        "check_subagent_status",
        "wait_for_subagents",
    ].into_iter().collect();

    for tool in openalpaca_core::tools::config::load_tools_from_dir(&tools_config_dir) {
        let name = tool.definition.name.clone();
        if builtin_names.contains(name.as_str()) || runtime_protected.contains(name.as_str()) {
            warn!(
                "Custom tool '{}' would override a protected tool — skipping",
                name
            );
            continue;
        }
        if tool_registry.get(&name).is_some() {
            warn!(
                "Custom tool '{}' conflicts with an existing tool name and will override it",
                name
            );
        }
        match tool_registry.register(tool) {
            Ok(()) => info!("Registered custom tool: {}", name),
            Err(e) => {
                warn!("Custom tool '{}' failed validation: {} — skipping", name, e);
                continue;
            }
        }
    }
    info!("Tool registry: {} tools loaded", tool_registry.count());

    if tool_registry.get("update_persona").is_none() {
        tracing::error!("update_persona tool failed to register — persona updates will not work");
    }

    let tool_registry = Arc::new(tool_registry);

    // Bring up the MCP supervisor (from config/mcp.toml) and reconcile once.
    // Never fatal: an unparseable store parks one pseudo-record and boots.
    let mcp_supervisor = super::mcp::build_mcp_supervisor(
        config_base_dir,
        &tool_registry,
        daemon_config,
        bus,
    )
    .await;

    Ok((tool_registry, connector_send_lock, mcp_supervisor))
}
