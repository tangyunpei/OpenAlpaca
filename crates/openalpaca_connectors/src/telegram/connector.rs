//! Telegram Connector Implementation
//!
//! Handles the integration between Telegram Bot API and the OpenAlpaca agent system.

use crate::common::{LinkResult, format_denial_message, handle_link_token, resolve_principal};
use crate::{Connector, ConnectorError};
use async_trait::async_trait;
use openalpaca_api::events::EventSource;
use openalpaca_core::{
    bus::EventBus,
    gateway::{Gateway, GatewayRequest},
    security::policy::Scope,
    types::Capability,
};
use openalpaca_storage::{Database, IdentityRepository};
use std::sync::Arc;
use teloxide::prelude::*;
use tracing::{error, info, warn};

/// TelegramConnector manages the Telegram bot lifecycle and message handling.
pub struct TelegramConnector {
    bot: Bot,
    db: Arc<Database>,
    bus: Arc<EventBus>,
    gateway: Arc<Gateway>,
}

impl TelegramConnector {
    /// Create a new TelegramConnector with the given bot token.
    pub fn new(token: String, db: Arc<Database>, bus: Arc<EventBus>, gateway: Arc<Gateway>) -> Self {
        let bot = Bot::new(token);
        Self {
            bot,
            db,
            bus,
            gateway,
        }
    }

    /// Start the connector (blocking, runs the teloxide dispatcher).
    /// Returns a ShutdownToken to stop the dispatcher.
    pub async fn run_with_signal(self) -> teloxide::dispatching::ShutdownToken {
        info!("Starting Telegram Connector...");

        let handler = Update::filter_message().endpoint(Self::handle_message);

        // Clone state for the handler
        let db = self.db.clone();
        let bus = self.bus.clone();
        let gateway = self.gateway.clone();

        let mut dispatcher = Dispatcher::builder(self.bot, handler)
            .dependencies(dptree::deps![db, bus, gateway])
            .enable_ctrlc_handler()
            .build();

        let token = dispatcher.shutdown_token();

        // Spawn dispatcher loop so we can return token
        tokio::spawn(async move {
            dispatcher.dispatch().await;
            info!("Telegram connector dispatcher finished");
        });

        token
    }

    /// Handle an incoming Telegram message.
    async fn handle_message(
        bot: Bot,
        msg: Message,
        db: Arc<Database>,
        bus: Arc<EventBus>,
        gateway: Arc<Gateway>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let text = match msg.text() {
            Some(t) => t.to_string(),
            None => {
                // Ignore non-text messages for now
                return Ok(());
            }
        };

        let chat_id = msg.chat.id;
        let user_id = msg
            .from
            .as_ref()
            .map(|u| u.id.0.to_string())
            .unwrap_or_default();
        let display_name = msg.from.as_ref().and_then(|u| u.first_name.clone().into());

        info!(
            "Received message from Telegram user {} in chat {}: {}",
            user_id,
            chat_id,
            text.chars().take(50).collect::<String>()
        );

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

        if let Err(e) = openalpaca_core::security::policy::TrustGate::check(
            &principal,
            &capability,
            &scope,
        ) {
            warn!("TrustGate denied request: {}", e);
            bot.send_message(chat_id, format_denial_message(&e)).await?;
            return Ok(());
        }

        // Step 4: Route through Gateway (replaces manual pipeline)
        let response = gateway.handle_event(GatewayRequest {
            source: EventSource::Telegram {
                chat_id: chat_id.0.to_string(),
                user_id: user_id.clone(),
            },
            content: text.clone(),
            principal,
            scope: Scope::Conversation {
                id: chat_id.0.to_string(),
            },
        }).await;

        // Step 5: Send response back to Telegram
        bot.send_message(chat_id, &response.content).await?;

        // Note: EventBus events (UserRequest + AgentResponse) are now emitted
        // by Gateway and the MessageHandler, not by the connector.
        let _ = bus; // Keep bus in deps for /link events if needed

        Ok(())
    }

    /// Handle the /link command.
    async fn handle_link_command(
        bot: &Bot,
        chat_id: ChatId,
        user_id: &str,
        token: &str,
        external_identity_id: i64,
        identity_repo: &IdentityRepository<'_>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!(
            "Processing /link command for user {} with token {}",
            user_id, token
        );

        match handle_link_token(identity_repo, token, external_identity_id) {
            Ok(LinkResult::Success(global_user_id)) => {
                info!(
                    "Successfully linked telegram:{} -> global_user:{}",
                    user_id, global_user_id
                );
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
                warn!("Invalid/expired link token: {}", token);
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
    async fn handle_unlink_command(
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

#[async_trait]
impl Connector for TelegramConnector {
    fn name(&self) -> &'static str {
        "telegram"
    }

    async fn run(&self) -> Result<(), ConnectorError> {
        // Note: run_blocking consumes self, so this is a simplified version
        // for the trait. In practice, you'd use run_blocking directly.
        Err(ConnectorError::Internal(
            "Use run_blocking() directly for Telegram connector".to_string(),
        ))
    }

    async fn shutdown(&self) -> Result<(), ConnectorError> {
        // Teloxide handles shutdown via Ctrl+C handler
        info!("Telegram connector shutdown requested");
        Ok(())
    }
}
