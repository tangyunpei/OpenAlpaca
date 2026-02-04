//! Startup logic for auto-discovering and spawning connectors.

use openalpaca_core::bus::EventBus;
use openalpaca_storage::Database;
use tracing::{error, info};

#[cfg(feature = "telegram")]
use crate::ConnectorBuilder;

/// Automatically scan environment variables and spawn enabled connectors.
///
/// This function returns a list of names of spawned connectors.
pub fn auto_start_connectors(db: Database, bus: EventBus) -> Vec<String> {
    let mut started = Vec::new();

    // --- Telegram ---
    #[cfg(feature = "telegram")]
    if let Ok(token) = std::env::var("OPENALPACA_TELEGRAM_TOKEN") {
        info!("Autostart: Found OPENALPACA_TELEGRAM_TOKEN");
        // Clone dependencies to keep the originals valid for subsequent connectors
        let connector = ConnectorBuilder::new(db.clone(), bus.clone()).telegram(token);
        spawn_telegram(connector);
        started.push("telegram".to_string());
    }

    // Future: iMessage, WeChat...
    // #[cfg(feature = "imessage")] ...

    started
}

/// Spawn the Telegram connector.
/// This uses `run_blocking` directly because teloxide's dispatcher consumes `self`.
#[cfg(feature = "telegram")]
fn spawn_telegram(connector: crate::telegram::TelegramConnector) {
    tokio::spawn(async move {
        info!("Starting connector: telegram");
        connector.run_blocking().await;
        // run_blocking only returns when dispatcher stops (e.g., Ctrl+C)
        info!("Telegram connector stopped");
    });
}
