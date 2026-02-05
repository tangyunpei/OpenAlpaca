//! OpenAlpaca Connectors Module
//!
//! Provides unified connector interface for chat platforms.
//! Each platform is feature-gated:
//! - `telegram`: Telegram Bot API via teloxide
//! - `imessage`: iMessage (macOS only) - future
//! - `wechat`: WeChat - future
//!
//! All connectors implement the `Connector` trait for a uniform interface.

pub mod adapter;
pub mod common;

#[cfg(feature = "telegram")]
pub mod telegram;

pub mod startup;

use async_trait::async_trait;
use openalpaca_core::bus::EventBus;
use openalpaca_core::gateway::Gateway;
use openalpaca_storage::Database;
use std::sync::Arc;

/// Unified connector trait for all chat platforms.
///
/// Each connector is responsible for:
/// 1. Receiving messages from the platform
/// 2. Converting them to SystemEvent::UserRequest
/// 3. Processing through the pipeline
/// 4. Sending responses back to the platform
#[async_trait]
pub trait Connector: Send + Sync {
    /// The unique identifier for this connector (e.g., "telegram", "imessage")
    fn name(&self) -> &'static str;

    /// Start the connector. This is typically a blocking call that
    /// runs the connector's main loop (e.g., polling, webhook server).
    async fn run(&self) -> Result<(), ConnectorError>;

    /// Gracefully shutdown the connector.
    async fn shutdown(&self) -> Result<(), ConnectorError>;
}

/// Common error type for connectors.
#[derive(Debug, thiserror::Error)]
pub enum ConnectorError {
    #[error("Initialization failed: {0}")]
    InitFailed(String),

    #[error("Connection error: {0}")]
    ConnectionError(String),

    #[error("Message send failed: {0}")]
    SendFailed(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Builder for creating connectors with shared dependencies.
pub struct ConnectorBuilder {
    #[allow(dead_code)]
    db: Arc<Database>,
    #[allow(dead_code)]
    bus: Arc<EventBus>,
    gateway: Arc<Gateway>,
}

impl ConnectorBuilder {
    pub fn new(db: Database, bus: EventBus, gateway: Arc<Gateway>) -> Self {
        Self {
            db: Arc::new(db),
            bus: Arc::new(bus),
            gateway,
        }
    }

    /// Build a Telegram connector (requires `telegram` feature).
    #[cfg(feature = "telegram")]
    pub fn telegram(self, token: String) -> telegram::TelegramConnector {
        telegram::TelegramConnector::new(token, self.db, self.bus, self.gateway)
    }
}

// Re-exports for convenience
#[cfg(feature = "telegram")]
pub use telegram::TelegramConnector;

/// Factory trait for creating connectors dynamically
pub trait ConnectorFactory: Send + Sync {
    /// Get the unique name of the connector (e.g. "telegram")
    fn name(&self) -> &str;

    /// Create and spawn a new instance of the connector
    fn spawn(
        &self,
        token: String,
        db: Database,
        bus: EventBus,
        gateway: Arc<Gateway>,
    ) -> Result<startup::ConnectorHandle, ConnectorError>;
}

/// Registry of supported connectors based on compiled features
pub fn get_supported_connectors() -> Vec<Box<dyn ConnectorFactory>> {
    let mut connectors: Vec<Box<dyn ConnectorFactory>> = Vec::new();

    #[cfg(feature = "telegram")]
    connectors.push(Box::new(TelegramFactory));

    connectors
}

#[cfg(feature = "telegram")]
struct TelegramFactory;

#[cfg(feature = "telegram")]
impl ConnectorFactory for TelegramFactory {
    fn name(&self) -> &str {
        "telegram"
    }

    fn spawn(
        &self,
        token: String,
        db: Database,
        bus: EventBus,
        gateway: Arc<Gateway>,
    ) -> Result<startup::ConnectorHandle, ConnectorError> {
        let handle = startup::spawn_telegram_connector(token, db, bus, gateway);
        Ok(handle)
    }
}
