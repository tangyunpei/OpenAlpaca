//! OpenAlpaca Daemon (openalpacad)
//!
//! Background service that provides:
//! - Singleton instance management (only one daemon per user)
//! - Dynamic port binding (OS-assigned port)
//! - Discovery file for GUI/CLI to connect
//! - HTTP API for health checks and commands
//! - WebSocket for real-time event streaming

mod events;
mod gateway_bridge;
mod managers;
mod middleware;
mod notification;
mod routes;

use ::tokio::sync::mpsc;
use anyhow::{Context, Result};
use axum::{
    Router,
    extract::State,
    response::Json,
    routing::{delete, get, post, put},
};
use events::EventBroadcaster;
use openalpaca_core::{
    agent::AgentConfigService,
    bus::EventBus,
    chat::{ChatService, ChatStreamManager},
    context::SharedContext,
    gateway::Gateway,
    lane::LaneManager,
    middleware::prompt::SystemPersona,
    orchestrator::Orchestrator,
};
use openalpaca_storage::{ConfigRepository, ConversationRepository, Database, IdentityRepository, discovery, paths};
use openalpaca_wake::manager::WakeManager;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub instance_id: String,
    pub token: String,
    pub event_broadcaster: EventBroadcaster,
    pub db: Database,
    pub shutdown_tx: mpsc::Sender<()>,
    pub connector_manager: managers::connector::ConnectorManager,
    pub gateway: Arc<Gateway>,
    pub llm_settings_service: Option<Arc<openalpaca_llm::LlmSettingsService>>,
    pub agent_config_service: Option<Arc<AgentConfigService>>,
    pub chat_service: Option<Arc<ChatService>>,
    pub chat_stream_manager: Option<Arc<ChatStreamManager>>,
    pub token_manager: Option<Arc<openalpaca_llm::TokenManager>>,
    pub provider_usage_tracker: Option<Arc<openalpaca_llm::ProviderUsageTracker>>,
    pub local_user_id: String,
    pub default_lane_key: String,
}

/// Resolve the config base directory.
///
/// Priority order:
/// 1. `OPENALPACA_CONFIG_DIR` env var (explicit override, e.g. set by Tauri)
/// 2. Walk upward from `current_exe()` looking for a parent that contains `config/llm.toml`
///    (handles `target/debug/openalpacad` in dev builds)
/// 3. Walk upward from `current_dir()` looking for the same sentinel
/// 4. Fallback: `current_dir()/config`
fn resolve_config_base_dir() -> std::path::PathBuf {
    use std::path::{Path, PathBuf};

    // 1. Explicit env var override
    if let Ok(dir) = std::env::var("OPENALPACA_CONFIG_DIR") {
        let p = PathBuf::from(dir);
        if p.exists() {
            return p;
        }
        tracing::warn!(
            "OPENALPACA_CONFIG_DIR={} does not exist, ignoring",
            p.display()
        );
    }

    // Helper: walk up from `start` looking for a dir that contains config/llm.toml
    fn find_config_upward(start: &Path) -> Option<PathBuf> {
        let mut dir = start;
        loop {
            let candidate = dir.join("config");
            if candidate.join("llm.toml").exists() {
                return Some(candidate);
            }
            dir = dir.parent()?;
        }
    }

    // 2. Walk up from exe directory (handles target/debug/)
    if let Some(found) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().and_then(find_config_upward))
    {
        return found;
    }

    // 3. Walk up from CWD
    if let Some(found) = std::env::current_dir()
        .ok()
        .and_then(|p| find_config_upward(&p))
    {
        return found;
    }

    // 4. Last resort fallback
    std::env::current_dir().unwrap_or_default().join("config")
}

/// Resolve the stable local user ID from the database. On first run, if legacy
/// `gui_user:gui` messages exist, adopt `"gui_user"` to preserve history continuity.
/// Otherwise generate a UUID.
fn resolve_local_user_id(db: &Database) -> String {
    let config_repo = ConfigRepository::new(db);

    // Check if we already have a persisted local user ID
    if let Ok(Some(id)) = config_repo.get("identity.local_user_id") {
        return id;
    }

    // Check for legacy gui_user:gui history
    let conv_repo = ConversationRepository::new(db);
    let local_user_id = if conv_repo.count_by_lane("gui_user:gui").unwrap_or(0) > 0 {
        "gui_user".to_string() // Preserve existing history
    } else {
        uuid::Uuid::new_v4().to_string()
    };

    // Persist for future runs
    let _ = config_repo.set("identity.local_user_id", &local_user_id, "string");

    // Ensure global_user row exists
    let identity_repo = IdentityRepository::new(db);
    if identity_repo
        .get_global_user(&local_user_id)
        .unwrap_or(None)
        .is_none()
    {
        let display_name = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "Local User".to_string());
        let _ = identity_repo.create_global_user(&local_user_id, Some(&display_name));
    }

    local_user_id
}

fn main() -> Result<()> {
    // Initialize logging (before tokio, so resolve_config_base_dir() can use tracing)
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("OpenAlpaca Daemon starting...");

    // D3: Singleton lock FIRST — prevents all multi-process races.
    // Acquired before any config I/O or key generation.
    let _lock_guard = match discovery::acquire_single_instance_lock(false) {
        Ok(guard) => {
            info!("Acquired singleton lock");
            guard
        }
        Err(e) => {
            error!("Another daemon instance is already running: {e}");
            std::process::exit(1);
        }
    };

    // Resolve config dir and set master key BEFORE spawning any threads.
    // std::env::set_var is unsafe in multi-threaded contexts (Rust 2024 edition),
    // so we do it here in the single-threaded preamble.
    let config_base_dir = resolve_config_base_dir();
    info!("Config base dir: {}", config_base_dir.display());
    if !config_base_dir.join("llm.toml").exists() {
        warn!(
            "config/llm.toml not found under {}. LLM routing and summary generation will be disabled (echo stub).",
            config_base_dir.display()
        );
    }

    // D1: Master key always at app_dir (canonical, CWD-independent).
    let app_dir = paths::app_dir().context("Failed to get app dir")?;

    // Legacy migration: if config_base_dir/.master_key exists but app_dir/.master_key doesn't,
    // copy legacy key to app_dir using atomic create-new.
    let legacy_key_path = config_base_dir.join(".master_key");
    if legacy_key_path.exists()
        && !app_dir.join(".master_key").exists()
        && let Ok(hex) = std::fs::read_to_string(&legacy_key_path)
    {
        std::fs::create_dir_all(&app_dir).ok();
        // Atomic copy: if another process beat us, that's fine
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(app_dir.join(".master_key"))
        {
            use std::io::Write;
            let _ = f.write_all(hex.trim().as_bytes());
            let _ = f.flush();
            let _ = f.sync_all();
        }
        info!(
            "Migrated legacy master key from {} to {}",
            legacy_key_path.display(),
            app_dir.join(".master_key").display()
        );
    }

    // D6+D7: ensure_at is race-safe; on failure, fail fast.
    match openalpaca_llm::key_encryption::KeyEncryptor::ensure_at(&app_dir) {
        Ok(hex_key) => {
            // SAFETY: No other threads exist yet — tokio runtime has not started.
            unsafe { std::env::set_var("OPENALPACA_MASTER_KEY", &hex_key); }
            info!("Master key loaded from {}", app_dir.join(".master_key").display());
        }
        Err(e) => {
            error!("FATAL: Cannot ensure master key at {}: {e}", app_dir.display());
            std::process::exit(1);
        }
    }

    // Start the tokio runtime AFTER env vars are safely set.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to build tokio runtime")?
        .block_on(async_main(config_base_dir))
}

async fn async_main(config_base_dir: std::path::PathBuf) -> Result<()> {
    // Note: Singleton lock was already acquired in sync fn main() (D3).

    // Step 2: Bind to dynamic port (127.0.0.1:0 -> OS assigns port)
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("Failed to bind to localhost")?;
    let addr = listener.local_addr()?;
    let host = "127.0.0.1";
    let port = addr.port();
    info!("Listening on http://{host}:{port}");

    // Step 3: Generate discovery info and write atomically
    let instance_id = Uuid::new_v4().to_string();
    let disc =
        discovery::make_discovery(host, port, instance_id.clone(), env!("CARGO_PKG_VERSION"));
    let discovery_path = discovery::write_discovery_atomic(&disc)?;
    info!("Discovery written to: {}", discovery_path.display());

    // Extract token from discovery for sharing with AppState
    let token = disc.auth.token.clone();

    // Step 4: Initialize database
    let db_path = paths::database_path()?;
    let db = Database::open(&db_path).context("Failed to initialize database")?;
    info!("Database initialized: {}", db_path.display());

    // One-time migration: move conversation summaries from preference → conversations table
    migrate_preference_summaries(&db);

    // Step 4.1: Resolve stable local user ID
    let local_user_id = resolve_local_user_id(&db);
    let default_lane_key = format!("{local_user_id}:gui");
    info!("Local user ID: {local_user_id}, default lane: {default_lane_key}");

    // Step 5: Create event broadcaster for WebSocket streaming
    let event_broadcaster = EventBroadcaster::new(64, instance_id.clone(), Some(db.clone()));

    // Step 5.1.1: Initialize WakeManager
    let (wake_tx, mut wake_rx) = mpsc::channel(256);
    let mut wake_manager = WakeManager::new(wake_tx)
        .await
        .context("Failed to init WakeManager")?;

    // Register filesystem watchers for specific config paths BEFORE start (D4, D9)
    // Watch config/agents/ for agent config hot-reload, and config/llm.toml for LLM config changes.
    // We do NOT watch all of config/ to avoid persisting noisy events for .DS_Store, .master_key, etc.
    let mut watch_paths = Vec::new();
    let agents_dir = config_base_dir.join("agents");
    if agents_dir.exists() {
        watch_paths.push(agents_dir.clone());
    }
    let llm_config = config_base_dir.join("llm.toml");
    if llm_config.exists() {
        watch_paths.push(llm_config);
    }
    if !watch_paths.is_empty() {
        info!("Wake: watching paths: {:?}", watch_paths);
        wake_manager.add_filesystem_watcher(watch_paths);
    }

    // Start WakeManager (starts scheduler + watchers)
    wake_manager
        .start()
        .await
        .context("Failed to start WakeManager")?;

    // Spawn forwarding task: WakeEvent -> ServerEvent::Wake -> Broadcast & Persist
    let eb_clone = event_broadcaster.clone();
    tokio::spawn(async move {
        while let Some(event) = wake_rx.recv().await {
            info!("Received WakeEvent: {:?}", event);
            eb_clone.wake(event);
        }
    });

    // Step 6: Build HTTP router with public/protected/websocket split

    // Shutdown channel for API-triggered shutdown
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);

    // Single EventBus for system-wide event distribution
    let bus = EventBus::default();

    // Spawn bridge: SystemEvent (Core) -> ServerEvent (API)
    let eb_bridge = event_broadcaster.clone();
    let mut system_rx = bus.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = system_rx.recv().await {
            match event {
                openalpaca_core::events::SystemEvent::ConnectorStatus { id, status, .. } => {
                    eb_bridge.connector_status(&id, &status);
                }
                openalpaca_core::events::SystemEvent::TaskCreated {
                    task_id, title, created_by: _, ..
                } => {
                    eb_bridge.task_status(&task_id, &title, "queued", None, None, None);
                }
                openalpaca_core::events::SystemEvent::TaskUpdated {
                    task_id,
                    status,
                    progress_current,
                    progress_total,
                    ..
                } => {
                    eb_bridge.task_status(&task_id, "", &status, progress_current, progress_total, None);
                }
                openalpaca_core::events::SystemEvent::TaskCompleted {
                    task_id,
                    result_summary,
                    ..
                } => {
                    eb_bridge.task_status(&task_id, "", "completed", None, None, result_summary);
                }
                openalpaca_core::events::SystemEvent::TaskFailed {
                    task_id, error, ..
                } => {
                    eb_bridge.task_status(&task_id, "", "failed", None, None, Some(error));
                }
                openalpaca_core::events::SystemEvent::AgentRegistered {
                    agent_id, name, ..
                } => {
                    eb_bridge.agent_status(&agent_id, &name, "idle", None);
                }
                openalpaca_core::events::SystemEvent::AgentStatusChanged {
                    agent_id, status, current_task_id, ..
                } => {
                    eb_bridge.agent_status(&agent_id, "", &status, current_task_id);
                }
                openalpaca_core::events::SystemEvent::SecurityViolation {
                    agent_id, tool_name, reason, ..
                } => {
                    tracing::warn!(
                        "Security violation: agent={}, tool={}, reason={}",
                        agent_id, tool_name, reason
                    );
                }
                openalpaca_core::events::SystemEvent::ToolExecuted {
                    agent_id, tool_name, success, duration_ms, ..
                } => {
                    tracing::debug!(
                        "Tool executed: agent={}, tool={}, success={}, duration={}ms",
                        agent_id, tool_name, success, duration_ms
                    );
                }
                openalpaca_core::events::SystemEvent::LlmCallCompleted {
                    agent_id, model, input_tokens, output_tokens, cost_usd, ..
                } => {
                    tracing::info!(
                        "LLM call: agent={}, model={}, tokens={}/{}, cost=${:.6}",
                        agent_id, model, input_tokens, output_tokens, cost_usd
                    );
                }
                openalpaca_core::events::SystemEvent::ModelAccessDenied {
                    agent_id, model_id, reason, ..
                } => {
                    tracing::warn!(
                        "Model access denied: agent={}, model={}, reason={}",
                        agent_id, model_id, reason
                    );
                }
                // Wake events are forwarded via the dedicated wake_rx channel (lines 130-135),
                // not through the Core EventBus. If a future Core component publishes
                // SystemEvent::Wake to the bus, add an explicit arm here — but remove
                // the wake_rx pipeline first to avoid double-broadcast.
                _ => {}
            }
        }
    });

    // Step 5.2: Gateway Construction (Phase 4.3)
    let shared_context = Arc::new(SharedContext::new());

    // Step 5.2.1: Load agent configs from TOML files (Phase 4.5)
    let config_dir = config_base_dir.join("agents");
    if config_dir.exists()
        && let Ok(entries) = std::fs::read_dir(&config_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        match toml::from_str::<openalpaca_core::agent::AgentConfigFile>(&content)
                        {
                            Ok(agent_config) => {
                                // Register in-memory
                                let subagent = agent_config.clone().into_subagent();
                                shared_context.agent_registry.register(subagent);

                                // Persist to DB
                                let storage_config = agent_config.into_storage_config();
                                let agent_id = storage_config.id.clone();
                                let repo =
                                    openalpaca_storage::SubAgentRepository::new(&db);
                                let _ = repo.upsert(&storage_config);

                                // Initialize metrics row if not exists
                                if let Ok(None) = repo.get_metrics(&agent_id) {
                                    let _ = repo.upsert_metrics(
                                        &openalpaca_storage::AgentMetrics::new_empty(
                                            &agent_id,
                                        ),
                                    );
                                }

                                info!("Loaded agent config: {}", path.display());
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to parse agent config {}: {}",
                                    path.display(),
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to read agent config {}: {}", path.display(), e);
                    }
                }
            }
        }
    }

    let lane_manager = Arc::new(LaneManager::new());

    // Step 5.2.2: Initialize OS secret store + auto-migrate secrets
    let secret_store: Arc<dyn openalpaca_llm::SecretStore> =
        Arc::new(openalpaca_llm::KeyringSecretStore);

    let llm_config_path = config_base_dir.join("llm.toml");

    // Auto-migrate secret_encrypted → OS keychain (Step 5)
    if llm_config_path.exists() {
        match openalpaca_llm::migrate_llm_secrets(&llm_config_path, &*secret_store) {
            Ok(0) => {}
            Ok(n) => info!("Migrated {n} secret(s) to OS keychain"),
            Err(e) => warn!("Secret migration failed: {e}. Legacy secrets will still work."),
        }
    }

    // Step 5.2.2b: Load LLM config (Phase 5.1 → 5.2.5 LlmRouter)
    // Note: OPENALPACA_MASTER_KEY was set in the sync main() preamble before tokio started.
    let llm_router: Option<Arc<openalpaca_llm::LlmRouter>> = {
        if llm_config_path.exists() {
            match openalpaca_llm::build_router_with_secret_store(&llm_config_path, Some(&*secret_store)) {
                Ok(router) => {
                    info!(
                        "LLM router loaded (default model: {})",
                        router.default_model()
                    );
                    Some(Arc::new(router))
                }
                Err(e) => {
                    warn!("Failed to build LLM router: {e}. Falling back to echo stub.");
                    None
                }
            }
        } else {
            info!("No config/llm.toml found. Using echo stub.");
            None
        }
    };

    // Step 5.2.3: Build LLM Settings Service (Phase 5.5)
    let llm_settings_service: Option<Arc<openalpaca_llm::LlmSettingsService>> =
        if let Some(ref router) = llm_router {
            match openalpaca_llm::LlmSettingsService::new_with_secret_store(
                router.clone(),
                llm_config_path.clone(),
                secret_store.clone(),
            ) {
                Ok(service) => {
                    info!("LLM settings service initialized");
                    Some(Arc::new(service))
                }
                Err(e) => {
                    warn!("Failed to init LLM settings service: {e}");
                    None
                }
            }
        } else {
            None
        };

    // Step 5.2.3b: Refresh models from provider APIs at startup
    if let Some(ref service) = llm_settings_service {
        info!("Refreshing available models from providers...");
        service.refresh_models().await;
    }

    // Step 5.2.3c: Credential Discovery & Token Manager
    let cancel_token = tokio_util::sync::CancellationToken::new();

    let llm_config: Option<openalpaca_llm::LlmRouterConfig> = {
        let p = config_base_dir.join("llm.toml");
        if p.exists() {
            openalpaca_llm::read_config(&p).ok()
        } else {
            None
        }
    };

    let cred_config = llm_config.as_ref()
        .and_then(|c| c.credential_discovery.clone())
        .unwrap_or_default();

    let token_manager: Option<Arc<openalpaca_llm::TokenManager>> =
        if cred_config.claude_code.unwrap_or(true) || cred_config.codex.unwrap_or(true) {
            let tm = Arc::new(openalpaca_llm::TokenManager::new(cred_config.clone()).await);
            if let (Some(router), Some(svc)) = (&llm_router, &llm_settings_service) {
                tm.rescan(svc, router).await;
            }
            if let (Some(router), Some(svc)) = (&llm_router, &llm_settings_service) {
                let _refresh_handle = tm.start_refresh_loop(
                    svc.clone(),
                    router.clone(),
                    cancel_token.clone(),
                );
            }
            info!("TokenManager initialized with credential discovery");
            Some(tm)
        } else {
            None
        };

    // Step 5.2.3d: Provider Usage Tracker
    let provider_usage_tracker: Option<Arc<openalpaca_llm::ProviderUsageTracker>> =
        if cred_config.fetch_external_usage.unwrap_or(false) {
            info!("Provider usage tracker enabled");
            Some(Arc::new(openalpaca_llm::ProviderUsageTracker::new()))
        } else {
            None
        };

    // Step 5.2.4: Build AgentConfigService (Phase 5.7)
    let agent_config_service = Arc::new(AgentConfigService::new(
        shared_context.agent_registry.clone(),
        config_dir.clone(),
        db.clone(),
    ));

    // Build ToolRegistry with built-in tools + user-defined tools
    let mut tool_registry = openalpaca_core::tools::ToolRegistry::new();

    // Register built-in tools
    for tool in openalpaca_core::tools::builtins::builtin_tools(Some(db.clone())) {
        tool_registry.register(tool);
    }

    // Load user tools from config/tools/*.toml (D11: use resolved config_base_dir)
    let tools_config_dir = config_base_dir.join("tools");
    for tool in openalpaca_core::tools::config::load_tools_from_dir(&tools_config_dir) {
        info!("Registered custom tool: {}", tool.definition.name);
        tool_registry.register(tool);
    }
    info!("Tool registry: {} tools loaded", tool_registry.count());

    let tool_registry = Arc::new(tool_registry);

    // Construct SecurityGate → SandboxManager → RegistryToolExecutor chain
    let registry_executor = Arc::new(
        openalpaca_core::tools::RegistryToolExecutor::new(tool_registry.clone()),
    );
    let sandbox_manager = Arc::new(
        openalpaca_core::security::sandbox::SandboxManager::new(registry_executor, bus.clone()),
    );
    let security_gate = Arc::new(
        openalpaca_core::security::gate::SecurityGate::new(sandbox_manager),
    );

    // Construct Orchestrator as the new message handler
    let orchestrator = Arc::new(Orchestrator::new(
        shared_context.clone(),
        lane_manager.clone(),
        bus.clone(),
        SystemPersona::default(),
        llm_router,
        openalpaca_core::runner::LoopConfig::default(),
        security_gate,
        tool_registry,
        Some(db.clone()),
    ));
    let handler = Arc::new(gateway_bridge::OrchestratorHandler::new(orchestrator));
    let gateway = Arc::new(Gateway::new(
        shared_context,
        lane_manager,
        handler,
        bus.clone(),
        Some(db.clone()),
    ));

    // Step 5.3: Connector Lifecycle (Phase 4.1.8)
    let notif_bus = bus.clone();
    let connector_manager = managers::connector::ConnectorManager::new(
        db.clone(),
        bus,
        gateway.clone(),
    );
    connector_manager.start_all().await;

    // Step 5.3.1: Spawn NotificationDispatcher (Phase 5.6)
    {
        let config_repo = openalpaca_storage::ConfigRepository::new(&db);
        let telegram_bot = config_repo
            .get("telegram.token")
            .ok()
            .flatten()
            .map(teloxide::Bot::new);
        let notif_rx = notif_bus.subscribe();
        let dispatcher = notification::NotificationDispatcher::new(notif_rx, telegram_bot, db.clone());
        tokio::spawn(dispatcher.run());
    }

    // Step 5.4: Build ChatService (Phase 5.6)
    let chat_stream_manager = Arc::new(ChatStreamManager::new());
    let chat_service = Arc::new(ChatService::new(
        gateway.clone(),
        chat_stream_manager.clone(),
        db.clone(),
    ));

    let state = Arc::new(AppState {
        instance_id: instance_id.clone(),
        token,
        event_broadcaster,
        db,
        shutdown_tx,
        connector_manager,
        gateway,
        llm_settings_service,
        agent_config_service: Some(agent_config_service),
        chat_service: Some(chat_service),
        chat_stream_manager: Some(chat_stream_manager.clone()),
        token_manager,
        provider_usage_tracker,
        local_user_id,
        default_lane_key,
    });

    // Public routes (no auth required)
    let public = Router::new()
        .route("/", get(root_handler))
        .route("/v1/health", get(health_handler));

    // Protected routes (require token)
    let protected_routes = Router::new()
        .route("/v1/command", post(routes::command_handler))
        .route("/v1/events/history", get(routes::events_history_handler))
        .route("/v1/connectors", get(routes::list_connectors_handler))
        .route(
            "/v1/connectors/{id}/action",
            post(routes::connector_action_handler),
        )
        .route(
            "/v1/connectors/{id}/config",
            post(routes::connector_config_handler),
        )
        .route("/v1/auth/link", post(routes::generate_link_token_handler))
        .route("/v1/tasks", post(routes::create_task_handler))
        .route("/v1/tasks", get(routes::list_tasks_handler))
        .route("/v1/tasks/{id}", get(routes::get_task_handler))
        .route("/v1/tasks/{id}/action", post(routes::task_action_handler))
        // Preferences routes (Phase 5)
        .route("/v1/preferences", get(routes::list_preferences_handler))
        .route("/v1/preferences/{key}", get(routes::get_preference_handler))
        .route("/v1/preferences/{key}", put(routes::set_preference_handler))
        .route("/v1/preferences/{key}", delete(routes::delete_preference_handler))
        .route("/v1/agents", get(routes::list_agents_handler))
        .route("/v1/agents", post(routes::create_agent_handler))
        .route("/v1/agents/from-toml", post(routes::create_agent_from_toml_handler))
        .route("/v1/agents/from-chat", post(routes::create_agent_from_chat_handler))
        .route("/v1/agents/{id}", get(routes::get_agent_handler))
        .route("/v1/agents/{id}", delete(routes::delete_agent_handler))
        .route("/v1/agents/{id}/config", get(routes::get_agent_config_handler))
        .route("/v1/agents/{id}/config", put(routes::update_agent_config_handler))
        .route(
            "/v1/agents/{id}/action",
            post(routes::agent_action_handler),
        )
        // Chat routes (Phase 5.6)
        .route("/v1/chat", post(routes::send_chat_handler))
        .route("/v1/chat/history", get(routes::get_chat_history_handler))
        .route("/v1/chat/history", delete(routes::delete_chat_history_handler))
        // Cross-platform conversation API (Phase 5.6)
        .route("/v1/conversations", get(routes::list_conversations_handler))
        .route("/v1/conversations/{id}/messages", get(routes::get_conversation_messages_handler))
        // Settings routes (Phase 5.5)
        .route("/v1/settings/llm", get(routes::get_llm_settings))
        .route("/v1/settings/llm", put(routes::upsert_key))
        .route(
            "/v1/settings/llm/keys/{provider}/{key_id}",
            delete(routes::delete_key),
        )
        .route(
            "/v1/settings/llm/keys/reorder",
            put(routes::reorder_keys),
        )
        .route(
            "/v1/settings/llm/keys/priority",
            put(routes::set_key_priority),
        )
        .route("/v1/settings/llm/validate", post(routes::validate_key))
        .route("/v1/settings/llm/status", get(routes::get_key_status))
        // LLM Usage routes (Phase 5.5.5)
        .route("/v1/llm/usage", get(routes::get_llm_usage))
        .route("/v1/llm/usage/daily", get(routes::get_llm_usage_daily))
        // Pricing routes
        .route("/v1/llm/pricing", get(routes::get_llm_pricing))
        .route("/v1/llm/pricing/estimate", get(routes::estimate_cost))
        // Model discovery routes
        .route("/v1/models", get(routes::list_models))
        .route("/v1/models/refresh", post(routes::refresh_models))
        // Credential discovery routes
        .route("/v1/settings/llm/credentials", get(routes::get_discovered_credentials))
        .route("/v1/settings/llm/credentials/rescan", post(routes::rescan_credentials))
        // CLI backend routes
        .route("/v1/settings/llm/cli-backends", get(routes::get_cli_backends))
        // Provider usage routes
        .route("/v1/settings/llm/providers/usage", get(routes::get_provider_usage))
        // Orchestrator config routes (Phase 5.7)
        .route("/v1/orchestrator/config", get(routes::get_orchestrator_config))
        .route("/v1/orchestrator/config", put(routes::update_orchestrator_config))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::auth_middleware,
        ));

    // WebSocket routes (token validated in handler via query param)
    let websocket = Router::new().route("/v1/events", get(routes::events_handler));

    // SSE chat stream route (token validated inline via query param)
    let chat_sse = Router::new().route(
        "/v1/chat/stream/{stream_id}",
        get(routes::chat_stream_handler),
    );

    // Merge all routes
    let app = public
        .merge(protected_routes)
        .merge(websocket)
        .merge(chat_sse)
        .with_state(state.clone())
        .layer(CorsLayer::permissive());

    // Step 6: Spawn daemon-level heartbeat task
    let heartbeat_state = state.clone();
    let _heartbeat_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            heartbeat_state.event_broadcaster.heartbeat();
        }
    });

    // Step 6.1: Spawn chat stream cleanup task (Phase 5.6)
    let cleanup_csm = chat_stream_manager;
    let _chat_cleanup_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            cleanup_csm.cleanup_stale(std::time::Duration::from_secs(30));
        }
    });

    // Step 7: Run server with graceful shutdown
    info!("Daemon ready (instance: {instance_id})");

    let server = axum::serve(listener, app);

    // Handle graceful shutdown on SIGINT/SIGTERM
    tokio::select! {
        result = server => {
            if let Err(e) = result {
                error!("Server error: {e}");
            }
        }
        _ = shutdown_signal() => {
            info!("Shutdown signal received (OS)");
            cancel_token.cancel();
        }
        _ = shutdown_rx.recv() => {
            info!("Shutdown signal received (API)");
            cancel_token.cancel();
        }
    }

    // Shutdown WakeManager (stops watchers + scheduler)
    if let Err(e) = wake_manager.shutdown().await {
        warn!("Failed to shut down WakeManager: {e}");
    }

    // Cleanup: remove discovery file
    info!("Cleaning up...");
    if let Err(e) = discovery::remove_discovery() {
        warn!("Failed to remove discovery file: {e}");
    }

    info!("Daemon stopped");
    Ok(())
}

/// Health check endpoint (includes instance_id for validation)
/// No token required - minimal info only
async fn health_handler(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "instance_id": state.instance_id
    }))
}

/// Root endpoint (basic info)
async fn root_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "name": "OpenAlpaca Daemon",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

/// Migrate conversation summaries from the preference table to the conversations table.
/// Idempotent: runs every startup but does nothing if no preference rows remain.
/// Non-fatal: failure is logged but doesn't prevent daemon startup.
fn migrate_preference_summaries(db: &Database) {
    if let Err(e) = db.with_connection(|conn| {
        // Find all preference rows with conversation_summary
        let mut stmt = conn.prepare(
            "SELECT user_id, value, version FROM preference WHERE key = 'conversation_summary'"
        )?;
        let rows: Vec<(String, String, i64)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .filter_map(Result::ok)
            .collect();

        if rows.is_empty() { return Ok(()); }

        let tx = conn.unchecked_transaction()?;
        let mut migrated = 0usize;
        for (lane_key, value, pref_version) in &rows {
            let parsed: serde_json::Value = match serde_json::from_str(value) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let summary = parsed.get("summary").and_then(|s| s.as_str()).unwrap_or("");
            let last_id = parsed.get("last_summarized_message_id").and_then(|n| n.as_i64()).unwrap_or(0);

            // Only update if conversation row exists
            let updated = tx.execute(
                "UPDATE conversations SET summary = ?1, summary_version = ?2,
                 last_summarized_message_id = ?3, summary_updated_at = datetime('now')
                 WHERE lane_key = ?4",
                (summary, pref_version, last_id, lane_key.as_str()),
            )?;

            if updated > 0 {
                tx.execute(
                    "DELETE FROM preference WHERE user_id = ?1 AND key = 'conversation_summary'",
                    [lane_key],
                )?;
                migrated += 1;
            }
        }
        tx.commit()?;
        if migrated > 0 {
            tracing::info!("Migrated {migrated} conversation summaries from preference -> conversations");
        }
        Ok(())
    }) {
        tracing::warn!("Summary migration failed (non-fatal): {e}");
    }
}

/// Wait for shutdown signals (SIGINT or SIGTERM)
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
