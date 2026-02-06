//! OpenAlpaca Daemon (openalpacad)
//!
//! Background service that provides:
//! - Singleton instance management (only one daemon per user)
//! - Dynamic port binding (OS-assigned port)
//! - Discovery file for GUI/CLI to connect
//! - HTTP API for health checks and commands
//! - WebSocket for real-time event streaming

mod core_ctx;
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
    chat::{ChatService, ChatStreamManager},
    context::SharedContext,
    gateway::Gateway,
    lane::LaneManager,
    middleware::prompt::SystemPersona,
    orchestrator::Orchestrator,
};
use openalpaca_storage::{Database, discovery, paths};
use openalpaca_wake::manager::WakeManager;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

/// Shared application state
#[allow(deprecated)]
#[derive(Clone)]
pub struct AppState {
    pub instance_id: String,
    pub token: String,
    pub event_broadcaster: EventBroadcaster,
    pub db: Database,
    pub shutdown_tx: mpsc::Sender<()>,
    pub core_ctx: core_ctx::CoreCtx,
    pub connector_manager: managers::connector::ConnectorManager,
    pub gateway: Arc<Gateway>,
    pub llm_settings_service: Option<Arc<openalpaca_llm::LlmSettingsService>>,
    pub agent_config_service: Option<Arc<AgentConfigService>>,
    pub chat_service: Option<Arc<ChatService>>,
    pub chat_stream_manager: Option<Arc<ChatStreamManager>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("OpenAlpaca Daemon starting...");

    // Step 1: Acquire singleton lock (non-blocking)
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

    // Step 5: Create event broadcaster for WebSocket streaming
    let event_broadcaster = EventBroadcaster::new(64, instance_id.clone(), Some(db.clone()));

    // Step 5.1: Initialize WakeManager and integration
    let (wake_tx, mut wake_rx) = mpsc::channel(256);
    let wake_manager = WakeManager::new(wake_tx)
        .await
        .context("Failed to init WakeManager")?;

    // Start WakeManager (spawns internal scheduler/watchers)
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

    // Create CoreCtx early so we can share the EventBus with connectors
    #[allow(deprecated)]
    let ctx = core_ctx::CoreCtx::new();

    // Spawn bridge: SystemEvent (Core) -> ServerEvent (API)
    let eb_bridge = event_broadcaster.clone();
    #[allow(deprecated)]
    let mut core_rx = ctx.bus.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = core_rx.recv().await {
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
                    tracing::debug!(
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
                _ => {}
            }
        }
    });

    // Step 5.2: Gateway Construction (Phase 4.3)
    let shared_context = Arc::new(SharedContext::new());

    // Step 5.2.1: Load agent configs from TOML files (Phase 4.5)
    let config_dir = std::env::current_dir()
        .unwrap_or_default()
        .join("config/agents");
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

    // Step 5.2.2: Load LLM config (Phase 5.1 → 5.2.5 LlmRouter)
    let llm_router: Option<Arc<openalpaca_llm::LlmRouter>> = {
        let llm_config_path = std::env::current_dir()
            .unwrap_or_default()
            .join("config/llm.toml");
        if llm_config_path.exists() {
            match openalpaca_llm::build_router(&llm_config_path) {
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
    let llm_config_path = std::env::current_dir()
        .unwrap_or_default()
        .join("config/llm.toml");
    let llm_settings_service: Option<Arc<openalpaca_llm::LlmSettingsService>> =
        if let Some(ref router) = llm_router {
            match openalpaca_llm::LlmSettingsService::new(
                router.clone(),
                llm_config_path,
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

    // Step 5.2.4: Build AgentConfigService (Phase 5.7)
    let agent_config_service = Arc::new(AgentConfigService::new(
        shared_context.agent_registry.clone(),
        config_dir.clone(),
        db.clone(),
    ));

    // Construct SecurityGate → SandboxManager → StubToolExecutor chain
    #[allow(deprecated)]
    let bus = ctx.bus.clone();
    let stub_executor = std::sync::Arc::new(openalpaca_core::runner::StubToolExecutor);
    let sandbox_manager = std::sync::Arc::new(
        openalpaca_core::security::sandbox::SandboxManager::new(stub_executor, bus.clone()),
    );
    let security_gate = std::sync::Arc::new(
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
        core_ctx: ctx,
        connector_manager,
        gateway,
        llm_settings_service,
        agent_config_service: Some(agent_config_service),
        chat_service: Some(chat_service),
        chat_stream_manager: Some(chat_stream_manager.clone()),
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
        .route("/v1/settings/llm/validate", post(routes::validate_key))
        .route("/v1/settings/llm/status", get(routes::get_key_status))
        // Model discovery routes
        .route("/v1/models", get(routes::list_models))
        .route("/v1/models/refresh", post(routes::refresh_models))
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
        }
        _ = shutdown_rx.recv() => {
            info!("Shutdown signal received (API)");
        }
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
