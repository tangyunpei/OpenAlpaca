//! Bridge types that implement the orchestrator's connector traits at daemon level.
//!
//! `CachedConnectorStatusProvider` — lock-free cached connector status for prompt injection.
//! `ConnectorSendBridge` — outbound message sending via Telegram + iMessage.

use arc_swap::ArcSwap;
use async_trait::async_trait;
use openalpaca_core::orchestrator::{ConnectorSendProvider, ConnectorStatusProvider};
use openalpaca_storage::{Database, PreferenceRepository};
use std::sync::Arc;

// ── CachedConnectorStatusProvider ────────────────────────────────────

/// Lock-free cached connector status snapshot.
///
/// Updated by an EventBus subscriber whenever `SystemEvent::ConnectorStatus` fires.
/// Read by the orchestrator's prompt injection paths on every request.
pub struct CachedConnectorStatusProvider {
    statuses: Arc<ArcSwap<Vec<(String, String)>>>,
}

impl CachedConnectorStatusProvider {
    pub fn new() -> Self {
        Self {
            statuses: Arc::new(ArcSwap::from_pointee(Vec::new())),
        }
    }

    /// Replace the cached status snapshot.
    pub fn update(&self, statuses: Vec<(String, String)>) {
        self.statuses.store(Arc::new(statuses));
    }
}

impl ConnectorStatusProvider for CachedConnectorStatusProvider {
    fn list_status(&self) -> Vec<(String, String)> {
        self.statuses.load().as_ref().clone()
    }
}

// ── ConnectorSendBridge ──────────────────────────────────────────────

/// Outbound message sending via Telegram and iMessage.
///
/// Reads the Telegram bot token lazily from `ConfigRepository` on each send
/// so that token changes via the GUI/CLI take effect without daemon restart.
pub struct ConnectorSendBridge {
    db: Database,
    /// The local user's ID, used to look up default recipients from preferences.
    local_user_id: String,
}

impl ConnectorSendBridge {
    pub fn new(db: Database, local_user_id: String) -> Self {
        Self {
            db,
            local_user_id,
        }
    }

    /// Resolve a fresh Telegram Bot from the current config.
    fn telegram_bot(&self) -> Option<teloxide::Bot> {
        let config_repo = openalpaca_storage::ConfigRepository::new(&self.db);
        config_repo
            .get("telegram.token")
            .ok()
            .flatten()
            .filter(|t| !t.is_empty())
            .map(teloxide::Bot::new)
    }
}

#[async_trait]
impl ConnectorSendProvider for ConnectorSendBridge {
    async fn send_message(
        &self,
        channel: &str,
        recipient: &str,
        content: &str,
    ) -> Result<String, String> {
        match channel {
            "telegram" => {
                let bot = self
                    .telegram_bot()
                    .ok_or("Telegram not configured (no token)")?;

                let chat_id = if recipient == "default" {
                    // Resolve from preferences — find the most recent Telegram chat
                    let pref_repo = PreferenceRepository::new(&self.db);
                    pref_repo
                        .get(&self.local_user_id, "telegram.last_chat_id")
                        .ok()
                        .flatten()
                        .and_then(|p| p.value.parse::<i64>().ok())
                        .ok_or("No default Telegram chat found. Please specify a chat_id.")?
                } else if recipient.starts_with('@') {
                    return Err(format!(
                        "Telegram @username ('{}') cannot be used directly. \
                         The Bot API requires a numeric chat_id. \
                         Try recipient=\"default\" instead.",
                        recipient
                    ));
                } else {
                    recipient
                        .parse::<i64>()
                        .map_err(|_| format!("Invalid Telegram chat_id: '{}'", recipient))?
                };

                use teloxide::prelude::*;
                bot.send_message(ChatId(chat_id), content)
                    .await
                    .map_err(|e| format!("Telegram send failed: {}", e))?;
                Ok(format!("Sent to Telegram chat {}", chat_id))
            }

            #[cfg(target_os = "macos")]
            "imessage" => {
                let pref_repo = PreferenceRepository::new(&self.db);
                let (target, is_group) = if recipient == "default" {
                    // Resolve from preferences — try last_reply_target first, then last_chat_id
                    let target = pref_repo
                        .get(&self.local_user_id, "imessage.last_reply_target")
                        .ok()
                        .flatten()
                        .map(|p| p.value)
                        .or_else(|| {
                            // Legacy fallback: last_chat_id (for users who haven't
                            // generated last_reply_target yet)
                            pref_repo
                                .get(&self.local_user_id, "imessage.last_chat_id")
                                .ok()
                                .flatten()
                                .map(|p| p.value)
                        })
                        .ok_or(
                            "No default iMessage target found. Please specify a recipient.",
                        )?;
                    let is_group = pref_repo
                        .get(&self.local_user_id, "imessage.last_is_group")
                        .ok()
                        .flatten()
                        .map(|p| p.value == "true")
                        .unwrap_or_else(|| target.starts_with("chat"));
                    (target, is_group)
                } else {
                    let is_group = recipient.starts_with("chat");
                    (recipient.to_string(), is_group)
                };

                // Resolve the sending account so replies use the correct iMessage address
                let from_account = pref_repo
                    .get(&self.local_user_id, "imessage.last_send_account")
                    .ok()
                    .flatten()
                    .map(|p| p.value);

                openalpaca_connectors::imessage::IMessageSender::send_from(
                    &target,
                    content,
                    is_group,
                    from_account.as_deref(),
                )
                .await?;
                Ok(format!("Sent to iMessage: {}", target))
            }

            #[cfg(not(target_os = "macos"))]
            "imessage" => Err("iMessage is only available on macOS".to_string()),

            _ => Err(format!("Unknown channel: '{}'. Available: telegram, imessage", channel)),
        }
    }

    fn sendable_channels(&self) -> Vec<String> {
        let mut channels = Vec::new();
        if self.telegram_bot().is_some() {
            channels.push("telegram".to_string());
        }
        #[cfg(target_os = "macos")]
        channels.push("imessage".to_string());
        channels
    }

    async fn send_file(
        &self,
        channel: &str,
        recipient: &str,
        file_path: &str,
        filename: &str,
        _mime_type: &str,
        caption: Option<&str>,
    ) -> Result<String, String> {
        match channel {
            "telegram" => {
                let bot = self
                    .telegram_bot()
                    .ok_or("Telegram not configured (no token)")?;

                let chat_id = if recipient == "default" {
                    let pref_repo = PreferenceRepository::new(&self.db);
                    pref_repo
                        .get(&self.local_user_id, "telegram.last_chat_id")
                        .ok()
                        .flatten()
                        .and_then(|p| p.value.parse::<i64>().ok())
                        .ok_or("No default Telegram chat found. Please specify a chat_id.")?
                } else {
                    recipient
                        .parse::<i64>()
                        .map_err(|_| format!("Invalid Telegram chat_id: '{}'", recipient))?
                };

                use teloxide::prelude::*;
                use teloxide::types::InputFile;
                let input_file = InputFile::file(file_path).file_name(filename.to_string());
                let mut request = bot.send_document(ChatId(chat_id), input_file);
                if let Some(cap) = caption {
                    request = request.caption(cap);
                }
                request
                    .await
                    .map_err(|e| format!("Telegram send_document failed: {}", e))?;
                Ok(format!("Sent file '{}' to Telegram chat {}", filename, chat_id))
            }
            _ => Err(format!("File sending not supported on channel: '{}'", channel)),
        }
    }

    fn file_capable_channels(&self) -> Vec<String> {
        let mut channels = Vec::new();
        if self.telegram_bot().is_some() {
            channels.push("telegram".to_string());
        }
        channels
    }
}
