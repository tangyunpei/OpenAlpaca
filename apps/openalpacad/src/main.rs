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
mod middleware;
mod routes;

use ::tokio::sync::mpsc;
use anyhow::{Context, Result};
use axum::{
    Router,
    extract::State,
    middleware::from_fn_with_state,
    response::Json,
    routing::{get, post},
};
use events::EventBroadcaster;
use openalpaca_connectors::{Connector, ConnectorBuilder};
use openalpaca_storage::{Database, discovery, paths};
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
    pub core_ctx: core_ctx::CoreCtx,
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
    let core_ctx = core_ctx::CoreCtx::new();

    // Step 5.2: Connector Lifecycle (Phase 4.1.5)
    // Auto-spawn connectors if tokens are present in environment
    if let Ok(token) = std::env::var("OPENALPACA_TELEGRAM_TOKEN") {
        info!("Found OPENALPACA_TELEGRAM_TOKEN, spawning Telegram Connector...");

        let db_clone = db.clone();
        let bus = core_ctx.bus.clone();

        tokio::spawn(async move {
            let builder = ConnectorBuilder::new(db_clone, bus);
            let connector = builder.telegram(token);

            info!("Telegram Connector initialized via Auto-Discovery");
            // Run the connector (blocking loop)
            if let Err(e) = connector.run().await {
                error!("Telegram Connector crashed: {e}");
            }
        });
    }

    let state = Arc::new(AppState {
        instance_id: instance_id.clone(),
        token,
        event_broadcaster,
        db,
        shutdown_tx,
        core_ctx,
    });

    // Public routes (no auth required)
    let public = Router::new()
        .route("/", get(root_handler))
        .route("/v1/health", get(health_handler));

    // Protected routes (Bearer token required)
    let protected = Router::new()
        .route("/v1/command", post(routes::command_handler))
        .route("/v1/events/history", get(routes::events_history_handler))
        .layer(from_fn_with_state(
            state.clone(),
            middleware::auth_middleware,
        ));

    // WebSocket routes (token validated in handler via query param)
    let websocket = Router::new().route("/v1/events", get(routes::events_handler));

    // Merge all routes
    let app = public
        .merge(protected)
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
