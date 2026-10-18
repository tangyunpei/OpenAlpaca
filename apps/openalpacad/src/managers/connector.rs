use anyhow::Result;
use arc_swap::ArcSwap;
use openalpaca_connectors::startup::ConnectorHandle;
use openalpaca_core::bus::EventBus;
use openalpaca_core::daemon_config::DaemonConfig;
use openalpaca_core::gateway::Gateway;
use openalpaca_core::security::confirmation::ConfirmationBroker;
use openalpaca_storage::{ConfigRepository, Database};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

/// One connector as the manager sees it — what `GET /v1/connectors` reports
/// before the route adds the config- and message-derived fields (GAP-17, T49).
#[derive(Debug, Clone)]
pub struct ConnectorDetail {
    /// The connector's unique id (`telegram`), which is also the `source` the
    /// gateway stamps on every message it carries.
    pub id: String,
    /// The human-facing name, taken from the connector's own factory. Never
    /// derived from the id by a table the connector is not in.
    pub name: String,
    /// `active` | `error` | `disabled` | `unconfigured`.
    pub status: String,
    /// Whether this manager holds a spawned handle for the connector. It is
    /// **not** "is enabled" and **not** "is alive": a handle whose task has
    /// exited is still registered, and reports `status: "error"`.
    pub registered: bool,
}

/// Manages the lifecycle of platform connectors (Telegram, etc.)
#[derive(Clone)]
pub struct ConnectorManager {
    db: Database,
    bus: EventBus,
    gateway: Arc<Gateway>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    handles: Arc<Mutex<HashMap<String, ConnectorHandle>>>,
    confirmation_broker: Option<Arc<ConfirmationBroker>>,
}

impl ConnectorManager {
    pub fn new(
        db: Database,
        bus: EventBus,
        gateway: Arc<Gateway>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
    ) -> Self {
        Self {
            db,
            bus,
            gateway,
            daemon_config,
            handles: Arc::new(Mutex::new(HashMap::new())),
            confirmation_broker: None,
        }
    }

    /// Set the confirmation broker for interactive tool approval.
    pub fn set_confirmation_broker(&mut self, broker: Arc<ConfirmationBroker>) {
        self.confirmation_broker = Some(broker);
    }

    /// Start all enabled connectors based on configuration
    pub async fn start_all(&self) {
        info!("Starting enabled connectors...");
        let factories = openalpaca_connectors::get_supported_connectors();
        let config_repo = ConfigRepository::new(&self.db);
        let mut handles = HashMap::new();

        for factory in factories {
            let name = factory.name();
            // Check if enabled in config
            // Use specific key pattern: "{name}.enabled"
            let enabled = config_repo
                .get_bool(&format!("{}.enabled", name))
                .unwrap_or(false);

            if enabled {
                // Some connectors (e.g. iMessage) don't require a token
                let token = config_repo
                    .get(&format!("{}.token", name))
                    .ok()
                    .flatten()
                    .unwrap_or_default();

                match factory.spawn(
                    token,
                    self.db.clone(),
                    self.bus.clone(),
                    self.gateway.clone(),
                    self.daemon_config.clone(),
                    self.confirmation_broker.clone(),
                ) {
                    Ok(handle) => {
                        handles.insert(name.to_string(), handle);
                        self.broadcast_status(name, "active");
                        info!("Started connector: {}", name);
                    }
                    Err(e) => {
                        self.broadcast_status(name, "error");
                        tracing::error!("Failed to start connector {}: {}", name, e);
                    }
                }
            }
        }

        let mut guard = self.handles.lock().await;
        *guard = handles;
        info!("Started {} connectors", guard.len());
    }

    /// List status of all potential connectors, as `(id, status)`.
    ///
    /// The prompt-injection cache ([`crate::connector_bridge`]) wants only
    /// these two fields; it is a projection of [`Self::list_detail`] so the
    /// status a prompt sees and the status the route serves cannot diverge.
    pub async fn list_status(&self) -> Vec<(String, String)> {
        self.list_detail()
            .await
            .into_iter()
            .map(|d| (d.id, d.status))
            .collect()
    }

    /// Every compiled-in connector, with the name it gives itself and whether
    /// this manager holds a handle for it (GAP-17, T49).
    pub async fn list_detail(&self) -> Vec<ConnectorDetail> {
        // Collect (name, is_alive) pairs and release lock immediately
        let handle_info: Vec<(String, bool)> = {
            let guard = self.handles.lock().await;
            guard
                .iter()
                .map(|(k, v)| (k.clone(), v.is_alive()))
                .collect()
        };

        let config_repo = ConfigRepository::new(&self.db);
        let factories = openalpaca_connectors::get_supported_connectors();
        let mut statuses = Vec::new();

        for factory in factories {
            let name = factory.name();

            // Some connectors (e.g. iMessage) don't require a token —
            // they are "configured" by virtue of being on the right platform.
            let token_optional = matches!(name, "imessage");

            let registered = handle_info.iter().any(|(n, _)| n == name);
            let status = if let Some((_, alive)) = handle_info.iter().find(|(n, _)| n == name) {
                if *alive {
                    "active".to_string()
                } else {
                    // Handle exists but the spawned task has exited/crashed
                    "error".to_string()
                }
            } else {
                let has_token = config_repo
                    .get(&format!("{}.token", name))
                    .map(|v| v.is_some())
                    .unwrap_or(false);

                if !has_token && !token_optional {
                    "unconfigured".to_string()
                } else {
                    let enabled = config_repo
                        .get_bool(&format!("{}.enabled", name))
                        .unwrap_or(false);
                    if enabled {
                        "error".to_string()
                    } else {
                        "disabled".to_string()
                    }
                }
            };
            statuses.push(ConnectorDetail {
                id: name.to_string(),
                name: factory.display_name().to_string(),
                status,
                registered,
            });
        }

        statuses
    }

    /// Enable and start a connector
    pub async fn enable(&self, name: &str) -> Result<()> {
        let config_repo = ConfigRepository::new(&self.db);
        config_repo.set(&format!("{}.enabled", name), "true", "bool")?;

        let mut guard = self.handles.lock().await;
        if !guard.contains_key(name) {
            match self.spawn_connector(name, &mut guard) {
                Ok(_) => self.broadcast_status(name, "active"),
                Err(e) => {
                    self.broadcast_status(name, "error");
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// Disable and stop a connector
    pub async fn disable(&self, name: &str) -> Result<()> {
        let config_repo = ConfigRepository::new(&self.db);
        config_repo.set(&format!("{}.enabled", name), "false", "bool")?;

        self.stop(name).await?;
        self.broadcast_status(name, "disabled");
        Ok(())
    }

    /// Delete a connector's configuration and stop it
    pub async fn delete(&self, name: &str) -> Result<()> {
        let config_repo = ConfigRepository::new(&self.db);
        config_repo.delete(&format!("{}.token", name))?;
        config_repo.delete(&format!("{}.enabled", name))?;

        // Also clean up identity data (individual reset)
        use openalpaca_storage::IdentityRepository;
        let identity_repo = IdentityRepository::new(&self.db);
        if let Err(e) = identity_repo.delete_data_for_provider(name) {
            tracing::warn!("Failed to clean up identity data for {}: {}", name, e);
        }

        self.stop(name).await?;
        self.broadcast_status(name, "unconfigured");
        Ok(())
    }

    /// Update a connector's configuration (e.g. token)
    pub async fn update_config(&self, name: &str, token: &str) -> Result<()> {
        let config_repo = ConfigRepository::new(&self.db);
        config_repo.set(&format!("{}.token", name), token, "string")?;
        // Auto-enable when configuring
        config_repo.set(&format!("{}.enabled", name), "true", "bool")?;

        // Restart if running, or start if not
        self.stop(name).await?;

        let mut guard = self.handles.lock().await;
        if let Err(e) = self.spawn_connector(name, &mut guard) {
            self.broadcast_status(name, "error");
            return Err(e);
        }
        self.broadcast_status(name, "active");
        Ok(())
    }

    /// Stop all running connectors (used during daemon shutdown), each within
    /// what is left of the shutdown window: a connector whose stop does not
    /// return by `deadline` is left behind with a warning, and every handle
    /// after it is still told to stop (`crate::shutdown::within` polls each
    /// once, so a cancel token is cancelled even past the deadline).
    pub async fn shutdown_all(&self, deadline: std::time::Instant) {
        let mut guard = self.handles.lock().await;
        stop_each_within(guard.drain(), deadline, |_, handle| handle.shutdown()).await;
    }

    /// Stop a running connector
    pub async fn stop(&self, name: &str) -> Result<()> {
        let mut guard = self.handles.lock().await;
        if let Some(handle) = guard.remove(name) {
            handle.shutdown().await;
            info!("Stopped connector: {}", name);
        }
        Ok(())
    }

    fn spawn_connector(
        &self,
        name: &str,
        _guard: &mut HashMap<String, ConnectorHandle>,
    ) -> Result<()> {
        let factories = openalpaca_connectors::get_supported_connectors();
        let factory = factories.iter().find(|f| f.name() == name);

        if let Some(factory) = factory {
            let config_repo = ConfigRepository::new(&self.db);
            let token = config_repo
                .get(&format!("{}.token", name))?
                .unwrap_or_default();

            let handle = factory
                .spawn(
                    token,
                    self.db.clone(),
                    self.bus.clone(),
                    self.gateway.clone(),
                    self.daemon_config.clone(),
                    self.confirmation_broker.clone(),
                )
                .map_err(|e| anyhow::anyhow!("Failed to spawn {}: {}", name, e))?;

            _guard.insert(name.to_string(), handle);
            info!("Spawned connector: {}", name);
            Ok(())
        } else {
            anyhow::bail!("Unknown connector: {}", name)
        }
    }

    fn broadcast_status(&self, id: &str, status: &str) {
        use chrono::Utc;
        use openalpaca_core::events::SystemEvent;

        let event = SystemEvent::ConnectorStatus {
            id: id.to_string(),
            status: status.to_string(),
            timestamp: Utc::now(),
        };
        // Fire and forget
        self.bus.publish(event);
    }
}

/// Stops each connector in turn through `stop` — [`ConnectorHandle::shutdown`]
/// in the daemon — each until `deadline` and no longer
/// ([`crate::shutdown::within`]): a stop that has not returned by then is left
/// behind with a warning, and every handle after it is still polled once, so
/// a cancel token is cancelled even past the deadline. `stop` is a parameter
/// so a test can hand it a stop that never returns, which no
/// `ConnectorHandle` can be made to do without a live Telegram dispatcher.
async fn stop_each_within<F>(
    handles: impl IntoIterator<Item = (String, ConnectorHandle)>,
    deadline: std::time::Instant,
    stop: impl Fn(&str, ConnectorHandle) -> F,
) where
    F: std::future::Future<Output = ()>,
{
    for (name, handle) in handles {
        let stage = format!("Connector {name}'s shutdown");
        if crate::shutdown::within(&stage, deadline, stop(&name, handle))
            .await
            .is_some()
        {
            info!("Stopped connector: {}", name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use openalpaca_core::context::SharedContext;
    use openalpaca_core::gateway::{HandleRequest, HandleResult, MessageHandler};
    use openalpaca_core::lane::LaneManager;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;

    /// Far below the daemon's window, so a stop that ignores it fails the
    /// test instead of slowing it down.
    const WINDOW: Duration = Duration::from_millis(200);

    /// Every wait is bounded, so a regression fails instead of hanging.
    const MUST_RETURN_WITHIN: Duration = Duration::from_secs(2);

    /// A plugin connector's handle — its stop is a cancel token — and the
    /// token, to see whether it was told to stop.
    fn plugin_handle() -> (ConnectorHandle, CancellationToken) {
        let token = CancellationToken::new();
        let handle = ConnectorHandle::Plugin(token.clone(), Arc::new(AtomicBool::new(true)));
        (handle, token)
    }

    /// N-DAEMON-1: a connector whose stop never returns — Telegram's waits
    /// for its dispatcher to drain, and nothing bounded that wait — holds the
    /// loop only until the deadline, and the handle after it is still told to
    /// stop. The loop awaiting each stop unbounded, as it did, hangs here.
    #[tokio::test]
    async fn a_connector_whose_stop_never_returns_holds_the_rest_only_until_the_deadline() {
        let (telegram, _) = plugin_handle();
        let (discord, discord_stopped) = plugin_handle();
        let started = Instant::now();
        let handles = vec![
            ("telegram".to_string(), telegram),
            ("discord".to_string(), discord),
        ];
        let stop =
            |name: &str, handle: ConnectorHandle| -> Pin<Box<dyn Future<Output = ()> + Send>> {
                if name == "telegram" {
                    Box::pin(std::future::pending())
                } else {
                    Box::pin(handle.shutdown())
                }
            };
        tokio::time::timeout(
            MUST_RETURN_WITHIN,
            stop_each_within(handles, started + WINDOW, stop),
        )
        .await
        .unwrap_or_else(|_| {
            panic!(
                "a connector whose stop never returns held the shutdown past its {WINDOW:?} \
                 deadline — the daemon would sit out its watchdog"
            )
        });
        assert!(
            discord_stopped.is_cancelled(),
            "the connector after the hung one was never told to stop"
        );
        assert!(
            started.elapsed() >= WINDOW / 2,
            "the hung stop was given its window, not cut at once: {:?}",
            started.elapsed()
        );
    }

    /// Answers every turn; nothing here sends one.
    struct AckHandler;

    #[async_trait]
    impl MessageHandler for AckHandler {
        async fn handle(&self, _request: HandleRequest) -> Result<HandleResult, String> {
            Ok(HandleResult::text("ack".to_string()))
        }
    }

    /// The daemon's own path: `shutdown_all` drains every handle the manager
    /// holds and stops each, even when the stages before it spent the whole
    /// window — each stop is still polled once, so a plugin connector's token
    /// is cancelled — and leaves no handle registered.
    #[tokio::test]
    async fn shutdown_all_stops_every_handle_even_once_the_window_is_spent() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let db = Database::open(&dir.path().join("test.db")).expect("open the database");
        let gateway = Arc::new(Gateway::new(
            Arc::new(SharedContext::new()),
            Arc::new(LaneManager::new()),
            Arc::new(AckHandler),
            EventBus::default(),
            Some(db.clone()),
        ));
        let manager = ConnectorManager::new(
            db,
            EventBus::default(),
            gateway,
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        );
        let (first, first_stopped) = plugin_handle();
        let (second, second_stopped) = plugin_handle();
        manager.handles.lock().await.extend([
            ("plugin-a".to_string(), first),
            ("plugin-b".to_string(), second),
        ]);

        let spent = Instant::now();
        tokio::time::sleep(Duration::from_millis(5)).await;
        tokio::time::timeout(MUST_RETURN_WITHIN, manager.shutdown_all(spent))
            .await
            .expect("shutdown_all returned");

        assert!(
            first_stopped.is_cancelled(),
            "plugin-a was not told to stop"
        );
        assert!(
            second_stopped.is_cancelled(),
            "plugin-b was not told to stop"
        );
        assert!(
            manager.handles.lock().await.is_empty(),
            "a stopped connector is still registered"
        );
    }
}
