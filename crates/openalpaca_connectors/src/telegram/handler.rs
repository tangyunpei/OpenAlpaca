//! Telegram message handling: dispatch, link/unlink commands, attachments.

use super::delivery::{download_telegram_file, send_with_retry};
use super::rate_limiter::ChatRateLimiter;
use super::TelegramConnector;
use crate::common::{
    LinkResult, format_denial_message, handle_link_token, redact_token, resolve_principal,
};
use arc_swap::ArcSwap;
use dashmap::DashMap;
use openalpaca_api::events::EventSource;
use openalpaca_core::{
    bus::EventBus,
    daemon_config::DaemonConfig,
    gateway::{Gateway, GatewayRequest, ResolvedAttachment},
    security::confirmation::{ConfirmationBroker, ConfirmationResponse},
    security::policy::Scope,
    types::Capability,
};
use openalpaca_storage::{Database, IdentityRepository, PreferenceRepository};
use std::collections::VecDeque;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ChatAction;
use tracing::{error, info, warn};

impl TelegramConnector {
    /// Handle an incoming Telegram message.
    pub(super) async fn handle_message(
        bot: Bot,
        msg: Message,
        db: Arc<Database>,
        bus: Arc<EventBus>,
        gateway: Arc<Gateway>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
        rate_limiter: Arc<ChatRateLimiter>,
        confirmation_broker: Option<Arc<ConfirmationBroker>>,
        pending_confirmations: Arc<DashMap<i64, VecDeque<String>>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let text = msg.text().unwrap_or("").to_string();

        let chat_id = msg.chat.id;
        let from = match msg.from.as_ref() {
            Some(user) => user,
            None => {
                // Channel posts, anonymous admin messages, etc. have no sender.
                // We cannot resolve identity without a user ID, so skip.
                return Ok(());
            }
        };
        let user_id = from.id.0.to_string();
        let display_name = Some(from.first_name.clone());

        info!(
            "Received message from Telegram user {} in chat {}: {}",
            user_id,
            chat_id,
            text.chars().take(50).collect::<String>()
        );

        // Intercept confirmation responses (/yes, /y, /no, /n)
        let text_lower = text.trim().to_lowercase();
        if matches!(text_lower.as_str(), "/yes" | "/y" | "/no" | "/n") {
            if let Some(broker) = confirmation_broker.as_ref() {
                let request_id = pending_confirmations
                    .get_mut(&chat_id.0)
                    .and_then(|mut q| q.pop_front());

                if let Some(request_id) = request_id {
                    let approved = matches!(text_lower.as_str(), "/yes" | "/y");
                    let remaining = pending_confirmations
                        .get(&chat_id.0)
                        .map(|q| q.len())
                        .unwrap_or(0);
                    let reply = if approved {
                        if remaining > 0 {
                            format!("Approved. Tool execution will proceed.\n({} more pending — reply /yes or /no)", remaining)
                        } else {
                            "Approved. Tool execution will proceed.".to_string()
                        }
                    } else if remaining > 0 {
                        format!("Denied. Tool execution has been cancelled.\n({} more pending — reply /yes or /no)", remaining)
                    } else {
                        "Denied. Tool execution has been cancelled.".to_string()
                    };

                    match broker.respond(
                        &request_id,
                        ConfirmationResponse {
                            approved,
                            approval_scope: None,
                        },
                    ) {
                        Ok(()) => {
                            info!(
                                "Confirmation {} for request {} in chat {}",
                                if approved { "approved" } else { "denied" },
                                request_id,
                                chat_id
                            );
                        }
                        Err(e) => {
                            warn!("Failed to deliver confirmation response: {}", e);
                        }
                    }

                    bot.send_message(chat_id, &reply).await?;
                    return Ok(());
                }
                // No pending confirmation — fall through to normal handling
            }
        }

        // Check rate limiter
        if let Some(wait) = rate_limiter.check(chat_id.0) {
            warn!("Rate limited chat {}, need to wait {:?}", chat_id, wait);
            bot.send_message(
                chat_id,
                "Please wait a moment before sending another message.",
            )
            .await?;
            return Ok(());
        }

        // Step 1: Resolve Principal
        let identity_repo = IdentityRepository::new(&db);
        let (principal, external_identity_id) = resolve_principal(
            &identity_repo,
            "telegram",
            &user_id,
            display_name.as_deref(),
        )?;

        let is_linked = matches!(
            principal,
            openalpaca_core::security::policy::Principal::User { .. }
        );
        if is_linked {
            info!("User {} is linked (Trusted)", user_id);
        } else {
            info!("User {} is NOT linked (Untrusted)", user_id);
        }

        // Step 2: Handle /link and /unlink commands (connector-specific)
        if text.starts_with("/link ") {
            let token = text.strip_prefix("/link ").unwrap().trim();
            return Self::handle_link_command(
                &bot,
                chat_id,
                &user_id,
                token,
                external_identity_id,
                &identity_repo,
            )
            .await;
        } else if text == "/unlink" || text == "/unbind" {
            return Self::handle_unlink_command(
                &bot,
                chat_id,
                &user_id,
                external_identity_id,
                &identity_repo,
            )
            .await;
        }

        // Step 3: Pre-check TrustGate (for early denial message to user)
        let capability = Capability {
            name: "chat.respond".to_string(),
        };
        let scope = Scope::Conversation {
            id: chat_id.0.to_string(),
        };

        if let Err(e) =
            openalpaca_core::security::policy::TrustGate::check(&principal, &capability, &scope)
        {
            warn!("TrustGate denied request: {}", e);
            bot.send_message(chat_id, format_denial_message(&e)).await?;
            return Ok(());
        }

        // Extract global_id before principal is consumed by gateway
        let global_id_for_pref = match &principal {
            openalpaca_core::security::policy::Principal::User { global_id } => {
                Some(global_id.clone())
            }
            _ => None,
        };

        // Send typing indicator
        if let Err(e) = bot.send_chat_action(chat_id, ChatAction::Typing).await {
            warn!("Failed to send typing indicator: {}", e);
        }

        // Step 3.5: Handle photo and document attachments
        let owner_id = match &global_id_for_pref {
            Some(gid) => gid.clone(),
            None => user_id.clone(),
        };
        let mut attachments: Vec<ResolvedAttachment> = Vec::new();
        let upload_cfg = daemon_config.load();
        let max_file_size = upload_cfg.upload.max_file_size_bytes;
        let max_img_dim = upload_cfg.upload.governance.max_image_dimension;

        // Handle photos — msg.photo() returns Option<&[PhotoSize]>
        if let Some(photos) = msg.photo()
            && let Some(largest) = photos.last()
        {
            // sorted by size, largest last
            match download_telegram_file(&bot, &largest.file.id.0).await {
                Ok(data) => {
                    let uid = &largest.file.unique_id.0;
                    let suffix = &uid[..8.min(uid.len())];
                    let fname = format!("photo_{suffix}.jpg");
                    match crate::common::store_attachment(
                        &db,
                        &owner_id,
                        &fname,
                        "image/jpeg",
                        &data,
                        max_file_size,
                        max_img_dim,
                    ) {
                        Ok(att) => attachments.push(att),
                        Err(e) => warn!("Failed to store telegram photo: {e}"),
                    }
                }
                Err(e) => warn!("Failed to download telegram photo: {e}"),
            }
        }

        // Handle documents — msg.document() returns Option<&Document>
        if let Some(doc) = msg.document() {
            let file_name = doc.file_name.as_deref().unwrap_or("document");
            let mime = doc
                .mime_type
                .as_ref()
                .map(|m| m.to_string())
                .unwrap_or_else(|| "application/octet-stream".into());
            match download_telegram_file(&bot, &doc.file.id.0).await {
                Ok(data) => {
                    match crate::common::store_attachment(
                        &db,
                        &owner_id,
                        file_name,
                        &mime,
                        &data,
                        max_file_size,
                        max_img_dim,
                    ) {
                        Ok(att) => attachments.push(att),
                        Err(e) => warn!("Failed to store telegram doc: {e}"),
                    }
                }
                Err(e) => warn!("Failed to download telegram doc: {e}"),
            }
        }

        // Skip if nothing useful
        if text.is_empty() && attachments.is_empty() {
            return Ok(());
        }

        // Step 4: Route through Gateway (replaces manual pipeline)
        let response = gateway
            .handle_event(GatewayRequest {
                source: EventSource::Telegram {
                    chat_id: chat_id.0.to_string(),
                    user_id: user_id.clone(),
                },
                content: text.clone(),
                attachments,
                principal,
                scope: Scope::Conversation {
                    id: chat_id.0.to_string(),
                },
                workspace_path: None, // Telegram has no workspace context; uses Global scope only
                stream_id: None,
                lane_override: None,
            })
            .await;

        // Step 4.5: Map external Telegram chat_id to internal lane_key
        let lane_key = response.lane_key.to_string();
        if let Err(e) = identity_repo.update_conversation_map_lane_key(
            "telegram",
            &chat_id.0.to_string(),
            &lane_key,
        ) {
            warn!("Failed to update conversation_map lane_key: {e}");
        }

        // Step 5.2: Persist telegram.last_chat_id for cross-channel delivery
        if let Some(ref gid) = global_id_for_pref {
            let pref_repo = PreferenceRepository::new(&db);
            if let Err(e) =
                pref_repo.set(gid, "telegram.last_chat_id", &chat_id.0.to_string(), None)
            {
                warn!("Failed to persist telegram.last_chat_id: {e}");
            }
        }

        // Step 5: Send response back to Telegram with retry and chunking
        if let Err(e) = send_with_retry(&bot, chat_id, &response.content).await {
            error!(
                "Failed to send response to Telegram chat {}: {}",
                chat_id, e
            );
        }

        // Note: EventBus events (UserRequest + AgentResponse) are now emitted
        // by Gateway and the MessageHandler, not by the connector.
        let _ = bus; // Keep bus in deps for /link events if needed

        Ok(())
    }

    /// Handle the /link command.
    pub(super) async fn handle_link_command(
        bot: &Bot,
        chat_id: ChatId,
        user_id: &str,
        token: &str,
        external_identity_id: i64,
        identity_repo: &IdentityRepository<'_>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!(
            "Processing /link command for user {} with token {}",
            user_id,
            redact_token(token)
        );

        match handle_link_token(identity_repo, token, external_identity_id) {
            Ok(LinkResult::Success(global_user_id)) => {
                info!(
                    "Successfully linked telegram:{} -> global_user:{}",
                    user_id, global_user_id
                );

                // Migrate old lane to global lane
                if let Err(e) = identity_repo.migrate_lane_on_link(
                    user_id,
                    &global_user_id,
                    "telegram",
                    &chat_id.0.to_string(),
                ) {
                    warn!("Lane migration failed: {e}");
                }

                bot.send_message(
                    chat_id,
                    format!(
                        "✅ Account linked successfully!\nYou are now connected as `{}`.",
                        global_user_id
                    ),
                )
                .await?;
            }
            Ok(LinkResult::InvalidToken) => {
                warn!("Invalid/expired link token: {}", redact_token(token));
                bot.send_message(
                    chat_id,
                    "❌ Invalid or expired token.\nPlease generate a new token from the GUI or CLI.",
                )
                .await?;
            }
            Err(e) => {
                error!("Error consuming link token: {}", e);
                bot.send_message(chat_id, format!("❌ Error: {}", e))
                    .await?;
            }
        }

        Ok(())
    }
    /// Handle the /unlink command.
    pub(super) async fn handle_unlink_command(
        bot: &Bot,
        chat_id: ChatId,
        user_id: &str,
        external_identity_id: i64,
        identity_repo: &IdentityRepository<'_>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Processing /unlink command for user {}", user_id);

        match identity_repo.unlink_external_identity(external_identity_id) {
            Ok(_) => {
                info!("Successfully unlinked telegram:{}", user_id);
                bot.send_message(
                    chat_id,
                    "✅ Account unlinked successfully.\nYou are now acting as an anonymous/external user.",
                )
                .await?;
            }
            Err(e) => {
                error!("Error unlinking identity: {}", e);
                bot.send_message(chat_id, format!("❌ Error: {}", e))
                    .await?;
            }
        }
        Ok(())
    }
}
