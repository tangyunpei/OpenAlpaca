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
mod routes;

use ::tokio::sync::mpsc;
use anyhow::{Context, Result};
use axum::{
    Router,
    extract::State,
    response::Json,
    routing::{get, post},
};
use events::EventBroadcaster;
use openalpaca_core::{
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
    if config_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&config_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "toml") {
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
    }

    let lane_manager = Arc::new(LaneManager::new());

    // Step 5.2.2: Load LLM config (Phase 5.1)
    let llm_provider: Option<Arc<dyn openalpaca_llm::LlmProvider>> = {
        let llm_config_path = std::env::current_dir()
            .unwrap_or_default()
            .join("config/llm.toml");
        if llm_config_path.exists() {
            match openalpaca_llm::LlmConfig::from_file(&llm_config_path) {
                Ok(config) => match openalpaca_llm::build_provider(&config) {
                    Ok(provider) => {
                        info!("LLM provider loaded: {}", provider.name());
                        Some(Arc::from(provider))
                    }
                    Err(e) => {
                        warn!("Failed to build LLM provider: {e}. Falling back to echo stub.");
                        None
                    }
                },
                Err(e) => {
                    warn!("Failed to load LLM config: {e}. Falling back to echo stub.");
                    None
                }
            }
        } else {
            info!("No config/llm.toml found. Using echo stub.");
            None
        }
    };

    // Construct Orchestrator as the new message handler
    #[allow(deprecated)]
    let bus = ctx.bus.clone();
    let orchestrator = Arc::new(Orchestrator::new(
        shared_context.clone(),
        lane_manager.clone(),
        bus.clone(),
        SystemPersona::default(),
        llm_provider,
        openalpaca_core::runner::LoopConfig::default(),
    ));
    let handler = Arc::new(gateway_bridge::OrchestratorHandler::new(orchestrator));
    let gateway = Arc::new(Gateway::new(
        shared_context,
        lane_manager,
        handler,
        bus.clone(),
    ));

    // Step 5.3: Connector Lifecycle (Phase 4.1.8)
    let connector_manager = managers::connector::ConnectorManager::new(
        db.clone(),
        bus,
        gateway.clone(),
    );
    connector_manager.start_all().await;

    let state = Arc::new(AppState {
        instance_id: instance_id.clone(),
        token,
        event_broadcaster,
        db,
        shutdown_tx,
        core_ctx: ctx,
        connector_manager,
        gateway,
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
        .route("/v1/agents/{id}", get(routes::get_agent_handler))
        .route(
            "/v1/agents/{id}/action",
            post(routes::agent_action_handler),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::auth_middleware,
        ));

    // WebSocket routes (token validated in handler via query param)
    let websocket = Router::new().route("/v1/events", get(routes::events_handler));

    // Merge all routes
    let app = public
        .merge(protected_routes)
        .merge(websocket)
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
