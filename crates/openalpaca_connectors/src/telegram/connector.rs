//! Telegram Connector: struct, lifecycle, and trait implementation.

use super::delivery::send_with_retry;
use super::rate_limiter::ChatRateLimiter;
use crate::{Connector, ConnectorError};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use dashmap::DashMap;
use openalpaca_core::{
    bus::EventBus,
    daemon_config::DaemonConfig,
    events::SystemEvent,
    gateway::Gateway,
    security::confirmation::ConfirmationBroker,
};
use openalpaca_storage::{Database, IdentityRepository, PreferenceRepository};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use tracing::{debug, error, info, warn};

/// TelegramConnector manages the Telegram bot lifecycle and message handling.
pub struct TelegramConnector {
    bot: Bot,
    db: Arc<Database>,
    bus: Arc<EventBus>,
    gateway: Arc<Gateway>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    rate_limiter: Arc<ChatRateLimiter>,
    confirmation_broker: Option<Arc<ConfirmationBroker>>,
    /// Maps chat_id -> queue of request_ids for pending tool confirmations.
    /// VecDeque allows FIFO processing when multiple tools need confirmation.
    pending_confirmations: Arc<DashMap<i64, VecDeque<String>>>,
}

impl TelegramConnector {
    /// Create a new TelegramConnector with the given bot token.
    pub fn new(
        token: String,
        db: Arc<Database>,
        bus: Arc<EventBus>,
        gateway: Arc<Gateway>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
    ) -> Self {
        let bot = Bot::new(token);
        Self {
            bot,
            db,
            bus,
            gateway,
            daemon_config,
            rate_limiter: Arc::new(ChatRateLimiter::new(Duration::from_secs(1))),
            confirmation_broker: None,
            pending_confirmations: Arc::new(DashMap::new()),
        }
    }

    /// Attach a confirmation broker for interactive tool approval.
    pub fn with_confirmation_broker(mut self, broker: Arc<ConfirmationBroker>) -> Self {
        self.confirmation_broker = Some(broker);
        self
    }

    /// Start the connector (blocking, runs the teloxide dispatcher).
    /// Returns a ShutdownToken to stop the dispatcher.
    ///
    /// The `running` flag is wrapped in a `RunningGuard` inside the spawned
    /// task so that `is_alive()` returns `false` once the dispatcher exits.
    pub async fn run_with_signal(
        self,
        running: Arc<std::sync::atomic::AtomicBool>,
    ) -> teloxide::dispatching::ShutdownToken {
        info!("Starting Telegram Connector...");

        let handler = Update::filter_message().endpoint(Self::handle_message);

        // Clone state for the handler
        let db = self.db.clone();
        let bus = self.bus.clone();
        let gateway = self.gateway.clone();
        let daemon_config = self.daemon_config.clone();
        let rate_limiter = self.rate_limiter.clone();
        let confirmation_broker: Option<Arc<ConfirmationBroker>> =
            self.confirmation_broker.clone();
        let pending = self.pending_confirmations.clone();

        // Spawn confirmation listener (if broker available)
        if confirmation_broker.is_some() {
            Self::spawn_confirmation_listener(
                self.bus.clone(),
                self.bot.clone(),
                self.db.clone(),
                self.pending_confirmations.clone(),
            );
        }

        let mut dispatcher = Dispatcher::builder(self.bot, handler)
            .dependencies(teloxide::dptree::deps![
                db,
                bus,
                gateway,
                daemon_config,
                rate_limiter,
                confirmation_broker,
                pending
            ])
            .build();

        let token = dispatcher.shutdown_token();

        tokio::spawn(async move {
            // Mark alive while running, then clear on exit
            struct RunningGuard(Arc<std::sync::atomic::AtomicBool>);
            impl Drop for RunningGuard {
                fn drop(&mut self) {
                    self.0
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                }
            }
            running.store(true, std::sync::atomic::Ordering::Relaxed);
            let _guard = RunningGuard(running);

            dispatcher.dispatch().await;
            info!("Telegram connector dispatcher finished");
        });

        token
    }

    /// Spawn a background task that listens for `ToolConfirmationRequested`
    /// events targeting Telegram lanes and sends confirmation prompts.
    fn spawn_confirmation_listener(
        bus: Arc<EventBus>,
        bot: Bot,
        db: Arc<Database>,
        pending: Arc<DashMap<i64, VecDeque<String>>>,
    ) {
        let mut rx = bus.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(SystemEvent::ToolConfirmationRequested {
                        request_id,
                        agent_id: _,
                        tool_name,
                        tool_arguments,
                        stream_id: _,
                        lane_key: Some(ref lane_key),
                        timestamp: _,
                    }) if lane_key.ends_with(":telegram") => {
                        // Resolve chat_id from lane_key via DB lookup
                        let identity_repo = IdentityRepository::new(&db);
                        let chat_id = {
                            // First try preference (most recent chat)
                            let user_id = lane_key.strip_suffix(":telegram").unwrap_or("");
                            let pref_repo = PreferenceRepository::new(&db);
                            pref_repo
                                .get(user_id, "telegram.last_chat_id")
                                .ok()
                                .flatten()
                                .and_then(|p| p.value.parse::<i64>().ok())
                                .or_else(|| {
                                    identity_repo
                                        .get_chat_id_by_lane_key(lane_key, "telegram")
                                        .ok()
                                        .flatten()
                                })
                        };

                        let Some(chat_id) = chat_id else {
                            warn!(
                                "Could not resolve Telegram chat_id for lane_key={}, skipping confirmation",
                                lane_key
                            );
                            continue;
                        };

                        // Store pending confirmation mapping (queue per chat)
                        pending.entry(chat_id).or_default().push_back(request_id.clone());
                        let queue_len = pending.get(&chat_id).map(|q| q.len()).unwrap_or(1);

                        // Format arguments for display (truncate if too long)
                        let args_display = {
                            let s = serde_json::to_string_pretty(&tool_arguments)
                                .unwrap_or_else(|_| tool_arguments.to_string());
                            if s.len() > 500 {
                                format!("{}...", &s[..500])
                            } else {
                                s
                            }
                        };

                        let queue_info = if queue_len > 1 {
                            format!(" (1 of {} pending)", queue_len)
                        } else {
                            String::new()
                        };

                        let prompt = format!(
                            "A tool requires your confirmation before executing{queue_info}:\n\n\
                             Tool: {tool_name}\n\
                             Arguments:\n{args_display}\n\n\
                             Reply /yes or /no to approve or deny."
                        );

                        if let Err(e) =
                            send_with_retry(&bot, ChatId(chat_id), &prompt).await
                        {
                            error!(
                                "Failed to send confirmation prompt to chat {}: {}",
                                chat_id, e
                            );
                            // Remove the one we just added (last in queue)
                            if let Some(mut q) = pending.get_mut(&chat_id) {
                                q.pop_back();
                            }
                        } else {
                            debug!(
                                "Sent confirmation prompt for request {} to chat {}",
                                request_id, chat_id
                            );
                        }
                    }
                    Ok(_) => {} // ignore other events
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Confirmation listener lagged by {} events", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        info!("EventBus closed, confirmation listener exiting");
                        break;
                    }
                }
            }
        });
    }
}

#[async_trait]
impl Connector for TelegramConnector {
    fn name(&self) -> &str {
        "telegram"
    }

    async fn run(&self) -> Result<(), ConnectorError> {
        // Note: The actual run is done via run_with_signal() which consumes self.
        // This trait method cannot consume self, so we just return Ok for now.
        // The startup logic uses run_with_signal() directly.
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ConnectorError> {
        info!("Telegram connector shutdown requested");
        Ok(())
    }
}
