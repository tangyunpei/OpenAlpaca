//! iMessage Connector Implementation
//!
//! Handles the integration between macOS iMessage (via chat.db polling
//! and AppleScript sending) and the OpenAlpaca agent system.
//!
//! Unlike bot-based connectors (Telegram), iMessage is a native macOS
//! integration where the Mac owner is always the trusted principal.
//! Messages must start with a trigger prefix (`/ask` or `@openalpaca`)
//! to be processed.

use crate::{Connector, ConnectorError};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use openalpaca_api::events::EventSource;
use openalpaca_core::{
    daemon_config::DaemonConfig,
    gateway::{Gateway, GatewayRequest, ResolvedAttachment},
    security::policy::{Principal, Scope},
};
use openalpaca_storage::{Database, IdentityRepository, PreferenceRepository};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::reader::{ChatDbReader, IncomingMessage};
use super::sender::IMessageSender;

/// Built-in trigger prefixes. A message must start with one of these to be processed.
const TRIGGER_PREFIXES: &[&str] = &["/ask", "@openalpaca"];

/// Check if the message starts with any trigger prefix.
/// Returns the content after the prefix (trimmed), or None if no prefix matched.
fn strip_trigger_prefix(text: &str) -> Option<String> {
    for prefix in TRIGGER_PREFIXES {
        if let Some(rest) = text.strip_prefix(prefix) {
            let content = rest.trim_start().to_string();
            return Some(content);
        }
    }
    None
}

/// IMessageConnector manages the iMessage integration lifecycle.
///
/// It polls `~/Library/Messages/chat.db` for new incoming messages and
/// routes them through the OpenAlpaca gateway, sending responses back
/// via AppleScript (`osascript`).
///
/// The macOS system user is always the trusted principal — no `/link`
/// flow is needed.
pub struct IMessageConnector {
    db: Arc<Database>,
    #[allow(dead_code)]
    bus: Arc<openalpaca_core::bus::EventBus>,
    gateway: Arc<Gateway>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    cancel_token: CancellationToken,
    chat_db_path: String,
    local_user_id: Option<String>,
}

impl IMessageConnector {
    /// Create a new IMessageConnector.
    ///
    /// The chat.db path defaults to `~/Library/Messages/chat.db`.
    /// If `local_user_id` is provided, it is used as the principal identity
    /// for all messages (bypassing the heuristic `resolve_owner` fallback).
    pub fn new(
        db: Arc<Database>,
        bus: Arc<openalpaca_core::bus::EventBus>,
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
            .and_then(|v| v.parse::<i64>().ok());

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

        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    info!("iMessage connector shutting down");
                    // Persist watermark on clean shutdown so offline messages
                    // are not lost on restart.
                    let config_repo = openalpaca_storage::ConfigRepository::new(&self.db);
                    if let Err(e) = config_repo.set(
                        "imessage.last_rowid",
                        &reader.watermark().to_string(),
                        "int",
                    ) {
                        warn!("Failed to persist iMessage watermark on shutdown: {e}");
                    }
                    return Ok(());
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                    match reader.poll_new_messages() {
                        Ok(messages) => {
                            let had_messages = !messages.is_empty();
                            for msg in messages {
                                if let Err(e) = self.handle_message(msg).await {
                                    error!("Failed to handle iMessage: {}", e);
                                }
                            }
                            // Persist watermark after processing so offline
                            // messages are not lost on restart.
                            if had_messages {
                                let config_repo = openalpaca_storage::ConfigRepository::new(&self.db);
                                if let Err(e) = config_repo.set(
                                    "imessage.last_rowid",
                                    &reader.watermark().to_string(),
                                    "int",
                                ) {
                                    warn!("Failed to persist iMessage watermark: {e}");
                                }
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

    /// Resolve the macOS owner as a GlobalUser principal.
    ///
    /// Resolution order:
    /// 1. Use the explicit `local_user_id` passed at construction (from daemon bootstrap).
    /// 2. Read `identity.local_user_id` from `system_config` (standalone mode).
    /// 3. Last resort: create a new global user from the macOS username.
    fn resolve_owner(&self) -> Result<Principal, String> {
        // 1. Prefer explicit local_user_id from daemon bootstrap
        if let Some(ref id) = self.local_user_id {
            return Ok(Principal::User {
                global_id: id.clone(),
            });
        }

        // 2. Fallback: read from system_config (standalone mode)
        let config_repo = openalpaca_storage::ConfigRepository::new(&self.db);
        if let Ok(Some(id)) = config_repo.get("identity.local_user_id") {
            return Ok(Principal::User { global_id: id });
        }

        // 3. Last resort: create from macOS username
        let identity_repo = IdentityRepository::new(&self.db);
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "mac-owner".to_string());
        let global_id = uuid::Uuid::new_v4().to_string();

        identity_repo
            .create_global_user(&global_id, Some(&username))
            .map_err(|e| format!("Failed to create global user: {e}"))?;

        Ok(Principal::User { global_id })
    }

    /// Handle a single incoming iMessage.
    async fn handle_message(&self, msg: IncomingMessage) -> Result<(), String> {
        info!(
            "Received iMessage from {} in chat {}",
            msg.sender, msg.chat_id
        );

        // Step 1: Check trigger prefix — skip messages that don't match
        let has_attachments = !msg.attachments.is_empty();
        let content = match strip_trigger_prefix(&msg.text) {
            Some(c) if !c.is_empty() => c,
            Some(_) if has_attachments => String::new(), // trigger with attachments only
            Some(_) => {
                IMessageSender::send(
                    &msg.chat_id,
                    "Usage: /ask <your question> or @openalpaca <your question>",
                    msg.is_group,
                )
                .await
                .ok();
                return Ok(());
            }
            None => return Ok(()), // silently skip non-triggered messages
        };

        // Step 2: Resolve macOS owner as principal (always Principal::User)
        let principal = self.resolve_owner()?;

        let global_id = match &principal {
            Principal::User { global_id } => global_id.clone(),
            _ => unreachable!("resolve_owner always returns Principal::User"),
        };

        // Step 2.5: Store attachments
        let mut attachments: Vec<ResolvedAttachment> = Vec::new();
        let upload_cfg = self.daemon_config.load();
        let max_file_size = upload_cfg.upload.max_file_size_bytes;
        let max_img_dim = upload_cfg.upload.governance.max_image_dimension;
        for att in &msg.attachments {
            let path = std::path::Path::new(&att.file_path);
            if !path.exists() {
                warn!("iMessage attachment file not found: {}", att.file_path);
                continue;
            }
            match std::fs::read(path) {
                Ok(data) => {
                    let name = if att.transfer_name.is_empty() {
                        path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "attachment".into())
                    } else {
                        att.transfer_name.clone()
                    };
                    match crate::common::store_attachment(
                        &self.db,
                        &global_id,
                        &name,
                        &att.mime_type,
                        &data,
                        max_file_size,
                        max_img_dim,
                    ) {
                        Ok(resolved) => attachments.push(resolved),
                        Err(e) => warn!("Failed to store iMessage attachment: {e}"),
                    }
                }
                Err(e) => warn!("Failed to read iMessage attachment {}: {e}", att.file_path),
            }
        }

        // Step 3: Route through Gateway
        let response = self
            .gateway
            .handle_event(GatewayRequest {
                source: EventSource::IMessage {
                    chat_id: msg.chat_id.clone(),
                    sender: msg.sender.clone(),
                },
                content,
                attachments,
                principal,
                scope: Scope::Conversation {
                    id: msg.chat_id.clone(),
                },
                workspace_path: None,
            })
            .await;

        // Step 3.5: Map external chat_id to internal lane_key
        let identity_repo = IdentityRepository::new(&self.db);
        let lane_key = response.lane_key.to_string();
        if let Err(e) =
            identity_repo.update_conversation_map_lane_key("imessage", &msg.chat_id, &lane_key)
        {
            warn!("Failed to update conversation_map lane_key: {e}");
        }

        // Step 3.6: Persist imessage.last_chat_id for cross-channel delivery
        let pref_repo = PreferenceRepository::new(&self.db);
        if let Err(e) = pref_repo.set(&global_id, "imessage.last_chat_id", &msg.chat_id, None) {
            warn!("Failed to persist imessage.last_chat_id: {e}");
        }

        // Step 4: Send response
        IMessageSender::send(&msg.chat_id, &response.content, msg.is_group)
            .await
            .map_err(|e| format!("Send failed: {}", e))?;

        Ok(())
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
