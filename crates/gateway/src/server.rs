use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use sha2::Digest;
use tokio::sync::RwLock;

use openalpaca_channels::ChannelRegistry;
use openalpaca_config::OpenAlpacaConfig;
use openalpaca_config::hot_reload::ConfigWatcher;
use openalpaca_core::CoreError;
use openalpaca_core::context::AppContext;
use openalpaca_sessions::SessionStore;

use crate::auth::{AuthMode, GatewayAuth};
use crate::health_monitor::{HealthMonitor, HealthMonitorConfig};
use crate::http::build_router;
use crate::rpc::build_default_registry;
use crate::state::{BroadcastEvent, GatewayState};
use crate::tls::load_rustls_config;

/// Run the gateway server.
pub async fn run_gateway(
    app_ctx: Arc<AppContext>,
    config: OpenAlpacaConfig,
) -> Result<(), CoreError> {
    // Read server params from initial config BEFORE handing it to the watcher
    let port = config.gateway.as_ref().and_then(|g| g.port).unwrap_or(3777);
    let host = config
        .gateway
        .as_ref()
        .and_then(|g| g.host.clone())
        .unwrap_or_else(|| "127.0.0.1".to_string());

    let auth_mode = config
        .gateway
        .as_ref()
        .and_then(|g| g.auth.as_ref())
        .and_then(|a| a.token.as_ref())
        .map(|t| AuthMode::Token(t.clone()))
        .unwrap_or(AuthMode::None);

    let tls_config = config.gateway.as_ref().and_then(|g| g.tls.clone());

    // Resolve reload settings
    let reload_mode = config
        .gateway
        .as_ref()
        .and_then(|g| g.reload.as_ref())
        .and_then(|r| r.mode.as_deref())
        .unwrap_or("hot");
    let debounce_ms = config
        .gateway
        .as_ref()
        .and_then(|g| g.reload.as_ref())
        .and_then(|r| r.debounce_ms);

    let sessions_path = app_ctx.state_dir.join("sessions.jsonl");
    let (broadcast_tx, _) = tokio::sync::broadcast::channel::<BroadcastEvent>(64);

    // Create shared config — with or without file watcher.
    // When mode is "off", no file watching occurs.
    let (shared_config, _config_watcher) = if reload_mode == "off" {
        tracing::info!("gateway: config reload disabled (mode=off)");
        let arc = Arc::new(ArcSwap::new(Arc::new(config)));
        (arc, None)
    } else {
        let watcher =
            ConfigWatcher::new(app_ctx.config_path.clone(), config, debounce_ms)?;
        let arc = watcher.config_arc();
        (arc, Some(watcher))
    };
    // NOTE: _config_watcher is kept alive here for the server's lifetime.
    // Dropping it stops the notify file watcher and the debounce loop task.

    let state = Arc::new(GatewayState {
        app_ctx: app_ctx.clone(),
        config: shared_config.clone(),
        channel_registry: Arc::new(RwLock::new(ChannelRegistry::new())),
        session_store: Arc::new(SessionStore::new(sessions_path)),
        auth: Arc::new(GatewayAuth::new(auth_mode)),
        rpc_registry: Arc::new(build_default_registry()),
        broadcast_tx: broadcast_tx.clone(),
    });

    // Spawn health monitor
    let monitor = HealthMonitor::new(state.clone(), HealthMonitorConfig::default());
    let monitor_shutdown = app_ctx.shutdown.clone();
    tokio::spawn(async move {
        monitor.run(monitor_shutdown).await;
    });

    // Spawn config change listener (only when watcher exists)
    if let Some(ref watcher) = _config_watcher {
        let broadcast_for_config = broadcast_tx.clone();
        let config_for_hash = shared_config.clone();
        let mut change_rx = watcher.subscribe();
        let shutdown = app_ctx.shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    result = change_rx.changed() => {
                        if result.is_err() { break; }
                        let hash = compute_config_hash(&config_for_hash);
                        tracing::info!(hash = %hash, "gateway: config changed, broadcasting");
                        let _ = broadcast_for_config.send(
                            BroadcastEvent::ConfigChanged { hash },
                        );
                    }
                }
            }
            tracing::debug!("gateway: config change listener stopped");
        });
    }

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e| CoreError::InvalidConfig(format!("invalid bind address: {e}")))?;

    let router = build_router(state);
    let shutdown = app_ctx.shutdown.clone();

    if let Some(ref tls) = tls_config {
        match (&tls.cert, &tls.key) {
            (Some(cert_path), Some(key_path)) => {
                tracing::info!(%addr, "gateway: starting HTTPS server (rustls)");
                let rustls_config = load_rustls_config(cert_path, key_path).await?;
                let handle = axum_server::Handle::new();
                let shutdown_handle = handle.clone();
                tokio::spawn(async move {
                    shutdown.cancelled().await;
                    tracing::info!("gateway: shutdown signal received");
                    shutdown_handle.graceful_shutdown(Some(Duration::from_secs(30)));
                });
                axum_server::bind_rustls(addr, rustls_config)
                    .handle(handle)
                    .serve(router.into_make_service_with_connect_info::<SocketAddr>())
                    .await?;
            }
            (None, None) => {
                // Empty tls section with no cert/key — fall through to plain HTTP
                tracing::info!(%addr, "gateway: starting HTTP server");
                let listener = tokio::net::TcpListener::bind(addr).await?;
                axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(async move {
                    shutdown.cancelled().await;
                    tracing::info!("gateway: shutdown signal received");
                })
                .await?;
            }
            _ => {
                return Err(CoreError::InvalidConfig(
                    "gateway.tls requires both 'cert' and 'key' to be set".into(),
                ));
            }
        }
    } else {
        tracing::info!(%addr, "gateway: starting HTTP server");
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            shutdown.cancelled().await;
            tracing::info!("gateway: shutdown signal received");
        })
        .await?;
    }

    tracing::info!("gateway: server stopped");
    Ok(())
}

/// Compute an opaque hash of the current config for broadcast events.
/// NOTE: HashMap iteration order is non-deterministic, so the same logical
/// config may produce different hashes across serializations. This is fine —
/// the hash is an opaque change identifier, not used for equality comparison.
fn compute_config_hash(config: &Arc<ArcSwap<OpenAlpacaConfig>>) -> String {
    let guard = config.load();
    let yaml = serde_yml::to_string(&**guard).unwrap_or_default();
    format!("{:x}", sha2::Sha256::digest(yaml.as_bytes()))
}

/// Helper to create a GatewayState for testing with a specific config path.
pub fn build_state_for_cli(
    app_ctx: Arc<AppContext>,
    config: OpenAlpacaConfig,
    sessions_path: PathBuf,
) -> Arc<GatewayState> {
    let auth_mode = config
        .gateway
        .as_ref()
        .and_then(|g| g.auth.as_ref())
        .and_then(|a| a.token.as_ref())
        .map(|t| AuthMode::Token(t.clone()))
        .unwrap_or(AuthMode::None);

    let (broadcast_tx, _) = tokio::sync::broadcast::channel::<BroadcastEvent>(64);

    Arc::new(GatewayState {
        app_ctx,
        config: Arc::new(ArcSwap::new(Arc::new(config))),
        channel_registry: Arc::new(RwLock::new(ChannelRegistry::new())),
        session_store: Arc::new(SessionStore::new(sessions_path)),
        auth: Arc::new(GatewayAuth::new(auth_mode)),
        rpc_registry: Arc::new(build_default_registry()),
        broadcast_tx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_config::hot_reload::ConfigWatcher;
    use openalpaca_config::io::load_config;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn test_config_watcher_arc_shared_with_gateway_state() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "gateway:\n  port: 3777").unwrap();

        let snapshot = load_config(&path).unwrap();
        let watcher = ConfigWatcher::new(path.clone(), snapshot.config, None).unwrap();
        let shared = watcher.config_arc();

        // Simulate what GatewayState would do
        let state_config = shared.clone();

        // Both point to the same ArcSwap
        assert_eq!(
            state_config.load().gateway.as_ref().unwrap().port,
            Some(3777)
        );

        // Modify config on disk
        std::fs::write(&path, "gateway:\n  port: 9999").unwrap();
        let mut rx = watcher.subscribe();
        let _ = tokio::time::timeout(Duration::from_secs(2), rx.changed()).await;

        // state_config sees the new value because it shares the ArcSwap
        assert_eq!(
            state_config.load().gateway.as_ref().unwrap().port,
            Some(9999)
        );
    }

    #[tokio::test]
    async fn test_config_change_broadcasts_event() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "gateway:\n  port: 3777").unwrap();

        let snapshot = load_config(&path).unwrap();
        let watcher = ConfigWatcher::new(path.clone(), snapshot.config, None).unwrap();
        let shared = watcher.config_arc();

        let (broadcast_tx, mut broadcast_rx) =
            tokio::sync::broadcast::channel::<BroadcastEvent>(16);

        // Spawn listener (mirrors server.rs logic)
        let config_for_hash = shared.clone();
        let mut change_rx = watcher.subscribe();
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_clone.cancelled() => break,
                    result = change_rx.changed() => {
                        if result.is_err() { break; }
                        let hash = compute_config_hash(&config_for_hash);
                        let _ = broadcast_tx.send(BroadcastEvent::ConfigChanged { hash });
                    }
                }
            }
        });

        // Modify config
        std::fs::write(&path, "gateway:\n  port: 8080").unwrap();

        // Should receive broadcast
        let event = tokio::time::timeout(Duration::from_secs(2), broadcast_rx.recv())
            .await
            .expect("timeout")
            .expect("recv error");

        assert!(matches!(event, BroadcastEvent::ConfigChanged { .. }));
        shutdown.cancel();
    }

    #[tokio::test]
    async fn test_no_op_change_no_broadcast() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "gateway:\n  port: 3777").unwrap();

        let snapshot = load_config(&path).unwrap();
        let watcher = ConfigWatcher::new(path.clone(), snapshot.config, None).unwrap();

        let (broadcast_tx, mut broadcast_rx) =
            tokio::sync::broadcast::channel::<BroadcastEvent>(16);

        let mut change_rx = watcher.subscribe();
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_clone.cancelled() => break,
                    result = change_rx.changed() => {
                        if result.is_err() { break; }
                        let _ = broadcast_tx.send(BroadcastEvent::ConfigChanged {
                            hash: "test".into(),
                        });
                    }
                }
            }
        });

        // Rewrite identical content
        tokio::time::sleep(Duration::from_millis(50)).await;
        std::fs::write(&path, "gateway:\n  port: 3777").unwrap();

        // Should NOT receive broadcast (ConfigWatcher skips no-op)
        let result = tokio::time::timeout(Duration::from_secs(1), broadcast_rx.recv()).await;
        assert!(result.is_err(), "should timeout — no broadcast for no-op");
        shutdown.cancel();
    }

    #[tokio::test]
    async fn test_shutdown_stops_config_listener() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "gateway:\n  port: 3777").unwrap();

        let snapshot = load_config(&path).unwrap();
        let watcher = ConfigWatcher::new(path, snapshot.config, None).unwrap();

        let (broadcast_tx, _) = tokio::sync::broadcast::channel::<BroadcastEvent>(16);
        let mut change_rx = watcher.subscribe();
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_clone.cancelled() => break,
                    result = change_rx.changed() => {
                        if result.is_err() { break; }
                        let _ = broadcast_tx.send(BroadcastEvent::ConfigChanged {
                            hash: "test".into(),
                        });
                    }
                }
            }
        });

        // Cancel shutdown
        shutdown.cancel();

        // Listener task should exit cleanly
        let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(result.is_ok(), "listener should exit on shutdown");
    }
}
