//! iMessage Connector: struct, lifecycle, config loading, and trait implementation.
//!
//! Handles the integration between macOS iMessage (via chat.db polling
//! and AppleScript sending) and the OpenAlpaca agent system.
//!
//! Unlike bot-based connectors (Telegram), iMessage is a native macOS
//! integration where the Mac owner is always the trusted principal.
//! Messages are optionally filtered by a trigger prefix (`/ask` or `@openalpaca`).
//! Prefix requirements are configurable per-chat-type via `direct_require_prefix`
//! and `group_require_prefix` settings.

use crate::common::format_confirmation_prompt;
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
use openalpaca_storage::{Database, IdentityRepository};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::reader::ChatDbReader;
use super::routing::{normalize_handle, IMessageConfig};
use super::sender::IMessageSender;

/// IMessageConnector manages the iMessage integration lifecycle.
///
/// It polls `~/Library/Messages/chat.db` for new incoming messages and
/// routes them through the OpenAlpaca gateway, sending responses back
/// via AppleScript (`osascript`).
///
/// The macOS system user is always the trusted principal — no `/link`
/// flow is needed.
pub struct IMessageConnector {
    pub(super) db: Arc<Database>,
    bus: Arc<EventBus>,
    pub(super) gateway: Arc<Gateway>,
    pub(super) daemon_config: Arc<ArcSwap<DaemonConfig>>,
    cancel_token: CancellationToken,
    chat_db_path: String,
    pub(super) local_user_id: Option<String>,
    pub(super) confirmation_broker: Option<Arc<ConfirmationBroker>>,
    /// Maps chat identifier -> queue of request_ids for pending tool
    /// confirmations. VecDeque allows FIFO processing when multiple tools
    /// need confirmation. (Same pattern — and same per-conversation-key
    /// caveat — as Telegram.)
    pub(super) pending_confirmations: Arc<DashMap<String, VecDeque<String>>>,
}

impl IMessageConnector {
    /// Create a new IMessageConnector.
    ///
    /// The chat.db path defaults to `~/Library/Messages/chat.db`.
    /// If `local_user_id` is provided, it is used as the principal identity
    /// for all messages (bypassing the heuristic `resolve_owner` fallback).
    pub fn new(
        db: Arc<Database>,
        bus: Arc<EventBus>,
        gateway: Arc<Gateway>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
        cancel_token: CancellationToken,
        local_user_id: Option<String>,
    ) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users".to_string());
        let chat_db_path = format!("{}/Library/Messages/chat.db", home);
        Self {
            db,
            bus,
            gateway,
            daemon_config,
            cancel_token,
            chat_db_path,
            local_user_id,
            confirmation_broker: None,
            pending_confirmations: Arc::new(DashMap::new()),
        }
    }

    /// Attach a confirmation broker for interactive tool approval.
    pub fn with_confirmation_broker(mut self, broker: Arc<ConfirmationBroker>) -> Self {
        self.confirmation_broker = Some(broker);
        self
    }

    /// Main polling loop.
    ///
    /// Restores the persisted ROWID watermark so that messages received
    /// while the connector was offline are still processed. On first run
    /// (no persisted watermark), initializes to the current max ROWID to
    /// avoid replaying the entire history.
    pub async fn run_loop(&self) -> Result<(), ConnectorError> {
        info!("Starting iMessage connector...");

        // Spawn confirmation listener (if broker available)
        if self.confirmation_broker.is_some() {
            self.spawn_confirmation_listener();
        }

        let mut reader =
            ChatDbReader::new(&self.chat_db_path).map_err(ConnectorError::InitFailed)?;

        // Restore persisted watermark, or initialize to current max ROWID
        let config_repo = openalpaca_storage::ConfigRepository::new(&self.db);
        let persisted_watermark = config_repo
            .get("imessage.last_rowid")
            .ok()
            .flatten()
            .and_then(|v| match v.parse::<i64>() {
                Ok(n) => Some(n),
                Err(e) => {
                    warn!("Corrupt imessage.last_rowid '{v}': {e}, re-initializing");
                    None
                }
            });

        if let Some(watermark) = persisted_watermark {
            reader.set_watermark(watermark);
            info!(
                "iMessage connector restored watermark to ROWID {}",
                watermark
            );
        } else {
            reader
                .initialize_watermark()
                .map_err(ConnectorError::InitFailed)?;
            info!("iMessage connector initialized, watermark set to current max ROWID");
        }
        // Log initial config on startup
        let initial_config = Self::load_imessage_config(&self.db);
        info!(
            allow_from_me = initial_config.allow_from_me,
            direct_require_prefix = initial_config.direct_require_prefix,
            group_require_prefix = initial_config.group_require_prefix,
            owner_handles_count = initial_config.owner_handles.len(),
            bot_handle = initial_config.bot_handle.as_deref().unwrap_or("(not set)"),
            "iMessage connector started with routing config"
        );

        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    info!("iMessage connector shutting down");
                    Self::persist_watermark(&self.db, reader.watermark());
                    return Ok(());
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                    // Re-read config each poll cycle so GUI/CLI changes take effect immediately
                    let imsg_config = Self::load_imessage_config(&self.db);

                    match reader.poll_new_messages(imsg_config.allow_from_me) {
                        Ok(messages) => {
                            let had_messages = !messages.is_empty();
                            for msg in messages {
                                if let Err(e) = self.handle_message(msg, &imsg_config).await {
                                    tracing::error!("Failed to handle iMessage: {}", e);
                                }
                            }
                            // Persist watermark after processing so offline
                            // messages are not lost on restart.
                            if had_messages {
                                Self::persist_watermark(&self.db, reader.watermark());
                            }
                        }
                        Err(e) => {
                            warn!("Failed to poll chat.db: {}", e);
                        }
                    }
                }
            }
        }
    }

    /// Spawn a background task that listens for `ToolConfirmationRequested`
    /// events targeting iMessage lanes and sends confirmation prompts.
    fn spawn_confirmation_listener(&self) {
        let mut rx = self.bus.subscribe();
        let db = self.db.clone();
        let pending = self.pending_confirmations.clone();
        let cancel = self.cancel_token.clone();
        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = cancel.cancelled() => {
                        info!("iMessage confirmation listener shutting down");
                        break;
                    }
                    event = rx.recv() => event,
                };
                match event {
                    Ok(SystemEvent::ToolConfirmationRequested {
                        request_id,
                        tool_name,
                        tool_arguments,
                        lane_key: Some(ref lane_key),
                        ..
                    }) if lane_key.ends_with(":imessage") => {
                        // Resolve the chat identifier from lane_key via the
                        // conversation_map (written on every inbound message).
                        let chat_id = IdentityRepository::new(&db)
                            .get_conversation_id_str_by_lane_key(lane_key, "imessage")
                            .ok()
                            .flatten();

                        let Some(chat_id) = chat_id else {
                            warn!(
                                "Could not resolve iMessage chat for lane_key={}, skipping confirmation",
                                lane_key
                            );
                            continue;
                        };

                        // Derive send addressing: groups use chat-id addressing,
                        // DMs use the sender handle (same as reply_target logic).
                        let (target, is_group) = super::routing::confirmation_reply_target(&chat_id);

                        // Store pending confirmation mapping (queue per chat)
                        pending
                            .entry(chat_id.clone())
                            .or_default()
                            .push_back(request_id.clone());
                        let queue_len = pending.get(&chat_id).map(|q| q.len()).unwrap_or(1);

                        let prompt =
                            format_confirmation_prompt(&tool_name, &tool_arguments, queue_len);

                        if let Err(e) = IMessageSender::send(&target, &prompt, is_group).await {
                            error!(
                                "Failed to send confirmation prompt to iMessage chat {}: {}",
                                chat_id, e
                            );
                            // Remove the one we just added (last in queue)
                            if let Some(mut q) = pending.get_mut(&chat_id) {
                                q.pop_back();
                            }
                        } else {
                            debug!(
                                "Sent confirmation prompt for request {} to iMessage chat {}",
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

    /// Persist the ROWID watermark to the database.
    fn persist_watermark(db: &Database, watermark: i64) {
        let config_repo = openalpaca_storage::ConfigRepository::new(db);
        if let Err(e) = config_repo.set(
            "imessage.last_rowid",
            &watermark.to_string(),
            "int",
        ) {
            warn!("Failed to persist iMessage watermark: {e}");
        }
    }

    /// Load iMessage routing configuration from the database.
    ///
    /// Called on each poll cycle so that changes made via the GUI or CLI
    /// take effect without restarting the connector.
    pub(super) fn load_imessage_config(db: &Database) -> IMessageConfig {
        let config_repo = openalpaca_storage::ConfigRepository::new(db);

        let allow_from_me = config_repo
            .get_or_default("imessage.allow_from_me")
            .ok()
            .flatten()
            .map(|v| matches!(v.as_str(), "true" | "1" | "yes"))
            .unwrap_or(true);

        let direct_require_prefix = config_repo
            .get_or_default("imessage.direct_require_prefix")
            .ok()
            .flatten()
            .map(|v| matches!(v.as_str(), "true" | "1" | "yes"))
            .unwrap_or(false);

        // Canonical values: "true"/"false". Also accepts "1"/"yes" for convenience.
        let group_require_prefix = config_repo
            .get_or_default("imessage.group_require_prefix")
            .ok()
            .flatten()
            .map(|v| matches!(v.as_str(), "true" | "1" | "yes"))
            .unwrap_or(true);

        let mut owner_handles: Vec<String> = config_repo
            .get("imessage.owner_handles")
            .ok()
            .flatten()
            .map(|v| {
                v.split(',')
                    .map(normalize_handle)
                    .filter(|h| !h.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let bot_handle = config_repo
            .get("imessage.bot_handle")
            .ok()
            .flatten()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        // Auto-populate owner_handles from bot_handle when empty.
        // This ensures that self-sent messages (e.g. from iPhone) are recognized
        // as owner messages without requiring explicit owner_handles configuration.
        if owner_handles.is_empty()
            && let Some(ref bh) = bot_handle
        {
            let normalized = normalize_handle(bh);
            if !normalized.is_empty() {
                owner_handles.push(normalized);
            }
        }

        // Safety: force-disable allow_from_me when bot_handle is missing
        // to prevent feedback loops (bot processes its own replies)
        let allow_from_me = if bot_handle.is_none() && allow_from_me {
            use std::sync::atomic::{AtomicBool, Ordering};
            static WARNED: AtomicBool = AtomicBool::new(false);
            if !WARNED.swap(true, Ordering::Relaxed) {
                warn!(
                    "imessage.bot_handle not configured — forcing allow_from_me=false to prevent feedback loop. \
                     Set imessage.bot_handle to the Mac's iMessage sending address to enable allow_from_me."
                );
            }
            false
        } else {
            allow_from_me
        };

        IMessageConfig {
            allow_from_me,
            direct_require_prefix,
            group_require_prefix,
            owner_handles,
            bot_handle,
        }
    }
}

#[async_trait]
impl Connector for IMessageConnector {
    fn name(&self) -> &str {
        "imessage"
    }

    async fn run(&self) -> Result<(), ConnectorError> {
        self.run_loop().await
    }

    async fn shutdown(&self) -> Result<(), ConnectorError> {
        info!("iMessage connector shutdown requested");
        self.cancel_token.cancel();
        Ok(())
    }
}
