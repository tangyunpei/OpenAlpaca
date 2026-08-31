//! OpenAlpaca Daemon (openalpacad)
//!
//! Background service that provides:
//! - Singleton instance management (only one daemon per user)
//! - Dynamic port binding (OS-assigned port)
//! - Discovery file for GUI/CLI to connect
//! - HTTP API for health checks and commands
//! - WebSocket for real-time event streaming

mod background;
mod bootstrap;
mod connector_bridge;
mod event_bridge;
mod events;
mod extraction;
mod followup;
mod gateway_bridge;
mod hot_reload;
mod managers;
mod middleware;
mod notification;
mod router;
mod routes;
mod scheduled_skills;
mod services;
mod shutdown;
mod state;

pub use state::AppState;

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use events::EventBroadcaster;
use openalpaca_core::{
    bus::EventBus,
    chat::{ChatService, ChatStreamManager},
    daemon_config::load_daemon_config,
    gateway::Gateway,
    lane::LaneManager,
    middleware::identity::identity_document_has_content,
    middleware::user::user_document_has_content,
    orchestrator::Orchestrator,
    runner::LoopConfig,
};
use openalpaca_storage::{Database, discovery, paths};
use openalpaca_wake::manager::WakeManager;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

fn main() -> Result<()> {
    // Initialize logging (before tokio, so resolve_config_base_dir() can use tracing)
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // Best-effort guardrail: surface terminal state that can make Ctrl+C appear
    // "broken" even when SIGINT handling is correct.
    shutdown::warn_if_ctrl_c_unavailable_on_tty();

    info!("OpenAlpaca Daemon starting...");

    // Migrate legacy app dir (com.openalpaca.OpenAlpaca → OpenAlpaca) before
    // acquiring the singleton lock, since the lock file lives inside app_dir.
    paths::migrate_legacy_app_dir();

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
    let config_base_dir = bootstrap::resolve_config_base_dir();
    info!("Config base dir: {}", config_base_dir.display());
    bootstrap::seed_default_configs(&config_base_dir);
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
            unsafe {
                std::env::set_var("OPENALPACA_MASTER_KEY", &hex_key);
            }
            info!(
                "Master key loaded from {}",
                app_dir.join(".master_key").display()
            );
        }
        Err(e) => {
            error!(
                "FATAL: Cannot ensure master key at {}: {e}",
                app_dir.display()
            );
            std::process::exit(1);
        }
    }

    // Install OS signal handlers BEFORE the tokio runtime starts.
    // This calls sigaction() in the single main thread, before any thread pools
    // (tokio, ONNX Runtime, etc.) are created. The handler sets an AtomicBool flag
    // which works regardless of which thread receives the signal.
    let shutdown_flag = shutdown::install_signal_handlers();
    info!("Signal handlers installed (SIGINT + SIGTERM)");

    // Start the tokio runtime AFTER env vars and signal handlers are set.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to build tokio runtime")?
        .block_on(async_main(config_base_dir, shutdown_flag))
}

async fn async_main(
    config_base_dir: PathBuf,
    shutdown_flag: Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    // Step 1: Bind to dynamic port (127.0.0.1:0 -> OS assigns port)
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("Failed to bind to localhost")?;
    let addr = listener.local_addr()?;
    let host = "127.0.0.1";
    let port = addr.port();
    info!("Listening on http://{host}:{port}");

    // Step 2: Generate discovery info and write atomically
    let instance_id = Uuid::new_v4().to_string();
    let disc =
        discovery::make_discovery(host, port, instance_id.clone(), env!("CARGO_PKG_VERSION"));
    let discovery_path = discovery::write_discovery_atomic(&disc)?;
    info!("Discovery written to: {}", discovery_path.display());
    let token = disc.auth.token.clone();

    // Step 3: Initialize database
    let db_path = paths::database_path()?;
    let db = Database::open(&db_path).context("Failed to initialize database")?;
    info!("Database initialized: {}", db_path.display());
    bootstrap::migrate_preference_summaries(&db);
    // Routing V2 Phase 3: fail all orphaned (non-terminal) tasks from the
    // previous daemon generation. MUST stay here — after the DB opens and
    // before any ingress starts (WakeManager::start in Step 8,
    // ConnectorManager::start_all in Step 12), so it can never sweep tasks
    // created by this run.
    bootstrap::sweep_orphaned_tasks(&db);

    // Step 4: Resolve stable local user ID
    let local_user_id = bootstrap::resolve_local_user_id(&db);
    let default_lane_key = format!("{local_user_id}:gui");
    info!("Local user ID: {local_user_id}, default lane: {default_lane_key}");

    // Step 5: Bootstrap persona documents
    let (initial_system_persona, soul_path) = bootstrap::bootstrap_system_persona(&config_base_dir);
    let (initial_user_document, user_path) = bootstrap::bootstrap_user_document(&config_base_dir);
    let (initial_identity_document, identity_path) =
        bootstrap::bootstrap_identity_document(&config_base_dir);

    let identity_has_content = initial_identity_document
        .as_ref()
        .is_some_and(identity_document_has_content);
    let user_has_content = initial_user_document
        .as_ref()
        .is_some_and(user_document_has_content);
    let (initial_bootstrap_document, bootstrap_path) = bootstrap::bootstrap_bootstrap_document(
        &config_base_dir,
        identity_has_content,
        user_has_content,
    );

    // Step 6: Load daemon config
    let daemon_config_path = config_base_dir.join("daemon.toml");
    let daemon_config = Arc::new(ArcSwap::from_pointee(load_daemon_config(
        &daemon_config_path,
    )));
    info!("Daemon config loaded from {}", daemon_config_path.display());

    // Step 7: Create event infrastructure
    let event_broadcaster = EventBroadcaster::new(
        daemon_config.load().server.event_broadcaster_capacity,
        instance_id.clone(),
        Some(db.clone()),
    );

    // Cancellation token for coordinated shutdown of all background tasks
    let cancel_token = tokio_util::sync::CancellationToken::new();

    // Single EventBus for system-wide event distribution
    let bus = EventBus::new(daemon_config.load().server.event_bus_capacity);

    // Create ChatStreamManager early (zero deps — just DashMap::new()) so the
    // event bridge can forward ToolConfirmationRequested events to SSE streams.
    let chat_stream_manager = Arc::new(ChatStreamManager::new());

    // Spawn SystemEvent → ServerEvent bridge
    event_bridge::spawn_event_bridge(
        event_broadcaster.clone(),
        &bus,
        Some(chat_stream_manager.clone()),
        cancel_token.clone(),
    );

    // Step 8: Initialize WakeManager
    let (wake_tx, wake_rx) = mpsc::channel(daemon_config.load().server.wake_channel_capacity);
    let mut wake_manager = WakeManager::new(wake_tx)
        .await
        .context("Failed to init WakeManager")?;

    // Register filesystem watchers for specific config paths
    let mut watch_paths = Vec::new();
    let agents_dir = config_base_dir.join("agents");
    if agents_dir.exists() {
        watch_paths.push(agents_dir.clone());
    }
    let llm_config_path = config_base_dir.join("llm.toml");
    if llm_config_path.exists() {
        watch_paths.push(llm_config_path.clone());
    }
    if daemon_config_path.exists() {
        watch_paths.push(daemon_config_path.clone());
    }
    if soul_path.exists() {
        watch_paths.push(soul_path.clone());
    }
    if user_path.exists() {
        watch_paths.push(user_path.clone());
    }
    if identity_path.exists() {
        watch_paths.push(identity_path.clone());
    }
    if let Some(ref bp) = bootstrap_path
        && bp.exists()
    {
        watch_paths.push(bp.clone());
    }
    let skills_dir = config_base_dir.join("skills");
    if skills_dir.exists() {
        watch_paths.push(skills_dir.clone());
    }
    if !watch_paths.is_empty() {
        info!("Wake: watching paths: {:?}", watch_paths);
        wake_manager.add_filesystem_watcher(watch_paths);
    }

    wake_manager
        .start()
        .await
        .context("Failed to start WakeManager")?;

    // Shared from here on: the scheduled-skills bridge (boot registration +
    // hot-reload re-sync inside the wake loop) needs scheduler access.
    let wake_manager = Arc::new(wake_manager);

    let fs_watch_handle = wake_manager.fs_watch_handle();

    // Step 9: Initialize all core services
    let svcs = services::initialize_services(
        &config_base_dir,
        &db,
        &bus,
        &daemon_config,
        &soul_path,
        &user_path,
        &identity_path,
        &cancel_token,
    )
    .await?;

    // Verify critical tool registered
    if svcs.tool_registry.get("update_persona").is_none() {
        anyhow::bail!("update_persona tool failed to register — persona updates will not work");
    }

    // Restore CostTracker from today's persisted usage so budget enforcement
    // is accurate across daemon restarts.
    if let Some(ref router) = svcs.llm_router {
        services::restore_cost_tracker(router, &db).await;
    }
    let cost_tracker_date = chrono::Utc::now().format("%Y-%m-%d").to_string();

    // Initialize PluginManager
    let plugin_dir = paths::app_dir()?.join("plugins");
    std::fs::create_dir_all(&plugin_dir).ok();
    let eb_for_plugins = event_broadcaster.clone();
    let plugin_manager = Arc::new(
        openalpaca_plugins::PluginManager::new(
            plugin_dir,
            svcs.tool_registry.clone(),
            Some(svcs.skill_catalog.clone()),
            Some(svcs.shared_context.agent_registry.clone()),
        )
        // Wire lifecycle events (ServerEvent::Plugin*) to the WS broadcaster
        // so clients like the GUI plugin panel live-update.
        .with_event_sink(Arc::new(move |event| eb_for_plugins.broadcast(event))),
    );
    if let Err(e) = plugin_manager.start().await {
        warn!("plugin manager startup: {e}");
    }

    // Step 10: Create ConfirmationBroker and construct Orchestrator
    let confirmation_broker = Arc::new(openalpaca_core::security::confirmation::ConfirmationBroker::new());

    let llm_router_for_reload = svcs.llm_router.clone();
    let llm_router_for_shutdown = svcs.llm_router.clone();
    let web_search_config_for_reload = svcs.web_search_config.clone();
    let lane_manager = Arc::new(LaneManager::new());

    let initial_loop_config = {
        let cfg = daemon_config.load();
        LoopConfig {
            experimental_ephemeral_pressure: cfg.experimental.ephemeral_pressure_layer,
            ..LoopConfig::default()
        }
    };

    let orchestrator = Arc::new(Orchestrator::new(
        svcs.shared_context.clone(),
        lane_manager.clone(),
        bus.clone(),
        initial_system_persona,
        svcs.llm_router,
        initial_loop_config,
        svcs.security_gate,
        svcs.tool_registry,
        Some(db.clone()),
        svcs.embedder.clone(),
        svcs.skill_catalog.clone(),
        svcs.skill_router.clone(),
        daemon_config.clone(),
    ));

    // Wire confirmation broker into orchestrator
    orchestrator.set_confirmation_broker(confirmation_broker.clone());

    // Set initial documents
    orchestrator.update_user_document(initial_user_document);
    orchestrator.set_user_path(user_path.clone());
    orchestrator.update_identity_document(initial_identity_document);
    orchestrator.set_identity_path(identity_path.clone());
    if let Some(ref doc) = initial_bootstrap_document {
        orchestrator.update_bootstrap_document(Some(doc.clone()));
    }
    if let Some(ref path) = bootstrap_path {
        orchestrator.set_bootstrap_path(path.clone());
    }

    // Gateway (needed by the hot-reload wake loop for scheduled-skill turns;
    // connectors and chat wire onto it in Step 12).
    let handler = Arc::new(gateway_bridge::OrchestratorHandler::new(
        orchestrator.clone(),
    ));
    let gateway = Arc::new(Gateway::new(
        svcs.shared_context.clone(),
        lane_manager,
        handler,
        bus.clone(),
        Some(db.clone()),
    ));

    // Scheduled skills: register cron jobs for skills with invoke.cron
    // frontmatter (gated by orchestrator.routing.scheduled_skills_enabled).
    scheduled_skills::sync_all(
        &wake_manager,
        &svcs.skill_catalog,
        daemon_config
            .load()
            .orchestrator
            .routing
            .scheduled_skills_enabled,
    )
    .await;

    // Step 11: Spawn hot-reload watchers
    let recent_soul_hashes = hot_reload::new_recent_hashes();
    let recent_user_hashes = hot_reload::new_recent_hashes();
    let recent_identity_hashes = hot_reload::new_recent_hashes();
    let recent_llm_hashes = hot_reload::new_recent_hashes();

    hot_reload::spawn_file_watcher(
        hot_reload::FileWatcherContext {
            soul_path: soul_path.clone(),
            user_path: user_path.clone(),
            identity_path: identity_path.clone(),
            bootstrap_path: bootstrap_path.clone(),
            llm_config_path: llm_config_path.clone(),
            daemon_config_path: daemon_config_path.clone(),
            skills_dir,
            agents_dir,
            orchestrator: orchestrator.clone(),
            agent_registry: svcs.shared_context.agent_registry.clone(),
            llm_router: llm_router_for_reload,
            secret_store: svcs.secret_store,
            skill_catalog: svcs.skill_catalog,
            daemon_config: daemon_config.clone(),
            web_search_config: web_search_config_for_reload,
            bus: bus.clone(),
            fs_watch_handle,
            gateway: gateway.clone(),
            wake_manager: wake_manager.clone(),
            local_user_id: local_user_id.clone(),
            soul_hashes: recent_soul_hashes.clone(),
            user_hashes: recent_user_hashes.clone(),
            identity_hashes: recent_identity_hashes.clone(),
            llm_hashes: recent_llm_hashes,
        },
        wake_rx,
        event_broadcaster.clone(),
        cancel_token.clone(),
    );

    hot_reload::spawn_soul_reload_subscriber(
        &bus,
        orchestrator.clone(),
        soul_path,
        recent_soul_hashes,
        cancel_token.clone(),
    );
    hot_reload::spawn_user_reload_subscriber(
        &bus,
        orchestrator.clone(),
        user_path,
        recent_user_hashes,
        cancel_token.clone(),
    );
    hot_reload::spawn_identity_reload_subscriber(
        &bus,
        orchestrator.clone(),
        identity_path,
        recent_identity_hashes,
        cancel_token.clone(),
    );

    // Step 12: Connectors, notifications, chat (gateway created above)
    // Routing V2: wire the follow-up runner so queued follow-ups re-enter
    // through the gateway when a workflow finalizes (inert while none are queued).
    orchestrator.set_followup_runner(Arc::new(followup::GatewayFollowupRunner::new(
        gateway.clone(),
        db.clone(),
    )));

    let notif_bus = bus.clone();
    let chat_bus = bus.clone();
    let connector_bus = bus.clone();
    let mut connector_manager = managers::connector::ConnectorManager::new(
        db.clone(),
        bus,
        gateway.clone(),
        daemon_config.clone(),
    );
    connector_manager.set_confirmation_broker(confirmation_broker.clone());
    connector_manager.start_all().await;

    // Create send bridge before connector-awareness block so it's available for both
    // the orchestrator and the notification dispatcher.
    let send_bridge: Arc<dyn openalpaca_core::orchestrator::ConnectorSendProvider> =
        Arc::new(connector_bridge::ConnectorSendBridge::new(
            db.clone(),
            local_user_id.clone(),
        ));

    // Connector awareness: cached status + send bridge wiring
    {
        // 1. Create cached status provider and populate initial state
        let connector_status_provider = Arc::new(connector_bridge::CachedConnectorStatusProvider::new());
        connector_status_provider.update(connector_manager.list_status().await);
        orchestrator.set_connector_status_provider(connector_status_provider.clone());

        // 2. Wire send bridge into orchestrator and shared lock
        orchestrator.set_connector_send_provider(send_bridge.clone());
        // Populate the shared lock so the send tool can access it
        if let Ok(mut guard) = svcs.connector_send_lock.write() {
            *guard = Some(send_bridge.clone());
        }

        // 3. Spawn EventBus subscriber to keep cached status fresh
        tokio::spawn({
            let mut rx = connector_bus.subscribe();
            let provider = connector_status_provider;
            let cm = connector_manager.clone();
            let ct = cancel_token.clone();
            async move {
                loop {
                    tokio::select! {
                        result = rx.recv() => match result {
                            Ok(openalpaca_core::events::SystemEvent::ConnectorStatus { .. }) => {
                                provider.update(cm.list_status().await);
                            }
                            Ok(_) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        },
                        _ = ct.cancelled() => break,
                    }
                }
            }
        });
    }

    // Spawn NotificationDispatcher (reads telegram token lazily from DB)
    {
        let notif_rx = notif_bus.subscribe();
        let dispatcher = notification::NotificationDispatcher::new(
            notif_rx,
            db.clone(),
            cancel_token.clone(),
            Some(send_bridge),
        );
        tokio::spawn(dispatcher.run());
    }

    // Build ChatService (chat_stream_manager created earlier for event bridge)
    let chat_service = Arc::new(ChatService::new(
        gateway.clone(),
        chat_stream_manager.clone(),
        db.clone(),
        chat_bus,
        daemon_config.clone(),
    ));

    // Step 13: Spawn background tasks
    if let Some(ref emb) = svcs.embedder {
        background::spawn_embedding_indexer(
            emb.clone(),
            db.clone(),
            daemon_config.clone(),
            cancel_token.clone(),
        );
    }
    background::spawn_memory_decay(db.clone(), daemon_config.clone(), cancel_token.clone());
    background::spawn_file_processing_worker(
        db.clone(),
        daemon_config.clone(),
        cancel_token.clone(),
    );
    background::spawn_asset_cleanup(db.clone(), daemon_config.clone(), cancel_token.clone());
    background::spawn_telemetry_cleanup(db.clone(), cancel_token.clone());

    // Step 14: Build AppState and HTTP router
    let db_for_shutdown = db.clone();
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);

    let state = Arc::new(AppState {
        instance_id: instance_id.clone(),
        token,
        event_broadcaster: event_broadcaster.clone(),
        db,
        shutdown_tx,
        connector_manager: connector_manager.clone(),
        gateway,
        llm_settings_service: svcs.llm_settings_service,
        agent_config_service: Some(svcs.agent_config_service),
        chat_service: Some(chat_service),
        chat_stream_manager: Some(chat_stream_manager.clone()),
        token_manager: svcs.token_manager,
        provider_usage_tracker: svcs.provider_usage_tracker,
        embedder: svcs.embedder,
        local_user_id,
        default_lane_key,
        llm_config_path,
        daemon_config: daemon_config.clone(),
        daemon_config_path,
        web_search_config: svcs.web_search_config,
        confirmation_broker: Some(confirmation_broker),
        plugin_manager: Some(plugin_manager),
    });

    let app = router::build_router(state);

    // Spawn heartbeat and chat cleanup
    background::spawn_heartbeat(
        event_broadcaster,
        daemon_config.clone(),
        cancel_token.clone(),
    );
    background::spawn_chat_cleanup(chat_stream_manager, daemon_config, cancel_token.clone());

    // Step 15: Run server with graceful shutdown
    info!("Daemon ready (instance: {instance_id})");

    let cancel_for_server = cancel_token.clone();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        tokio::select! {
            _ = shutdown::wait_for_shutdown(&shutdown_flag) => {
                info!("Shutdown signal received (OS)");
            }
            _ = shutdown_rx.recv() => {
                info!("Shutdown signal received (API)");
            }
        }
        cancel_for_server.cancel();
    });

    // Force-exit watchdog: if graceful shutdown takes too long, exit the process.
    let watchdog_cancel = cancel_token.clone();
    tokio::spawn(async move {
        watchdog_cancel.cancelled().await;
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
        warn!("Graceful shutdown timed out after 10s, forcing exit");
        std::process::exit(1);
    });

    if let Err(e) = server.await {
        error!("Server error: {e}");
    }

    // Flush CostTracker to DB (defense-in-depth)
    if let Some(ref router) = llm_router_for_shutdown {
        services::flush_cost_tracker(router, &db_for_shutdown, &cost_tracker_date).await;
    }

    // Shutdown connectors
    info!("Shutting down connectors...");
    connector_manager.shutdown_all().await;

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
