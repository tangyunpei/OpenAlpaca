//! Connector handles and spawn helpers used by the daemon's ConnectorManager.

use arc_swap::ArcSwap;
use openalpaca_core::bus::EventBus;
use openalpaca_core::daemon_config::DaemonConfig;
use openalpaca_core::gateway::Gateway;
use openalpaca_storage::Database;
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
