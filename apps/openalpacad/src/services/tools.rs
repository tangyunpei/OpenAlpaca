//! Tool registry construction and custom tool loading.

use arc_swap::ArcSwap;
use openalpaca_core::{
    bus::EventBus,
    memory::service::MemoryService,
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
    memory_service: &Arc<MemoryService>,
    soul_path: &Path,
    user_path: &Path,
    identity_path: &Path,
    bus: &EventBus,
    daemon_config: &Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    web_search_config: &Arc<ArcSwap<openalpaca_llm::WebSearchConfig>>,
    skill_catalog: &Arc<openalpaca_core::orchestrator::skill_catalog::SkillCatalog>,
    agent_registry: &Arc<openalpaca_core::agent::AgentRegistry>,
    default_lane_key: &str,
) -> anyhow::Result<(
    Arc<openalpaca_core::tools::ToolRegistry>,
    ConnectorSendLock,
    Arc<crate::managers::mcp::McpSupervisor>,
)> {
    // **The one production `with_event_bus` call** (extension design §7.1).
    // C1 landed the constructor and left every caller on `new()`; C4 installs
    // the bus here, together with the `ExtensionCapabilityWithheld` variant it
    // exists to publish. The other 100 `new()`/`default()` sites are tests: a
    // ledger with no bus logs and returns.
    let tool_registry = openalpaca_core::tools::ToolRegistry::with_event_bus(bus.clone())
        .map_err(|e| anyhow::anyhow!(e))?;

    // Register built-in tools (including update_persona)
    let persona_ctx = PersonaToolContext {
        soul_path: soul_path.to_path_buf(),
        user_path: user_path.to_path_buf(),
        identity_path: identity_path.to_path_buf(),
        backup_dir: crate::bootstrap::persona_backup_dir(config_base_dir),
        bus: bus.clone(),
        max_backups: Some(crate::bootstrap::PERSONA_MAX_BACKUPS),
    };
    // Capture workspace root once at startup for file tools, avoiding
    // reliance on the process-global current_dir() at tool execution time.
    let workspace_root = std::env::current_dir().ok();
    // Create the shared lock for the send tool's connector send provider.
    // The actual provider is set post-construction in main.rs after ConnectorSendBridge is created.
    let connector_send_lock: ConnectorSendLock = Arc::new(std::sync::RwLock::new(None));
    for tool in openalpaca_core::tools::builtins::builtin_tools_with_persona_context(
        Some(db.clone()),
        persona_ctx,
        Some(daemon_config.clone()),
        Some(web_search_config.clone()),
        workspace_root,
        Some(connector_send_lock.clone()),
    ) {
        // Built-in tools have known-good names — unwrap is safe.
        tool_registry.register(tool).unwrap();
    }
    register_memory_read_tools(&tool_registry, memory_service);

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
        skill_catalog,
        agent_registry,
        default_lane_key,
    )
    .await;

    Ok((tool_registry, connector_send_lock, mcp_supervisor))
}

/// The memory read surface — `memory_search`, `memory_read`, `memory_list`,
/// `memory_timeline` (memory redesign, design §7; design 09 §13.1):
/// registered globally, each providing the `memory_read` capability, so
/// every agent template that grants it has them; the writers are the main
/// loop's per-request tools and are never registered here.
fn register_memory_read_tools(
    tool_registry: &openalpaca_core::tools::ToolRegistry,
    memory_service: &Arc<MemoryService>,
) {
    for tool in openalpaca_core::tools::builtins::memory_read_tools(memory_service.clone()) {
        // Built-in names, known good.
        tool_registry.register(tool).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::HomeStoreGuard;

    /// The daemon's registry offers the four read tools under `memory_read`,
    /// read-only, and none of the writers.
    #[test]
    fn the_memory_read_surface_is_registered_under_memory_read() {
        let home = tempfile::tempdir().unwrap();
        let _env = HomeStoreGuard::set(&home.path().canonicalize().unwrap());
        let service = Arc::new(MemoryService::open_default(None));
        let registry = openalpaca_core::tools::ToolRegistry::default();
        register_memory_read_tools(&registry, &service);
        for name in [
            "memory_search",
            "memory_read",
            "memory_list",
            "memory_timeline",
        ] {
            let tool = registry
                .get(name)
                .unwrap_or_else(|| panic!("{name} registered"));
            assert_eq!(tool.provides_capabilities, vec!["memory_read".to_string()]);
            assert_eq!(
                tool.annotations.as_ref().and_then(|a| a.read_only_hint),
                Some(true),
                "{name} is read-only"
            );
        }
        for name in ["memory_store", "memory_update", "memory_forget"] {
            assert!(
                registry.get(name).is_none(),
                "{name} is per-request, never global"
            );
        }
        assert!(
            home.path()
                .join("state")
                .join("cache")
                .join("memory-index-v1.sqlite")
                .exists(),
            "the cache lives under the home store's state/cache"
        );
    }
}
