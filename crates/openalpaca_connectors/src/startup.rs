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

/// Handle to a running connector, allowing graceful shutdown.
pub enum ConnectorHandle {
    #[cfg(feature = "telegram")]
    Telegram(ShutdownToken),
    /// For future connectors or testing
    None,
}

impl std::fmt::Debug for ConnectorHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(feature = "telegram")]
            ConnectorHandle::Telegram(_) => write!(f, "ConnectorHandle::Telegram"),
            ConnectorHandle::None => write!(f, "ConnectorHandle::None"),
        }
    }
}

impl ConnectorHandle {
    pub async fn shutdown(self) {
        match self {
            #[cfg(feature = "telegram")]
            ConnectorHandle::Telegram(token) => {
                // Ignore errors during shutdown
                let _ = token.shutdown();
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

    // Future: iMessage, WeChat...
    // #[cfg(feature = "imessage")] ...

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
