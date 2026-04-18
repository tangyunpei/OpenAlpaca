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
    /// Holder for connected MCP clients. Owns Arc handles for daemon lifetime.
    pub mcp_client_set: openalpaca_core::tools::mcp::McpClientSet,
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
) -> Result<InitializedServices> {
    let shared_context = Arc::new(SharedContext::new());

    // Load agent templates from .md files + legacy .toml files
    agents::load_agent_templates(config_base_dir, db, &shared_context)?;

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

    // Build ToolRegistry
    let (tool_registry, connector_send_lock, mcp_client_set) = tools::build_tool_registry(
        config_base_dir,
        db,
        &embedder,
        soul_path,
        user_path,
        identity_path,
        bus,
        daemon_config,
        &web_search_config,
    )
    .await?;

    // Build security chain
    let sandbox_manager = Arc::new(openalpaca_core::security::sandbox::SandboxManager::new(
        tool_registry.clone(),
        bus.clone(),
        &daemon_config.load().security.circuit_breaker,
    ));
    let security_gate = Arc::new(openalpaca_core::security::gate::SecurityGate::new(
        sandbox_manager,
    ));

    // Build SkillCatalog
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
        mcp_client_set,
    })
}
