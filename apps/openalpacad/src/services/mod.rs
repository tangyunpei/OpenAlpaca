//! Service initialization for daemon startup.

mod agents;
mod llm;
mod mcp;
mod tools;

pub use llm::{flush_cost_tracker, restore_cost_tracker};

use anyhow::Result;
use arc_swap::ArcSwap;
use openalpaca_core::{
    agent::AgentConfigService,
    bus::EventBus,
    context::SharedContext,
    tools::builtins::ConnectorSendLock,
};
use openalpaca_storage::Database;
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::info;

/// All services initialized during daemon startup.
pub struct InitializedServices {
    pub shared_context: Arc<SharedContext>,
    pub llm_router: Option<Arc<openalpaca_llm::LlmRouter>>,
    pub llm_settings_service: Option<Arc<openalpaca_llm::LlmSettingsService>>,
    pub token_manager: Option<Arc<openalpaca_llm::TokenManager>>,
    pub provider_usage_tracker: Option<Arc<openalpaca_llm::ProviderUsageTracker>>,
    pub embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    pub agent_config_service: Arc<AgentConfigService>,
    pub tool_registry: Arc<openalpaca_core::tools::ToolRegistry>,
    pub security_gate: Arc<openalpaca_core::security::gate::SecurityGate>,
    pub skill_catalog: Arc<openalpaca_core::orchestrator::skill_catalog::SkillCatalog>,
    pub skill_router: Arc<openalpaca_core::orchestrator::skill_router::SkillRouter>,
    pub secret_store: Arc<dyn openalpaca_llm::SecretStore>,
    pub web_search_config: Arc<ArcSwap<openalpaca_llm::WebSearchConfig>>,
    /// Shared lock for the `send` tool's connector send provider.
    /// Populated post-construction in main.rs after the ConnectorSendBridge is created.
    pub connector_send_lock: ConnectorSendLock,
    /// The MCP half of the ENABLE axis (extension design ADR-030, C2).
    ///
    /// Parked here between C2 and C6: the file watcher finds it here for edge
    /// case 15's `reconcile_all`, and the daemon shutdown path calls its
    /// `shutdown_all()` directly. C6 folds it into the `Extensions` aggregator
    /// and both call sites move behind that.
    pub mcp_supervisor: Arc<crate::managers::mcp::McpSupervisor>,
}

/// Initialize all core services: agent templates, LLM router, tools, security, etc.
#[allow(clippy::too_many_arguments)]
pub async fn initialize_services(
    config_base_dir: &Path,
    db: &Database,
    bus: &EventBus,
    daemon_config: &Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    soul_path: &Path,
    user_path: &Path,
    identity_path: &Path,
    cancel_token: &CancellationToken,
    // The daemon's default lane, `{local_user_id}:gui` — where T1 step 3's
    // cron notice is written (extension design §7.3 step 1).
    default_lane_key: &str,
) -> Result<InitializedServices> {
    let shared_context = Arc::new(SharedContext::new());

    // The session event log (§5.5). Parked on `SharedContext` the way the
    // event bus is, so the gateway, the dispatcher, the runner and the
    // steering rail all reach it without a global. It lives under the **home**
    // store's `sessions/`, never a project directory — a transcript carries
    // persona and cross-project content and must not be git-committable. A
    // store that cannot be resolved is a warning, not a boot failure: the
    // daemon runs without a log rather than not at all.
    match openalpaca_core::session_log::default_root() {
        Ok(root) => {
            let limits = openalpaca_core::session_log::SessionLogLimits {
                max_session_bytes: daemon_config
                    .load()
                    .orchestrator
                    .sessions
                    .log_max_session_bytes,
                ..Default::default()
            };
            // §5.4's global cap, "once at boot for the global cap". It runs
            // here — after the store movers, before any writer is handed out,
            // so nothing it examines is being appended to underneath it. The
            // per-session cap is the writer's and needs no boot pass.
            let swept = sweep_session_logs(&root, db, daemon_config).await;
            let mut service = openalpaca_core::session_log::SessionLogService::new(
                root,
                Some(db.clone()),
                limits,
                env!("CARGO_PKG_VERSION").to_string(),
            );
            // The pass's account outlives it: T44's status route reads
            // `over_cap_after` from here rather than from the boot log.
            if let Some(report) = swept {
                service = service.with_last_sweep(report);
            }
            shared_context.set_session_log(Arc::new(service));
        }
        Err(e) => tracing::warn!("Session event log disabled — no sessions directory: {e}"),
    }

    // Initialize secret store
    let llm_config_path = config_base_dir.join("llm.toml");
    let (secret_store, keyring_available) = llm::initialize_secret_store(&llm_config_path);

    // Build LLM router
    let llm_router = llm::build_llm_router(&llm_config_path, &*secret_store);

    // Build LLM settings service
    let llm_settings_service =
        llm::build_llm_settings_service(&llm_router, &llm_config_path, &secret_store).await;

    // Load LLM config for embedder / token manager
    let llm_config: Option<openalpaca_llm::LlmRouterConfig> = if llm_config_path.exists() {
        openalpaca_llm::read_config(&llm_config_path).ok()
    } else {
        None
    };

    // Build embedder
    let embedder = llm::build_embedder(&llm_config, &secret_store);

    // Credential discovery & token manager
    let cred_config = llm_config
        .as_ref()
        .and_then(|c| c.credential_discovery.clone())
        .unwrap_or_default();

    let token_manager = llm::build_token_manager(
        &cred_config,
        &llm_router,
        &llm_settings_service,
        cancel_token,
    )
    .await;

    // Provider usage tracker
    let provider_usage_tracker = if cred_config.fetch_external_usage.unwrap_or(false) {
        info!("Provider usage tracker enabled");
        Some(Arc::new(openalpaca_llm::ProviderUsageTracker::new()))
    } else {
        None
    };

    // Forward-migrate secrets if keychain is active
    if keyring_available && llm_config_path.exists() {
        match openalpaca_llm::migrate_llm_secrets(&llm_config_path, &*secret_store) {
            Ok(0) => {}
            Ok(n) => info!("Migrated {n} secret(s) to OS keychain"),
            Err(e) => tracing::warn!("Secret migration failed: {e}. Legacy secrets will still work."),
        }
    }

    // Build AgentConfigService
    let config_dir = config_base_dir.join("agents");
    let agent_config_service = Arc::new(AgentConfigService::new(
        shared_context.agent_registry.clone(),
        config_dir,
        db.clone(),
    ));

    // Extract web_search config from LLM config (hot-reloadable via ArcSwap)
    let web_search_cfg = llm_config
        .as_ref()
        .and_then(|c| c.web_search.clone())
        .unwrap_or_default();
    let web_search_config = Arc::new(ArcSwap::from_pointee(web_search_cfg));

    // Build SkillCatalog — **before** the tool registry, because the MCP
    // supervisor's first `reconcile_all` happens inside `build_tool_registry`
    // and T1 step 3's dependent scan needs the catalog handle from
    // construction (extension design §7.3). The catalog reads only
    // `config/skills`, so nothing here depends on the registry.
    let skill_catalog = {
        let catalog = openalpaca_core::orchestrator::skill_catalog::SkillCatalog::new();
        let skills_dir = config_base_dir.join("skills");
        if skills_dir.exists() {
            let count = catalog.scan_directory(
                &skills_dir,
                openalpaca_core::middleware::skill::SkillScope::Project,
            );
            info!(
                "Skill catalog: loaded {} skill(s) from {}",
                count,
                skills_dir.display()
            );
        }
        Arc::new(catalog)
    };

    // Build ToolRegistry
    let (tool_registry, connector_send_lock, mcp_supervisor) = tools::build_tool_registry(
        config_base_dir,
        db,
        &embedder,
        soul_path,
        user_path,
        identity_path,
        bus,
        daemon_config,
        &web_search_config,
        &skill_catalog,
        &shared_context.agent_registry,
        default_lane_key,
    )
    .await?;

    // Install the ENABLE axis's availability oracle (extension design §6.2
    // #12). `SkillRouter::route` takes only `(&str, &SkillCatalog)` and has no
    // registry handle, so the catalog carries `ToolRegistry` for it, for
    // `catalog_summary` (`<available_skills>`) and for the `invoke_skill`
    // listing. Wired here, the one place both exist.
    skill_catalog.set_availability_oracle(tool_registry.clone());

    // Load agent templates from .md files + legacy .toml files
    // (deferred until the tool registry exists so annotation: capabilities
    // in agent frontmatter can be validated against the known set.)
    agents::load_agent_templates(config_base_dir, db, &shared_context, &tool_registry)?;

    // The MCP crash reaper starts **after** the templates are loaded: its T1
    // step 3 intersects the withdrawn capabilities with the agent registry, and
    // a crash inside the boot window would otherwise report a dependent scan
    // that names nothing (extension design §7.3, C4 review).
    mcp_supervisor.spawn_reaper();

    // Build security chain
    let sandbox_manager = Arc::new(openalpaca_core::security::sandbox::SandboxManager::new(
        tool_registry.clone(),
        bus.clone(),
        &daemon_config.load().security.circuit_breaker,
    ));
    let security_gate = Arc::new(openalpaca_core::security::gate::SecurityGate::new(
        sandbox_manager,
    ));

    // Build SkillRouter with configurable thresholds from daemon config
    let skill_router = {
        let sd = &daemon_config.load().execution.skill_defaults;
        Arc::new(
            openalpaca_core::orchestrator::skill_router::SkillRouter::new_with_bus(
                sd.router_auto_select_threshold,
                sd.router_suggest_threshold,
                bus.clone(),
            ),
        )
    };

    Ok(InitializedServices {
        shared_context,
        llm_router,
        llm_settings_service,
        token_manager,
        provider_usage_tracker,
        embedder,
        agent_config_service,
        tool_registry,
        security_gate,
        skill_catalog,
        skill_router,
        secret_store,
        web_search_config,
        connector_send_lock,
        mcp_supervisor,
    })
}

/// Enforce §5.4's cross-session cap, once, at boot.
///
/// > `log_max_total_bytes` — 2 GB — Across all sessions. Evict oldest-touched
/// > **archived** sessions' logs first, LRU; an active session's log is never
/// > evicted.
///
/// The active set comes from the database, so "archived" means what the
/// session rows say it means, and an archived session gives up its whole log —
/// `results/`, rotated segments, then the live segment (R54). The walk is
/// filesystem work on a directory that may hold thousands of files, so it goes
/// to a blocking thread rather than stalling the boot runtime; a failure is a
/// warning, never a boot failure — the daemon runs with a log that is over its
/// cap rather than not at all.
///
/// Returns the report so the [`SessionLogService`] can carry it: a root that is
/// **still** over its cap after a full pass is a fact a status route must be
/// able to state, not one that lives only in this function's log line.
async fn sweep_session_logs(
    root: &Path,
    db: &Database,
    daemon_config: &Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
) -> Option<openalpaca_core::session_log::sweep::SweepReport> {
    let max_total = daemon_config.load().orchestrator.sessions.log_max_total_bytes;
    let active: std::collections::HashSet<String> =
        match openalpaca_storage::ConversationRepository::new(db).active_session_ids() {
            Ok(ids) => ids.into_iter().collect(),
            Err(e) => {
                // Without the protected set the sweep could evict a live
                // session's log, which §5.4 forbids outright. Skipping is the
                // only safe answer.
                tracing::warn!("Session log sweep skipped — active sessions unreadable: {e}");
                return None;
            }
        };

    let root = root.to_path_buf();
    // The database goes with it: an eviction that takes a session's live
    // segment de-indexes that session's `tool_execution_log` rows first
    // (write-first, T42 re-review Minor 2), and the walk is blocking work
    // either way.
    let db = db.clone();
    let swept = tokio::task::spawn_blocking(move || {
        openalpaca_core::session_log::sweep::enforce_total_cap(&root, max_total, &active, Some(&db))
    })
    .await;

    let report = match swept {
        Ok(Ok(report)) => report,
        Ok(Err(e)) => {
            tracing::warn!("Session log sweep failed: {e}");
            return None;
        }
        Err(e) => {
            tracing::warn!("Session log sweep task failed: {e}");
            return None;
        }
    };

    if report.files_removed == 0 && !report.over_cap_after {
        tracing::debug!(
            sessions = report.sessions_visited,
            bytes = report.bytes_before,
            max_total,
            "Session logs are within their total cap"
        );
    } else if report.over_cap_after {
        // Everything left over the cap is protected — an active session,
        // `snapshots/`, or a name this store did not create. Nothing further
        // can be done at boot, so it is said once, loudly.
        tracing::warn!(
            sessions_visited = report.sessions_visited,
            sessions_evicted = report.sessions_evicted,
            files_removed = report.files_removed,
            bytes_freed = report.bytes_freed,
            bytes_after = report.bytes_after,
            max_total,
            "Session logs are still over their total cap — only protected bytes remain"
        );
    } else {
        tracing::info!(
            sessions_visited = report.sessions_visited,
            sessions_evicted = report.sessions_evicted,
            files_removed = report.files_removed,
            index_rows_cleared = report.index_rows_cleared,
            bytes_freed = report.bytes_freed,
            bytes_before = report.bytes_before,
            bytes_after = report.bytes_after,
            max_total,
            "Session log sweep completed"
        );
    }
    Some(report)
}
