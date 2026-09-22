//! Telegram Connector: dispatcher and owned confirmation listener.

use super::delivery::send_with_retry;
use super::ChatRateLimiter;
use crate::common::format_confirmation_prompt;
use crate::startup::RunningGuard;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use openalpaca_core::{
    bus::EventBus, daemon_config::DaemonConfig, events::SystemEvent, gateway::Gateway,
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

    /// Spawn the connector dispatcher and its owned confirmation listener.
    /// Returns a ShutdownToken to stop the dispatcher.
    ///
    /// The `running` flag is wrapped in a `RunningGuard` inside the spawned
    /// task so that `is_alive()` returns `false` once the dispatcher exits.
    pub fn start(
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
        let confirmation_broker: Option<Arc<ConfirmationBroker>> = self.confirmation_broker.clone();
        let pending = self.pending_confirmations.clone();

        let listener_enabled = confirmation_broker.is_some();
        let listener_rx = self.bus.subscribe();
        let listener_bot = self.bot.clone();
        let listener_db = self.db.clone();
        let listener_pending = self.pending_confirmations.clone();
        let listener = async move {
            if listener_enabled {
                Self::listen_for_confirmations(
                    listener_rx,
                    listener_bot,
                    listener_db,
                    listener_pending,
                )
                .await;
            }
        };

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

        spawn_dispatcher(
            async move {
                dispatcher.dispatch().await;
                info!("Telegram connector dispatcher finished");
            },
            listener,
            running,
        );

        token
    }

    /// Listen for `ToolConfirmationRequested`
    /// events targeting Telegram lanes and sends confirmation prompts.
    async fn listen_for_confirmations(
        mut rx: tokio::sync::broadcast::Receiver<SystemEvent>,
        bot: Bot,
        db: Arc<Database>,
        pending: Arc<DashMap<i64, VecDeque<String>>>,
    ) {
        loop {
            match rx.recv().await {
                Ok(SystemEvent::ToolConfirmationRequested {
                    request_id,
                    agent_id: _,
                    tool_name,
                    tool_arguments,
                    stream_id: _,
                    lane_key: Some(ref lane_key),
                    ..
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
                    pending
                        .entry(chat_id)
                        .or_default()
                        .push_back(request_id.clone());
                    let queue_len = pending.get(&chat_id).map(|q| q.len()).unwrap_or(1);

                    let prompt = format_confirmation_prompt(&tool_name, &tool_arguments, queue_len);

                    if let Err(e) = send_with_retry(&bot, ChatId(chat_id), &prompt).await {
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
    }
}

/// Own both futures in one task: returning or cancelling the dispatcher drops
/// the listener, including any in-flight prompt delivery. If the event bus
/// closes first, the dispatcher may continue serving messages.
fn spawn_dispatcher(
    dispatcher: impl std::future::Future<Output = ()> + Send + 'static,
    listener: impl std::future::Future<Output = ()> + Send + 'static,
    running: Arc<std::sync::atomic::AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    running.store(true, std::sync::atomic::Ordering::Release);
    let guard = RunningGuard(running);
    tokio::spawn(async move {
        let _guard = guard;
        tokio::pin!(dispatcher);
        tokio::select! {
            _ = &mut dispatcher => {},
            _ = listener => dispatcher.await,
        }
    })
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    fn listener(dropped: Arc<AtomicBool>) -> impl std::future::Future<Output = ()> + Send {
        let guard = Dropped(dropped);
        async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatcher_completion_drops_listener_and_clears_running() {
        for _ in 0..3 {
            let running = Arc::new(AtomicBool::new(false));
            let dropped = Arc::new(AtomicBool::new(false));
            spawn_dispatcher(async {}, listener(dropped.clone()), running.clone())
                .await
                .unwrap();
            assert!(!running.load(Ordering::Acquire));
            assert!(dropped.load(Ordering::Acquire));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelling_dispatcher_before_first_poll_drops_owned_listener() {
        let running = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let task = spawn_dispatcher(
            std::future::pending(),
            listener(dropped.clone()),
            running.clone(),
        );
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!running.load(Ordering::Acquire));
        assert!(dropped.load(Ordering::Acquire));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn closed_listener_does_not_stop_dispatcher() {
        let running = Arc::new(AtomicBool::new(false));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = spawn_dispatcher(
            async {
                let _ = rx.await;
            },
            async {},
            running.clone(),
        );
        tokio::task::yield_now().await;
        assert!(running.load(Ordering::Acquire));
        assert!(!task.is_finished());
        tx.send(()).unwrap();
        task.await.unwrap();
        assert!(!running.load(Ordering::Acquire));
    }
}
