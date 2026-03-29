//! OpenAlpaca Connectors Module
//!
//! Provides unified connector interface for chat platforms.
//! Each platform is feature-gated:
//! - `telegram`: Telegram Bot API via teloxide
//! - `imessage`: iMessage (macOS only)
//! - `wechat`: WeChat (future)
//!
//! All connectors implement the `Connector` trait for a uniform interface.

pub mod adapter;
pub mod common;

#[cfg(feature = "telegram")]
pub mod telegram;

#[cfg(all(feature = "imessage", target_os = "macos"))]
pub mod imessage;

#[cfg(feature = "discord")]
pub mod discord;

pub mod startup;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use openalpaca_core::bus::EventBus;
use openalpaca_core::daemon_config::DaemonConfig;
use openalpaca_core::gateway::Gateway;
use openalpaca_core::security::confirmation::ConfirmationBroker;
use openalpaca_storage::Database;
use std::sync::atomic::AtomicBool;
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
    fn name(&self) -> &str;

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
    db: Arc<Database>,
    bus: Arc<EventBus>,
    gateway: Arc<Gateway>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    confirmation_broker: Option<Arc<ConfirmationBroker>>,
}

impl ConnectorBuilder {
    pub fn new(
        db: Database,
        bus: EventBus,
        gateway: Arc<Gateway>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
    ) -> Self {
        Self {
            db: Arc::new(db),
            bus: Arc::new(bus),
            gateway,
            daemon_config,
            confirmation_broker: None,
        }
    }

    /// Attach a confirmation broker for interactive tool approval.
    pub fn with_confirmation_broker(mut self, broker: Arc<ConfirmationBroker>) -> Self {
        self.confirmation_broker = Some(broker);
        self
    }

    /// Build a Telegram connector (requires `telegram` feature).
    #[cfg(feature = "telegram")]
    pub fn telegram(self, token: String) -> telegram::TelegramConnector {
        let connector =
            telegram::TelegramConnector::new(token, self.db, self.bus, self.gateway, self.daemon_config);
        if let Some(broker) = self.confirmation_broker {
            connector.with_confirmation_broker(broker)
        } else {
            connector
        }
    }

    /// Build an iMessage connector (requires `imessage` feature, macOS only).
    #[cfg(all(feature = "imessage", target_os = "macos"))]
    pub fn imessage(
        self,
        cancel_token: tokio_util::sync::CancellationToken,
        local_user_id: Option<String>,
    ) -> imessage::IMessageConnector {
        imessage::IMessageConnector::new(
            self.db,
            self.gateway,
            self.daemon_config,
            cancel_token,
            local_user_id,
        )
    }

    /// Build a Discord connector (requires `discord` feature).
    #[cfg(feature = "discord")]
    pub fn discord(
        self,
        token: String,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> discord::DiscordConnector {
        discord::DiscordConnector::new(
            token, self.db, self.gateway, self.daemon_config, cancel_token,
        )
    }
}

// Re-exports for convenience
#[cfg(feature = "telegram")]
pub use telegram::TelegramConnector;

#[cfg(all(feature = "imessage", target_os = "macos"))]
pub use imessage::IMessageConnector;

#[cfg(feature = "discord")]
pub use discord::DiscordConnector;

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
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
        confirmation_broker: Option<Arc<ConfirmationBroker>>,
    ) -> Result<startup::ConnectorHandle, ConnectorError>;
}

/// Registry of supported connectors based on compiled features
pub fn get_supported_connectors() -> Vec<Box<dyn ConnectorFactory>> {
    let connectors: Vec<Box<dyn ConnectorFactory>> = vec![
        #[cfg(feature = "telegram")]
        Box::new(TelegramFactory),
        #[cfg(all(feature = "imessage", target_os = "macos"))]
        Box::new(IMessageFactory),
        #[cfg(feature = "discord")]
        Box::new(DiscordFactory),
    ];

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
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
        confirmation_broker: Option<Arc<ConfirmationBroker>>,
    ) -> Result<startup::ConnectorHandle, ConnectorError> {
        let handle = startup::spawn_telegram_connector(
            token,
            db,
            bus,
            gateway,
            daemon_config,
            confirmation_broker,
        );
        Ok(handle)
    }
}

#[cfg(all(feature = "imessage", target_os = "macos"))]
struct IMessageFactory;

#[cfg(all(feature = "imessage", target_os = "macos"))]
impl ConnectorFactory for IMessageFactory {
    fn name(&self) -> &str {
        "imessage"
    }

    fn spawn(
        &self,
        _token: String,
        db: Database,
        _bus: EventBus,
        gateway: Arc<Gateway>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
        _confirmation_broker: Option<Arc<ConfirmationBroker>>,
    ) -> Result<startup::ConnectorHandle, ConnectorError> {
        let local_user_id = openalpaca_storage::ConfigRepository::new(&db)
            .get("identity.local_user_id")
            .ok()
            .flatten();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let connector = imessage::IMessageConnector::new(
            Arc::new(db),
            gateway,
            daemon_config,
            cancel_token.clone(),
            local_user_id,
        );

        let running = Arc::new(AtomicBool::new(true));
        let guard = startup::RunningGuard(running.clone());
        tokio::spawn(async move {
            let _guard = guard;
            if let Err(e) = connector.run_loop().await {
                tracing::error!("iMessage connector exited with error: {}", e);
            }
        });

        Ok(startup::ConnectorHandle::IMessage(cancel_token, running))
    }
}

#[cfg(feature = "discord")]
struct DiscordFactory;

#[cfg(feature = "discord")]
impl ConnectorFactory for DiscordFactory {
    fn name(&self) -> &str {
        "discord"
    }

    fn spawn(
        &self,
        token: String,
        db: Database,
        _bus: EventBus,
        gateway: Arc<Gateway>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
        _confirmation_broker: Option<Arc<ConfirmationBroker>>,
    ) -> Result<startup::ConnectorHandle, ConnectorError> {
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let connector = discord::DiscordConnector::new(
            token,
            Arc::new(db),
            gateway,
            daemon_config,
            cancel_token.clone(),
        );
        let running = Arc::new(AtomicBool::new(true));
        let guard = startup::RunningGuard(running.clone());
        tokio::spawn(async move {
            let _guard = guard;
            if let Err(e) = connector.run_loop().await {
                tracing::error!("Discord connector exited with error: {}", e);
            }
        });
        Ok(startup::ConnectorHandle::Discord(cancel_token, running))
    }
}
