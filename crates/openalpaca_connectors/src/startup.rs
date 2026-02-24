//! Startup logic for auto-discovering and spawning connectors.

use openalpaca_core::bus::EventBus;
use openalpaca_core::gateway::Gateway;
use openalpaca_storage::{ConfigRepository, Database};
use std::sync::Arc;
#[allow(unused_imports)]
use tracing::{info, warn};

#[cfg(feature = "telegram")]
use crate::ConnectorBuilder;

#[cfg(feature = "telegram")]
use teloxide::dispatching::ShutdownToken;

#[cfg(all(feature = "imessage", target_os = "macos"))]
use tokio_util::sync::CancellationToken;

/// Handle to a running connector, allowing graceful shutdown.
pub enum ConnectorHandle {
    #[cfg(feature = "telegram")]
    Telegram(ShutdownToken),
    #[cfg(all(feature = "imessage", target_os = "macos"))]
    IMessage(CancellationToken),
    /// For future connectors or testing
    None,
}

impl std::fmt::Debug for ConnectorHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(feature = "telegram")]
            ConnectorHandle::Telegram(_) => write!(f, "ConnectorHandle::Telegram"),
            #[cfg(all(feature = "imessage", target_os = "macos"))]
            ConnectorHandle::IMessage(_) => write!(f, "ConnectorHandle::IMessage"),
            ConnectorHandle::None => write!(f, "ConnectorHandle::None"),
        }
    }
}

impl ConnectorHandle {
    pub async fn shutdown(self) {
        match self {
            #[cfg(feature = "telegram")]
            ConnectorHandle::Telegram(token) => {
                if let Ok(fut) = token.shutdown() {
                    fut.await;
                }
            }
            #[cfg(all(feature = "imessage", target_os = "macos"))]
            ConnectorHandle::IMessage(token) => {
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
) -> std::collections::HashMap<String, ConnectorHandle> {
    let mut started = std::collections::HashMap::new();

    // Initialize Config Repository
    let config_repo = ConfigRepository::new(&db);

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
                let connector =
                    ConnectorBuilder::new(db.clone(), bus.clone(), gateway.clone()).telegram(token);
                let handle = spawn_telegram(connector);
                started.insert("telegram".to_string(), ConnectorHandle::Telegram(handle));
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
            let cancel_token = CancellationToken::new();
            let connector = crate::imessage::IMessageConnector::new(
                Arc::new(db.clone()),
                Arc::new(bus.clone()),
                gateway.clone(),
                cancel_token.clone(),
            );

            tokio::spawn(async move {
                if let Err(e) = connector.run_loop().await {
                    tracing::error!("iMessage connector exited with error: {}", e);
                }
            });

            started.insert(
                "imessage".to_string(),
                ConnectorHandle::IMessage(cancel_token),
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
) -> ConnectorHandle {
    let connector = ConnectorBuilder::new(db, bus, gateway).telegram(token);
    ConnectorHandle::Telegram(spawn_telegram(connector))
}

/// Start dispatcher logic (internal)
#[cfg(feature = "telegram")]
fn spawn_telegram(connector: crate::telegram::TelegramConnector) -> ShutdownToken {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async move { connector.run_with_signal().await })
    })
}
