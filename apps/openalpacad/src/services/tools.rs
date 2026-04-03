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
pub(super) fn build_tool_registry(
    config_base_dir: &Path,
    db: &Database,
    embedder: &Option<Arc<dyn openalpaca_llm::Embedder>>,
    soul_path: &Path,
    user_path: &Path,
    identity_path: &Path,
    bus: &EventBus,
    daemon_config: &Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    web_search_config: &Arc<ArcSwap<openalpaca_llm::WebSearchConfig>>,
) -> anyhow::Result<(Arc<openalpaca_core::tools::ToolRegistry>, ConnectorSendLock)> {
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
        tool_registry.register(tool);
    }

    // Load user tools from config/tools/*.toml
    let tools_config_dir = config_base_dir.join("tools");
    // Security-critical tool names that TOML configs must not override.
    // Includes both registry-registered built-ins and runtime-injected tools
    // (e.g., spawn_subagent and its variants are provided by LeadAgentToolExecutor
    // at runtime, not the global registry, but must still be protected from
    // TOML name collisions).
    let protected_builtins: &[&str] = &[
        // Registry built-ins
        "update_persona",
        "shell_execute",
        "file_read",
        "file_write",
        "memory_search",
        // ContextualToolExecutor runtime tools
        "workspace_read",
        "workspace_write",
        // LeadAgentToolExecutor runtime tools
        "spawn_subagent",
        "spawn_subagents_batch",
        "check_subagent_status",
        "wait_for_subagents",
        // Connector tools
        "send",
    ];
    for tool in openalpaca_core::tools::config::load_tools_from_dir(&tools_config_dir) {
        if protected_builtins.contains(&tool.definition.name.as_str()) {
            warn!(
                "Custom tool '{}' would override a security-critical built-in — skipping",
                tool.definition.name
            );
            continue;
        }
        if tool_registry.get(&tool.definition.name).is_some() {
            warn!(
                "Custom tool '{}' conflicts with an existing tool name and will override it",
                tool.definition.name
            );
        }
        info!("Registered custom tool: {}", tool.definition.name);
        tool_registry.register(tool);
    }
    info!("Tool registry: {} tools loaded", tool_registry.count());

    if tool_registry.get("update_persona").is_none() {
        tracing::error!("update_persona tool failed to register — persona updates will not work");
    }

    Ok((Arc::new(tool_registry), connector_send_lock))
}
