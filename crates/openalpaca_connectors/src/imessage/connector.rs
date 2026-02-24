//! iMessage Connector Implementation
//!
//! Handles the integration between macOS iMessage (via chat.db polling
//! and AppleScript sending) and the OpenAlpaca agent system.

use crate::common::{format_denial_message, handle_link_token, resolve_principal, LinkResult};
use crate::{Connector, ConnectorError};
use async_trait::async_trait;
use openalpaca_api::events::EventSource;
use openalpaca_core::{
    bus::EventBus,
    gateway::{Gateway, GatewayRequest},
    security::policy::Scope,
    types::Capability,
};
use openalpaca_storage::{Database, IdentityRepository, PreferenceRepository};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::reader::{ChatDbReader, IncomingMessage};
use super::sender::IMessageSender;

/// IMessageConnector manages the iMessage integration lifecycle.
///
/// It polls `~/Library/Messages/chat.db` for new incoming messages and
/// routes them through the OpenAlpaca gateway, sending responses back
/// via AppleScript (`osascript`).
pub struct IMessageConnector {
    db: Arc<Database>,
    bus: Arc<EventBus>,
    gateway: Arc<Gateway>,
    cancel_token: CancellationToken,
    chat_db_path: String,
}

impl IMessageConnector {
    /// Create a new IMessageConnector.
    ///
    /// The chat.db path defaults to `~/Library/Messages/chat.db`.
    pub fn new(
        db: Arc<Database>,
        bus: Arc<EventBus>,
        gateway: Arc<Gateway>,
        cancel_token: CancellationToken,
    ) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users".to_string());
        let chat_db_path = format!("{}/Library/Messages/chat.db", home);
        Self {
            db,
            bus,
            gateway,
            cancel_token,
            chat_db_path,
        }
    }

    /// Main polling loop.
    ///
    /// Initialises the ROWID watermark to the current maximum so that
    /// historical messages are skipped, then polls every 2 seconds for
    /// new incoming messages.
    pub async fn run_loop(&self) -> Result<(), ConnectorError> {
        info!("Starting iMessage connector...");
        let mut reader =
            ChatDbReader::new(&self.chat_db_path).map_err(ConnectorError::InitFailed)?;

        // Initialize watermark to current max ROWID
        reader
            .initialize_watermark()
            .map_err(ConnectorError::InitFailed)?;
        info!("iMessage connector initialized, watermark set");

        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    info!("iMessage connector shutting down");
                    return Ok(());
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                    match reader.poll_new_messages() {
                        Ok(messages) => {
                            for msg in messages {
                                if let Err(e) = self.handle_message(msg).await {
                                    error!("Failed to handle iMessage: {}", e);
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

    /// Handle a single incoming iMessage.
    async fn handle_message(&self, msg: IncomingMessage) -> Result<(), String> {
        info!(
            "Received iMessage from {} in chat {}",
            msg.sender, msg.chat_id
        );

        // Step 1: Resolve principal
        let identity_repo = IdentityRepository::new(&self.db);
        let (principal, external_identity_id) =
            resolve_principal(&identity_repo, "imessage", &msg.sender, None)?;

        // Step 2: Handle /link commands
        if msg.text.starts_with("/link ") {
            let token = msg.text.strip_prefix("/link ").unwrap().trim();
            match handle_link_token(&identity_repo, token, external_identity_id) {
                Ok(LinkResult::Success(global_user_id)) => {
                    IMessageSender::send(
                        &msg.chat_id,
                        &format!("Account linked to {}", global_user_id),
                        msg.is_group,
                    )
                    .await
                    .ok();
                }
                Ok(LinkResult::InvalidToken) => {
                    IMessageSender::send(
                        &msg.chat_id,
                        "Invalid or expired token.",
                        msg.is_group,
                    )
                    .await
                    .ok();
                }
                Err(e) => {
                    IMessageSender::send(
                        &msg.chat_id,
                        &format!("Error: {}", e),
                        msg.is_group,
                    )
                    .await
                    .ok();
                }
            }
            return Ok(());
        }

        // Step 3: TrustGate pre-check
        let capability = Capability {
            name: "chat.respond".to_string(),
        };
        let scope = Scope::Conversation {
            id: msg.chat_id.clone(),
        };

        if let Err(e) =
            openalpaca_core::security::policy::TrustGate::check(&principal, &capability, &scope)
        {
            warn!("TrustGate denied iMessage request: {}", e);
            IMessageSender::send(&msg.chat_id, &format_denial_message(&e), msg.is_group)
                .await
                .ok();
            return Ok(());
        }

        // Extract global_id before principal is consumed by gateway
        let global_id_for_pref = match &principal {
            openalpaca_core::security::policy::Principal::User { global_id } => {
                Some(global_id.clone())
            }
            _ => None,
        };

        // Step 4: Route through Gateway
        let response = self
            .gateway
            .handle_event(GatewayRequest {
                source: EventSource::IMessage {
                    chat_id: msg.chat_id.clone(),
                    sender: msg.sender.clone(),
                },
                content: msg.text.clone(),
                principal,
                scope: Scope::Conversation {
                    id: msg.chat_id.clone(),
                },
                workspace_path: None,
            })
            .await;

        // Step 4.5: Map external chat_id to internal lane_key
        let lane_key = response.lane_key.to_string();
        if let Err(e) = identity_repo.update_conversation_map_lane_key(
            "imessage",
            &msg.chat_id,
            &lane_key,
        ) {
            warn!("Failed to update conversation_map lane_key: {e}");
        }

        // Step 4.6: Persist imessage.last_chat_id for cross-channel delivery
        if let Some(ref gid) = global_id_for_pref {
            let pref_repo = PreferenceRepository::new(&self.db);
            if let Err(e) = pref_repo.set(gid, "imessage.last_chat_id", &msg.chat_id, None) {
                warn!("Failed to persist imessage.last_chat_id: {e}");
            }
        }

        // Step 5: Send response
        IMessageSender::send(&msg.chat_id, &response.content, msg.is_group)
            .await
            .map_err(|e| format!("Send failed: {}", e))?;

        let _ = &self.bus; // Keep bus in scope for future event emission
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
