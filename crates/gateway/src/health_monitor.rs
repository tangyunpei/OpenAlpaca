use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use openalpaca_channels::ChannelGatewayContext;
use openalpaca_core::types::{AccountId, ChannelId};
use tokio_util::sync::CancellationToken;

use crate::state::{BroadcastEvent, GatewayState};

/// Configuration for the health monitor.
pub struct HealthMonitorConfig {
    pub check_interval: Duration,
    pub probe_timeout: Duration,
    pub max_failures_before_restart: u32,
    pub max_restarts_per_hour: u32,
    pub startup_grace: Duration,
}

impl Default for HealthMonitorConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(300),
            probe_timeout: Duration::from_secs(10),
            max_failures_before_restart: 3,
            max_restarts_per_hour: 10,
            startup_grace: Duration::from_secs(60),
        }
    }
}

/// Per-account health tracking.
pub struct AccountHealth {
    pub ready: bool,
    pub last_check: Instant,
    pub last_ready: Option<Instant>,
    pub consecutive_failures: u32,
    pub restart_count: u32,
    pub last_restart: Option<Instant>,
}

impl AccountHealth {
    fn new() -> Self {
        Self {
            ready: true,
            last_check: Instant::now(),
            last_ready: None,
            consecutive_failures: 0,
            restart_count: 0,
            last_restart: None,
        }
    }
}

/// Periodic health monitor that probes channel accounts via `check_ready()`,
/// auto-restarts after consecutive failures, and broadcasts status changes.
pub struct HealthMonitor {
    config: HealthMonitorConfig,
    state: Arc<GatewayState>,
    health: HashMap<(ChannelId, AccountId), AccountHealth>,
}

impl HealthMonitor {
    pub fn new(state: Arc<GatewayState>, config: HealthMonitorConfig) -> Self {
        Self {
            config,
            state,
            health: HashMap::new(),
        }
    }

    /// Run the health monitor loop until shutdown.
    pub async fn run(mut self, shutdown: CancellationToken) {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep(self.config.startup_grace) => {}
        }

        tracing::info!(
            interval_secs = self.config.check_interval.as_secs(),
            "health monitor: started"
        );

        let mut interval = tokio::time::interval(self.config.check_interval);
        interval.tick().await; // consume first immediate tick

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = interval.tick() => {
                    self.check_all().await;
                }
            }
        }

        tracing::info!("health monitor: shutting down");
    }

    /// Probe all registered channel accounts and update health state.
    async fn check_all(&mut self) {
        let config_guard = self.state.config.load();
        let registry = self.state.channel_registry.read().await;

        for (channel_id, account_id, plugin) in registry.entries() {
            let ready = match tokio::time::timeout(
                self.config.probe_timeout,
                plugin.check_ready(&config_guard, account_id),
            )
            .await
            {
                Ok(Ok(ready)) => ready,
                Ok(Err(e)) => {
                    tracing::warn!(
                        channel = %channel_id.0,
                        account = %account_id.0,
                        error = %e,
                        "health monitor: check_ready failed"
                    );
                    false
                }
                Err(_) => {
                    tracing::warn!(
                        channel = %channel_id.0,
                        account = %account_id.0,
                        "health monitor: check_ready timed out"
                    );
                    false
                }
            };

            let key = (channel_id.clone(), account_id.clone());
            let health = self.health.entry(key).or_insert_with(AccountHealth::new);
            health.last_check = Instant::now();

            if ready {
                health.ready = true;
                health.last_ready = Some(Instant::now());
                health.consecutive_failures = 0;
            } else {
                health.ready = false;
                health.consecutive_failures += 1;

                if health.consecutive_failures >= self.config.max_failures_before_restart {
                    // Rate limit: reset counter if last restart was > 1 hour ago
                    if let Some(last_restart) = health.last_restart
                        && last_restart.elapsed() > Duration::from_secs(3600)
                    {
                        health.restart_count = 0;
                    }

                    if health.restart_count >= self.config.max_restarts_per_hour {
                        tracing::error!(
                            channel = %channel_id.0,
                            account = %account_id.0,
                            max = self.config.max_restarts_per_hour,
                            "health monitor: max restarts per hour exceeded, skipping"
                        );
                        continue;
                    }

                    tracing::warn!(
                        channel = %channel_id.0,
                        account = %account_id.0,
                        failures = health.consecutive_failures,
                        "health monitor: restarting unhealthy channel account"
                    );

                    let ctx = ChannelGatewayContext {
                        account_id: account_id.clone(),
                        config: Arc::clone(&*config_guard),
                    };

                    if let Err(e) = plugin.stop_account(&ctx).await {
                        tracing::error!(
                            channel = %channel_id.0,
                            account = %account_id.0,
                            error = %e,
                            "health monitor: stop_account failed"
                        );
                    }

                    if let Err(e) = plugin.start_account(&ctx).await {
                        tracing::error!(
                            channel = %channel_id.0,
                            account = %account_id.0,
                            error = %e,
                            "health monitor: start_account failed"
                        );
                    }

                    health.restart_count += 1;
                    health.last_restart = Some(Instant::now());
                    health.consecutive_failures = 0;

                    let _ = self
                        .state
                        .broadcast_tx
                        .send(BroadcastEvent::ChannelStatusChanged {
                            channel_id: channel_id.0.clone(),
                        });
                }
            }
        }
    }

    /// Return a snapshot of all channel health states.
    pub fn snapshot(&self) -> Vec<serde_json::Value> {
        self.health
            .iter()
            .map(|((channel_id, account_id), h)| {
                serde_json::json!({
                    "channelId": channel_id.0,
                    "accountId": account_id.0,
                    "ready": h.ready,
                    "consecutiveFailures": h.consecutive_failures,
                    "restartCount": h.restart_count,
                    "lastCheckSecsAgo": h.last_check.elapsed().as_secs(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use arc_swap::ArcSwap;
    use async_trait::async_trait;
    use tokio::sync::RwLock;

    use openalpaca_channels::{
        ChannelCapabilities, ChannelError, ChannelGatewayContext, ChannelMeta, ChannelPlugin,
        ChannelRegistry,
    };
    use openalpaca_config::OpenAlpacaConfig;
    use openalpaca_core::context::AppContext;
    use openalpaca_core::types::{AccountId, ChannelId};
    use openalpaca_sessions::SessionStore;

    use crate::auth::{AuthMode, GatewayAuth};
    use crate::rpc::build_default_registry;

    // -- Mock channel plugin --

    struct MockChannel {
        id: ChannelId,
        meta: ChannelMeta,
        capabilities: ChannelCapabilities,
        ready: Arc<StdMutex<bool>>,
        check_delay: Option<Duration>,
        start_count: Arc<AtomicU32>,
        stop_count: Arc<AtomicU32>,
    }

    impl MockChannel {
        fn new(
            ready: Arc<StdMutex<bool>>,
            start_count: Arc<AtomicU32>,
            stop_count: Arc<AtomicU32>,
        ) -> Self {
            let id = ChannelId("mock".into());
            Self {
                meta: ChannelMeta {
                    id: id.clone(),
                    label: "Mock".into(),
                    blurb: "Mock channel for testing".into(),
                    order: 0,
                    aliases: vec![],
                },
                capabilities: ChannelCapabilities::default(),
                id,
                ready,
                check_delay: None,
                start_count,
                stop_count,
            }
        }

        fn with_delay(mut self, delay: Duration) -> Self {
            self.check_delay = Some(delay);
            self
        }
    }

    #[async_trait]
    impl ChannelPlugin for MockChannel {
        fn id(&self) -> &ChannelId {
            &self.id
        }

        fn meta(&self) -> &ChannelMeta {
            &self.meta
        }

        fn capabilities(&self) -> &ChannelCapabilities {
            &self.capabilities
        }

        fn list_account_ids(&self, _config: &OpenAlpacaConfig) -> Vec<AccountId> {
            vec![AccountId("default".into())]
        }

        fn is_account_enabled(&self, _config: &OpenAlpacaConfig, _account_id: &AccountId) -> bool {
            true
        }

        async fn check_ready(
            &self,
            _config: &OpenAlpacaConfig,
            _account_id: &AccountId,
        ) -> Result<bool, ChannelError> {
            if let Some(delay) = self.check_delay {
                tokio::time::sleep(delay).await;
            }
            Ok(*self.ready.lock().unwrap())
        }

        async fn start_account(&self, _ctx: &ChannelGatewayContext) -> Result<(), ChannelError> {
            self.start_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn stop_account(&self, _ctx: &ChannelGatewayContext) -> Result<(), ChannelError> {
            self.stop_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    // -- Test helpers --

    fn make_test_state() -> Arc<GatewayState> {
        let app_ctx = Arc::new(AppContext {
            config_path: PathBuf::from("/tmp/test-config.yaml"),
            state_dir: PathBuf::from("/tmp/test-state"),
            home_dir: PathBuf::from("/tmp"),
            http_client: reqwest::Client::new(),
            shutdown: CancellationToken::new(),
        });

        let (broadcast_tx, _) = tokio::sync::broadcast::channel(16);

        Arc::new(GatewayState {
            app_ctx,
            config: Arc::new(ArcSwap::new(Arc::new(OpenAlpacaConfig::default()))),
            channel_registry: Arc::new(RwLock::new(ChannelRegistry::new())),
            session_store: Arc::new(SessionStore::new(PathBuf::from("/tmp/test-sessions.jsonl"))),
            auth: Arc::new(GatewayAuth::new(AuthMode::None)),
            rpc_registry: Arc::new(build_default_registry()),
            broadcast_tx,
        })
    }

    fn mock_key() -> (ChannelId, AccountId) {
        (ChannelId("mock".into()), AccountId("default".into()))
    }

    // -- Tests --

    #[test]
    fn test_default_config_values() {
        let config = HealthMonitorConfig::default();
        assert_eq!(config.check_interval, Duration::from_secs(300));
        assert_eq!(config.probe_timeout, Duration::from_secs(10));
        assert_eq!(config.max_failures_before_restart, 3);
        assert_eq!(config.max_restarts_per_hour, 10);
        assert_eq!(config.startup_grace, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_healthy_channel_resets_failures() {
        let state = make_test_state();
        let ready = Arc::new(StdMutex::new(true));
        let start_count = Arc::new(AtomicU32::new(0));
        let stop_count = Arc::new(AtomicU32::new(0));

        let mock = MockChannel::new(ready, start_count, stop_count);
        {
            let mut registry = state.channel_registry.write().await;
            registry.register(AccountId("default".into()), Box::new(mock));
        }

        let mut monitor = HealthMonitor::new(state, HealthMonitorConfig::default());

        // Pre-populate with some prior failures
        let key = mock_key();
        monitor.health.insert(
            key.clone(),
            AccountHealth {
                ready: false,
                last_check: Instant::now(),
                last_ready: None,
                consecutive_failures: 2,
                restart_count: 0,
                last_restart: None,
            },
        );

        monitor.check_all().await;

        let health = monitor.health.get(&key).unwrap();
        assert!(health.ready);
        assert_eq!(health.consecutive_failures, 0);
        assert!(health.last_ready.is_some());
    }

    #[tokio::test]
    async fn test_consecutive_failures_trigger_restart() {
        let state = make_test_state();
        let ready = Arc::new(StdMutex::new(false));
        let start_count = Arc::new(AtomicU32::new(0));
        let stop_count = Arc::new(AtomicU32::new(0));

        let mock = MockChannel::new(ready, start_count.clone(), stop_count.clone());
        {
            let mut registry = state.channel_registry.write().await;
            registry.register(AccountId("default".into()), Box::new(mock));
        }

        let config = HealthMonitorConfig {
            max_failures_before_restart: 3,
            ..Default::default()
        };
        let mut monitor = HealthMonitor::new(state, config);

        // 3 consecutive failures to trigger restart
        monitor.check_all().await; // consecutive_failures = 1
        monitor.check_all().await; // consecutive_failures = 2
        monitor.check_all().await; // consecutive_failures = 3 → restart

        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        assert_eq!(start_count.load(Ordering::SeqCst), 1);

        let key = mock_key();
        let health = monitor.health.get(&key).unwrap();
        assert_eq!(health.consecutive_failures, 0); // reset after restart
        assert_eq!(health.restart_count, 1);
    }

    #[tokio::test]
    async fn test_max_restarts_per_hour_enforced() {
        let state = make_test_state();
        let ready = Arc::new(StdMutex::new(false));
        let start_count = Arc::new(AtomicU32::new(0));
        let stop_count = Arc::new(AtomicU32::new(0));

        let mock = MockChannel::new(ready, start_count.clone(), stop_count.clone());
        {
            let mut registry = state.channel_registry.write().await;
            registry.register(AccountId("default".into()), Box::new(mock));
        }

        let config = HealthMonitorConfig {
            max_failures_before_restart: 1,
            max_restarts_per_hour: 2,
            ..Default::default()
        };
        let mut monitor = HealthMonitor::new(state, config);

        // Pre-populate: already at the restart limit
        let key = mock_key();
        monitor.health.insert(
            key.clone(),
            AccountHealth {
                ready: false,
                last_check: Instant::now(),
                last_ready: None,
                consecutive_failures: 0,
                restart_count: 2,
                last_restart: Some(Instant::now()),
            },
        );

        monitor.check_all().await;

        // Restart should be skipped due to rate limit
        assert_eq!(stop_count.load(Ordering::SeqCst), 0);
        assert_eq!(start_count.load(Ordering::SeqCst), 0);

        let health = monitor.health.get(&key).unwrap();
        assert_eq!(health.restart_count, 2); // unchanged
    }

    #[tokio::test]
    async fn test_restart_count_resets_after_one_hour() {
        let state = make_test_state();
        let ready = Arc::new(StdMutex::new(false));
        let start_count = Arc::new(AtomicU32::new(0));
        let stop_count = Arc::new(AtomicU32::new(0));

        let mock = MockChannel::new(ready, start_count.clone(), stop_count.clone());
        {
            let mut registry = state.channel_registry.write().await;
            registry.register(AccountId("default".into()), Box::new(mock));
        }

        let config = HealthMonitorConfig {
            max_failures_before_restart: 1,
            max_restarts_per_hour: 5,
            ..Default::default()
        };
        let mut monitor = HealthMonitor::new(state, config);

        // Set last_restart to > 1 hour ago (requires system uptime > 1hr)
        let past = match Instant::now().checked_sub(Duration::from_secs(3601)) {
            Some(p) => p,
            None => return, // System uptime too low, skip test
        };

        let key = mock_key();
        monitor.health.insert(
            key.clone(),
            AccountHealth {
                ready: false,
                last_check: Instant::now(),
                last_ready: None,
                consecutive_failures: 0,
                restart_count: 5, // at the limit
                last_restart: Some(past),
            },
        );

        monitor.check_all().await;

        // Counter should have been reset, allowing restart
        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        assert_eq!(start_count.load(Ordering::SeqCst), 1);

        let health = monitor.health.get(&key).unwrap();
        assert_eq!(health.restart_count, 1); // reset to 0, then +1
    }

    #[tokio::test]
    async fn test_broadcast_on_restart() {
        let state = make_test_state();
        let mut broadcast_rx = state.broadcast_tx.subscribe();
        let ready = Arc::new(StdMutex::new(false));
        let start_count = Arc::new(AtomicU32::new(0));
        let stop_count = Arc::new(AtomicU32::new(0));

        let mock = MockChannel::new(ready, start_count, stop_count);
        {
            let mut registry = state.channel_registry.write().await;
            registry.register(AccountId("default".into()), Box::new(mock));
        }

        let config = HealthMonitorConfig {
            max_failures_before_restart: 1,
            ..Default::default()
        };
        let mut monitor = HealthMonitor::new(state, config);

        monitor.check_all().await;

        let event = broadcast_rx.try_recv().unwrap();
        assert!(matches!(
            event,
            BroadcastEvent::ChannelStatusChanged { channel_id } if channel_id == "mock"
        ));
    }

    #[tokio::test]
    async fn test_probe_timeout_counts_as_failure() {
        let state = make_test_state();
        let ready = Arc::new(StdMutex::new(true)); // would return true, but will timeout
        let start_count = Arc::new(AtomicU32::new(0));
        let stop_count = Arc::new(AtomicU32::new(0));

        let mock =
            MockChannel::new(ready, start_count, stop_count).with_delay(Duration::from_millis(500)); // exceeds probe_timeout
        {
            let mut registry = state.channel_registry.write().await;
            registry.register(AccountId("default".into()), Box::new(mock));
        }

        let config = HealthMonitorConfig {
            probe_timeout: Duration::from_millis(50),
            max_failures_before_restart: 100, // high threshold, no restart
            ..Default::default()
        };
        let mut monitor = HealthMonitor::new(state, config);

        monitor.check_all().await;

        let key = mock_key();
        let health = monitor.health.get(&key).unwrap();
        assert!(!health.ready);
        assert_eq!(health.consecutive_failures, 1);
    }

    #[tokio::test]
    async fn test_shutdown_stops_monitor() {
        let state = make_test_state();
        let config = HealthMonitorConfig {
            startup_grace: Duration::from_secs(3600), // long, but we cancel early
            ..Default::default()
        };
        let monitor = HealthMonitor::new(state, config);
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();

        let handle = tokio::spawn(async move {
            monitor.run(shutdown_clone).await;
        });

        // Cancel immediately
        shutdown.cancel();

        // Monitor should exit during grace period
        let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(result.is_ok(), "monitor should exit on shutdown");
        assert!(result.unwrap().is_ok(), "monitor should exit cleanly");
    }
}
