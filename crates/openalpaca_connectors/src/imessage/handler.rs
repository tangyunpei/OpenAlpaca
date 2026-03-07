//! iMessage message handling: routing decisions, attachments, gateway dispatch.

use super::reader::IncomingMessage;
use super::routing::{should_process, IMessageConfig, ProcessDecision};
use super::sender::IMessageSender;
use super::IMessageConnector;
use openalpaca_api::events::EventSource;
use openalpaca_core::{
    gateway::{GatewayRequest, ResolvedAttachment},
    security::policy::{Principal, Scope},
};
use openalpaca_storage::{IdentityRepository, PreferenceRepository};
use tracing::{debug, info, warn};

impl IMessageConnector {
    /// Handle a single incoming iMessage.
    pub(super) async fn handle_message(
        &self,
        msg: IncomingMessage,
        config: &IMessageConfig,
    ) -> Result<(), String> {
        // Compute stable reply target:
        // - DMs: use sender handle (phone/email) — consistent across chat_identifier variants
        // - Groups: use chat_id (group identifier required by AppleScript "chat id")
        let (reply_target, reply_is_group) = if msg.is_group {
            (msg.chat_id.clone(), true)
        } else if !msg.sender.is_empty() {
            (msg.sender.clone(), false)
        } else {
            // Fallback for is_from_me=1 with empty sender
            (msg.chat_id.clone(), false)
        };

        if reply_target != msg.chat_id {
            debug!(
                reply_target = %reply_target,
                chat_id = %msg.chat_id,
                "Reply target differs from chat_id (DM sender-based addressing)"
            );
        }

        // Step 1: Evaluate routing decision
        let content = match should_process(&msg, config) {
            ProcessDecision::Process { content } => {
                info!(
                    chat_id = %msg.chat_id,
                    sender = %msg.sender,
                    is_group = msg.is_group,
                    is_from_me = msg.is_from_me,
                    account = %msg.account,
                    reply_target = %reply_target,
                    "Accepted iMessage for processing"
                );
                content
            }
            ProcessDecision::Skip(reason) => {
                debug!(
                    chat_id = %msg.chat_id,
                    sender = %msg.sender,
                    is_group = msg.is_group,
                    is_from_me = msg.is_from_me,
                    account = %msg.account,
                    reason,
                    "Skipped iMessage"
                );
                // Send usage hint for empty-after-prefix cases
                if reason.ends_with("empty_after_prefix") {
                    IMessageSender::send(
                        &reply_target,
                        "Usage: /ask <your question> or @openalpaca <your question>",
                        reply_is_group,
                    )
                    .await
                    .ok();
                }
                return Ok(());
            }
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

        // If all attachments failed and content is empty, skip to avoid nonsensical response
        if content.is_empty() && attachments.is_empty() && !msg.attachments.is_empty() {
            warn!(
                chat_id = %msg.chat_id,
                "All iMessage attachments failed to store and message content is empty, skipping"
            );
            return Ok(());
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
                stream_id: None,
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

        // Step 3.6: Persist reply metadata for cross-channel delivery / notifications
        let pref_repo = PreferenceRepository::new(&self.db);
        if let Err(e) = pref_repo.set(&global_id, "imessage.last_reply_target", &reply_target, None) {
            warn!("Failed to persist imessage.last_reply_target: {e}");
        }
        if let Err(e) = pref_repo.set(&global_id, "imessage.last_is_group", &reply_is_group.to_string(), None) {
            warn!("Failed to persist imessage.last_is_group: {e}");
        }
        // Persist the receiving account so outbound sends use the same one
        if !msg.account.is_empty()
            && let Err(e) = pref_repo.set(&global_id, "imessage.last_send_account", &msg.account, None)
        {
            warn!("Failed to persist imessage.last_send_account: {e}");
        }

        // Step 4: Send response using stable reply target + the same account that received
        let from_account = if msg.account.is_empty() {
            None
        } else {
            Some(msg.account.as_str())
        };
        IMessageSender::send_from(&reply_target, &response.content, reply_is_group, from_account)
            .await
            .map_err(|e| format!("Send failed: {}", e))?;

        Ok(())
    }

    /// Resolve the macOS owner as a GlobalUser principal.
    ///
    /// Resolution order:
    /// 1. Use the explicit `local_user_id` passed at construction (from daemon bootstrap).
    /// 2. Read `identity.local_user_id` from `system_config` (standalone mode).
    /// 3. Last resort: create a new global user from the macOS username.
    pub(super) fn resolve_owner(&self) -> Result<Principal, String> {
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
}
