//! Startup logic for auto-discovering and spawning connectors.

use arc_swap::ArcSwap;
use openalpaca_core::bus::EventBus;
use openalpaca_core::daemon_config::DaemonConfig;
use openalpaca_core::gateway::Gateway;
use openalpaca_storage::{ConfigRepository, Database};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[allow(unused_imports)]
use tracing::{info, warn};

#[cfg(feature = "telegram")]
use crate::ConnectorBuilder;

#[cfg(feature = "telegram")]
use teloxide::dispatching::ShutdownToken;

use tokio_util::sync::CancellationToken;

/// Handle to a running connector, allowing graceful shutdown.
///
/// Each variant carries a `running` flag (`Arc<AtomicBool>`) that is set to
/// `false` when the spawned task exits (normal or error). This lets
/// `ConnectorManager::list_status()` detect crashed connectors instead of
/// reporting them as "active".
pub enum ConnectorHandle {
    #[cfg(feature = "telegram")]
    Telegram(ShutdownToken, Arc<AtomicBool>),
    #[cfg(all(feature = "imessage", target_os = "macos"))]
    IMessage(CancellationToken, Arc<AtomicBool>),
    #[cfg(feature = "discord")]
    Discord(CancellationToken, Arc<AtomicBool>),
    /// Plugin-backed connector managed by PluginManager.
    Plugin(CancellationToken, Arc<AtomicBool>),
    /// For future connectors or testing
    None,
}

/// Guard that clears a running flag on drop.
/// Wrap a connector future with this so the flag is cleared on exit.
#[allow(dead_code)] // constructed only in feature-gated connector spawn paths
pub(crate) struct RunningGuard(pub Arc<AtomicBool>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl std::fmt::Debug for ConnectorHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(feature = "telegram")]
            ConnectorHandle::Telegram(..) => write!(f, "ConnectorHandle::Telegram"),
            #[cfg(all(feature = "imessage", target_os = "macos"))]
            ConnectorHandle::IMessage(..) => write!(f, "ConnectorHandle::IMessage"),
            #[cfg(feature = "discord")]
            ConnectorHandle::Discord(..) => write!(f, "ConnectorHandle::Discord"),
            ConnectorHandle::Plugin(..) => write!(f, "ConnectorHandle::Plugin"),
            ConnectorHandle::None => write!(f, "ConnectorHandle::None"),
        }
    }
}

impl ConnectorHandle {
    /// Returns `true` if the connector's spawned task is still running.
    pub fn is_alive(&self) -> bool {
        match self {
            #[cfg(feature = "telegram")]
            ConnectorHandle::Telegram(_, running) => running.load(Ordering::Acquire),
            #[cfg(all(feature = "imessage", target_os = "macos"))]
            ConnectorHandle::IMessage(_, running) => running.load(Ordering::Acquire),
            #[cfg(feature = "discord")]
            ConnectorHandle::Discord(_, running) => running.load(Ordering::Acquire),
            ConnectorHandle::Plugin(_, running) => running.load(Ordering::Acquire),
            ConnectorHandle::None => false,
        }
    }

    pub async fn shutdown(self) {
        match self {
            #[cfg(feature = "telegram")]
            ConnectorHandle::Telegram(token, _) => {
                if let Ok(fut) = token.shutdown() {
                    fut.await;
                }
            }
            #[cfg(all(feature = "imessage", target_os = "macos"))]
            ConnectorHandle::IMessage(token, _) => {
                token.cancel();
            }
            #[cfg(feature = "discord")]
            ConnectorHandle::Discord(token, _) => {
                token.cancel();
            }
            ConnectorHandle::Plugin(token, _) => {
                token.cancel();
            }
            ConnectorHandle::None => {}
        }
    }
}

/// Automatically scan environment variables and spawn enabled connectors.
///
/// This function returns a map of {connector_name -> handle}.
pub fn auto_start_connectors(
    db: Database,
    bus: EventBus,
    gateway: Arc<Gateway>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
) -> std::collections::HashMap<String, ConnectorHandle> {
    #[allow(unused_mut)] // mutated only by the feature-gated blocks below
    let mut started = std::collections::HashMap::new();

    // Initialize Config Repository
    let config_repo = ConfigRepository::new(&db);
    // Read only by the feature-gated connector blocks below.
    let _ = (&bus, &gateway, &daemon_config, &config_repo);

    // --- Telegram ---
    #[cfg(feature = "telegram")]
    {
        // Check "telegram.enabled" first
        let enabled = match config_repo.get("telegram.enabled") {
            Ok(Some(v)) => v == "true",
            Ok(None) => true, // Default to true if not set (legacy behavior, or if token exists)
            Err(_) => true,
        };

        if enabled {
            // Strategy: 1. DB Config -> 2. Env Var -> 3. Skip
            let token_opt = match config_repo.get("telegram.token") {
                Ok(Some(t)) => Some(t),
                Ok(None) => None,
                Err(e) => {
                    warn!("Failed to read config: {}. Ignoring DB config.", e);
                    None
                }
            };

            // Fallback to Env Var
            let (token, source) = if let Some(t) = token_opt {
                (Some(t), "DB")
            } else if let Ok(t) = std::env::var("OPENALPACA_TELEGRAM_TOKEN") {
                (Some(t), "ENV")
            } else {
                (None, "")
            };

            if let Some(token) = token {
                info!("Autostart: Finding Telegram Token (Source: {})", source);

                // Clone dependencies to keep the originals valid for subsequent connectors
                let connector = ConnectorBuilder::new(
                    db.clone(),
                    bus.clone(),
                    gateway.clone(),
                    daemon_config.clone(),
                )
                .telegram(token);
                let (shutdown_token, running) = spawn_telegram(connector);
                started.insert("telegram".to_string(), ConnectorHandle::Telegram(shutdown_token, running));
            }
        } else {
            info!("Connector 'telegram' is disabled in config.");
        }
    }

    // --- iMessage (macOS only) ---
    #[cfg(all(feature = "imessage", target_os = "macos"))]
    {
        let enabled = match config_repo.get("imessage.enabled") {
            Ok(Some(v)) => v == "true",
            Ok(None) => false, // Default to disabled — requires explicit opt-in
            Err(_) => false,
        };

        if enabled {
            info!("Autostart: Spawning iMessage connector");
            let local_user_id = config_repo.get("identity.local_user_id").ok().flatten();
            let cancel_token = CancellationToken::new();
            let connector = crate::imessage::IMessageConnector::new(
                Arc::new(db.clone()),
                gateway.clone(),
                daemon_config.clone(),
                cancel_token.clone(),
                local_user_id,
            );

            let running = Arc::new(AtomicBool::new(true));
            let guard = RunningGuard(running.clone());
            tokio::spawn(async move {
                let _guard = guard;
                if let Err(e) = connector.run_loop().await {
                    tracing::error!("iMessage connector exited with error: {}", e);
                }
            });

            started.insert(
                "imessage".to_string(),
                ConnectorHandle::IMessage(cancel_token, running),
            );
        } else {
            info!("Connector 'imessage' is disabled in config.");
        }
    }

    // Future: WeChat...

    started
}

/// Spawn the Telegram connector using a provided token.
#[cfg(feature = "telegram")]
pub fn spawn_telegram_connector(
    token: String,
    db: Database,
    bus: EventBus,
    gateway: Arc<Gateway>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    confirmation_broker: Option<Arc<openalpaca_core::security::confirmation::ConfirmationBroker>>,
) -> ConnectorHandle {
    let mut builder = ConnectorBuilder::new(db, bus, gateway, daemon_config);
    if let Some(broker) = confirmation_broker {
        builder = builder.with_confirmation_broker(broker);
    }
    let connector = builder.telegram(token);
    let (shutdown_token, running) = spawn_telegram(connector);
    ConnectorHandle::Telegram(shutdown_token, running)
}

/// Start dispatcher logic (internal).
///
/// Returns the shutdown token and a running flag. The running flag is set to
/// `false` when the dispatcher task exits (via `RunningGuard` drop), so
/// `is_alive()` accurately reflects whether the Telegram connector is running.
#[cfg(feature = "telegram")]
fn spawn_telegram(connector: crate::telegram::TelegramConnector) -> (ShutdownToken, Arc<AtomicBool>) {
    let running = Arc::new(AtomicBool::new(true));
    let token = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current()
            .block_on(async { connector.run_with_signal(running.clone()).await })
    });
    (token, running)
}
