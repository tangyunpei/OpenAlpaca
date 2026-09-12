//! MCP server bootstrap.
//!
//! Boot is now the [`McpSupervisor`](crate::managers::mcp::McpSupervisor)'s
//! **first `reconcile_all()`** — not a separate code path (extension design
//! §5). Three things follow from that, and they are the whole of this file's
//! change in C2:
//!
//! * a disabled server builds a **listable `Disabled` record** instead of the
//!   bare `continue` that made it invisible (and silently depressed the boot
//!   log's `connected/total` ratio);
//! * servers are brought up with `join_all` rather than one at a time, so the
//!   first request after boot sees a connected or a `Failed` record, never a
//!   pending one;
//! * a `config/mcp.toml` that does not parse is **no longer fatal** — it used
//!   to `?`-propagate through `services/tools.rs` and `services/mod.rs` and
//!   stop the daemon booting at all, with no fall-back-to-defaults. It now
//!   registers one pseudo-record naming the parse error and boots.

use std::path::Path;
use std::sync::Arc;

use arc_swap::ArcSwap;
use openalpaca_core::bus::EventBus;
use openalpaca_core::daemon_config::DaemonConfig;
use openalpaca_core::tools::ToolRegistry;
use openalpaca_core::tools::extensions::ExtensionSupervisor;

use crate::managers::mcp::McpSupervisor;

/// Build the MCP supervisor over `config/mcp.toml`, reconcile it once, and
/// start its crash reaper.
///
/// Registered tools hold `Arc`s to their `McpClient`, but the supervisor holds
/// its own outside the registry — teardown must never depend on dropping the
/// last registry `Arc`, because `McpClient` has no `Drop` impl and an implicit
/// drop performs no close at all.
pub(super) async fn build_mcp_supervisor(
    config_base_dir: &Path,
    tool_registry: &Arc<ToolRegistry>,
    daemon_config: &Arc<ArcSwap<DaemonConfig>>,
    bus: &EventBus,
    skill_catalog: &Arc<openalpaca_core::orchestrator::skill_catalog::SkillCatalog>,
    agent_registry: &Arc<openalpaca_core::agent::AgentRegistry>,
    default_lane_key: &str,
) -> Arc<McpSupervisor> {
    let supervisor = McpSupervisor::new(
        config_base_dir.join("mcp.toml"),
        Arc::clone(tool_registry),
        Arc::clone(daemon_config),
        bus.clone(),
        // T1 step 3's dependent scan reads both registries and writes its cron
        // notice to the default lane (extension design §7.3). `PluginManager`
        // already holds the same two handles.
        Some(Arc::clone(skill_catalog)),
        Some(Arc::clone(agent_registry)),
        default_lane_key,
    );

    supervisor.reconcile_all().await;

    let rows = supervisor.list().await;
    let connected = rows.iter().filter(|r| r.state.is_enabled()).count();
    tracing::info!(
        connected,
        total = rows.len(),
        "MCP server bootstrap complete"
    );

    // The reaper is **not** started here: it is started after
    // `load_agent_templates` (`services/mod.rs`), so a boot-window crash's
    // dependent scan can name the templates that declare the lost capabilities
    // rather than reporting an empty set (C4 review).
    supervisor
}
