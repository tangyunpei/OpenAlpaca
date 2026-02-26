use anyhow::Result;
use arc_swap::ArcSwap;
use openalpaca_connectors::startup::ConnectorHandle;
use openalpaca_core::bus::EventBus;
use openalpaca_core::daemon_config::DaemonConfig;
use openalpaca_core::gateway::Gateway;
use openalpaca_storage::{ConfigRepository, Database};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

/// Manages the lifecycle of platform connectors (Telegram, etc.)
#[derive(Clone)]
pub struct ConnectorManager {
    db: Database,
    bus: EventBus,
    gateway: Arc<Gateway>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    handles: Arc<Mutex<HashMap<String, ConnectorHandle>>>,
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
        }
    }

    /// Start all enabled connectors based on configuration
    pub async fn start_all(&self) {
        info!("Starting enabled connectors...");
        // auto_start_connectors logic is now moved here to support dynamic loading
        // We will keep auto_start_connectors in openalpaca_connectors for legacy/CLI if needed,
        // but for Daemon we implement the new logic manually.

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

    /// List status of all potential connectors
    pub async fn list_status(&self) -> Vec<(String, String)> {
        // Optimize: Get running connectors and release lock immediately
        let running_connectors: Vec<String> = {
            let guard = self.handles.lock().await;
            guard.keys().cloned().collect()
        };

        let config_repo = ConfigRepository::new(&self.db);
        let factories = openalpaca_connectors::get_supported_connectors();
        let mut statuses = Vec::new();

        for factory in factories {
            let name = factory.name();

            // Some connectors (e.g. iMessage) don't require a token —
            // they are "configured" by virtue of being on the right platform.
            let token_optional = matches!(name, "imessage");

            let status = if running_connectors.contains(&name.to_string()) {
                "active".to_string()
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
            statuses.push((name.to_string(), status));
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

    /// Stop all running connectors (used during daemon shutdown)
    pub async fn shutdown_all(&self) {
        let mut guard = self.handles.lock().await;
        for (name, handle) in guard.drain() {
            handle.shutdown().await;
            info!("Stopped connector: {}", name);
        }
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
