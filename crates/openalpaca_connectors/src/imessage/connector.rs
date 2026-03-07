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

use crate::{Connector, ConnectorError};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use openalpaca_core::{
    daemon_config::DaemonConfig,
    gateway::Gateway,
};
use openalpaca_storage::Database;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::reader::ChatDbReader;
use super::routing::{normalize_handle, IMessageConfig};

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
    pub(super) gateway: Arc<Gateway>,
    pub(super) daemon_config: Arc<ArcSwap<DaemonConfig>>,
    cancel_token: CancellationToken,
    chat_db_path: String,
    pub(super) local_user_id: Option<String>,
}

impl IMessageConnector {
    /// Create a new IMessageConnector.
    ///
    /// The chat.db path defaults to `~/Library/Messages/chat.db`.
    /// If `local_user_id` is provided, it is used as the principal identity
    /// for all messages (bypassing the heuristic `resolve_owner` fallback).
    pub fn new(
        db: Arc<Database>,
        gateway: Arc<Gateway>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
        cancel_token: CancellationToken,
        local_user_id: Option<String>,
    ) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users".to_string());
        let chat_db_path = format!("{}/Library/Messages/chat.db", home);
        Self {
            db,
            gateway,
            daemon_config,
            cancel_token,
            chat_db_path,
            local_user_id,
        }
    }

    /// Main polling loop.
    ///
    /// Restores the persisted ROWID watermark so that messages received
    /// while the connector was offline are still processed. On first run
    /// (no persisted watermark), initializes to the current max ROWID to
    /// avoid replaying the entire history.
    pub async fn run_loop(&self) -> Result<(), ConnectorError> {
        info!("Starting iMessage connector...");
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
    fn name(&self) -> &'static str {
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
