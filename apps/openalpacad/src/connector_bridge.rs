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

// ── Telegram recipient validation ────────────────────────────────────

/// Parsed Telegram recipient.
#[derive(Debug)]
enum TelegramRecipient {
    Default,
    ChatId(i64),
}

/// Validate Telegram recipient format. Returns Ok(parsed) or Err(user-facing message).
/// Used by send_message and send_file paths. Testable without async/network.
fn validate_telegram_recipient(recipient: &str) -> Result<TelegramRecipient, String> {
    if recipient == "default" {
        return Ok(TelegramRecipient::Default);
    }
    if recipient.starts_with('@') {
        return Err(format!(
            "Telegram @username ('{}') cannot be used directly. \
             The Bot API requires a numeric chat_id. \
             Try recipient=\"default\" instead.",
            recipient
        ));
    }
    recipient
        .parse::<i64>()
        .map(TelegramRecipient::ChatId)
        .map_err(|_| format!("Invalid Telegram chat_id: '{}'", recipient))
}

// ── Discord recipient validation ─────────────────────────────────────

/// Parsed Discord recipient.
#[derive(Debug)]
enum DiscordRecipient {
    Default,
    ChannelId(u64),
}

/// Validate Discord recipient format. Returns Ok(parsed) or Err(user-facing message).
fn validate_discord_recipient(recipient: &str) -> Result<DiscordRecipient, String> {
    if recipient == "default" {
        return Ok(DiscordRecipient::Default);
    }
    recipient
        .parse::<u64>()
        .map(DiscordRecipient::ChannelId)
        .map_err(|_| {
            format!(
                "Invalid Discord channel_id: '{}'. Must be a numeric snowflake ID.",
                recipient
            )
        })
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

    /// Resolve a fresh Discord HTTP client from the current config.
    fn discord_http(&self) -> Option<twilight_http::Client> {
        let config_repo = openalpaca_storage::ConfigRepository::new(&self.db);
        config_repo
            .get("discord.token")
            .ok()
            .flatten()
            .filter(|t| !t.is_empty())
            .map(|t| twilight_http::Client::new(t))
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

                let chat_id = match validate_telegram_recipient(recipient)? {
                    TelegramRecipient::Default => {
                        let pref_repo = PreferenceRepository::new(&self.db);
                        pref_repo
                            .get(&self.local_user_id, "telegram.last_chat_id")
                            .ok()
                            .flatten()
                            .and_then(|p| p.value.parse::<i64>().ok())
                            .ok_or("No default Telegram chat found. Please specify a chat_id.")?
                    }
                    TelegramRecipient::ChatId(id) => id,
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

            "discord" => {
                let http = self
                    .discord_http()
                    .ok_or("Discord not configured (no token)")?;
                let channel_id = match validate_discord_recipient(recipient)? {
                    DiscordRecipient::Default => {
                        let pref_repo = PreferenceRepository::new(&self.db);
                        pref_repo
                            .get(&self.local_user_id, "discord.last_channel_id")
                            .ok()
                            .flatten()
                            .and_then(|p| p.value.parse::<u64>().ok())
                            .ok_or(
                                "No default Discord channel found. Please specify a channel_id.",
                            )?
                    }
                    DiscordRecipient::ChannelId(id) => id,
                };
                use twilight_model::id::{Id, marker::ChannelMarker};
                openalpaca_connectors::discord::send_with_retry(
                    &http,
                    Id::<ChannelMarker>::new(channel_id),
                    content,
                )
                .await?;
                Ok(format!("Sent to Discord channel {}", channel_id))
            }

            _ => Err(format!(
                "Unknown channel: '{}'. Available: telegram, imessage, discord",
                channel
            )),
        }
    }

    fn sendable_channels(&self) -> Vec<String> {
        let mut channels = Vec::new();
        if self.telegram_bot().is_some() {
            channels.push("telegram".to_string());
        }
        #[cfg(target_os = "macos")]
        channels.push("imessage".to_string());
        if self.discord_http().is_some() {
            channels.push("discord".to_string());
        }
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

                let chat_id = match validate_telegram_recipient(recipient)? {
                    TelegramRecipient::Default => {
                        let pref_repo = PreferenceRepository::new(&self.db);
                        pref_repo
                            .get(&self.local_user_id, "telegram.last_chat_id")
                            .ok()
                            .flatten()
                            .and_then(|p| p.value.parse::<i64>().ok())
                            .ok_or("No default Telegram chat found. Please specify a chat_id.")?
                    }
                    TelegramRecipient::ChatId(id) => id,
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
            #[cfg(target_os = "macos")]
            "imessage" => {
                let pref_repo = PreferenceRepository::new(&self.db);
                let (target, is_group) = if recipient == "default" {
                    let target = pref_repo
                        .get(&self.local_user_id, "imessage.last_reply_target")
                        .ok()
                        .flatten()
                        .map(|p| p.value)
                        .or_else(|| {
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

                let from_account = pref_repo
                    .get(&self.local_user_id, "imessage.last_send_account")
                    .ok()
                    .flatten()
                    .map(|p| p.value);

                openalpaca_connectors::imessage::IMessageSender::send_file_from(
                    &target,
                    file_path,
                    is_group,
                    from_account.as_deref(),
                )
                .await?;

                // Send caption as separate text message if provided
                let caption_note = if let Some(cap) = caption
                    && !cap.is_empty()
                {
                    match openalpaca_connectors::imessage::IMessageSender::send_from(
                        &target,
                        cap,
                        is_group,
                        from_account.as_deref(),
                    )
                    .await
                    {
                        Ok(()) => String::new(),
                        Err(e) => {
                            tracing::warn!(target = %target, "iMessage caption send failed (file sent OK): {e}");
                            format!(" (note: caption failed: {})", e)
                        }
                    }
                } else {
                    String::new()
                };

                Ok(format!(
                    "Sent file '{}' to iMessage: {}{}",
                    filename, target, caption_note
                ))
            }

            #[cfg(not(target_os = "macos"))]
            "imessage" => Err("iMessage is only available on macOS".to_string()),

            "discord" => {
                let http = self
                    .discord_http()
                    .ok_or("Discord not configured (no token)")?;
                let channel_id = match validate_discord_recipient(recipient)? {
                    DiscordRecipient::Default => {
                        let pref_repo = PreferenceRepository::new(&self.db);
                        pref_repo
                            .get(&self.local_user_id, "discord.last_channel_id")
                            .ok()
                            .flatten()
                            .and_then(|p| p.value.parse::<u64>().ok())
                            .ok_or(
                                "No default Discord channel found. Please specify a channel_id.",
                            )?
                    }
                    DiscordRecipient::ChannelId(id) => id,
                };
                use twilight_model::id::{Id, marker::ChannelMarker};
                use twilight_model::http::attachment::Attachment;
                let file_data = tokio::fs::read(file_path)
                    .await
                    .map_err(|e| format!("Failed to read file: {e}"))?;
                let mut attempts = 0;
                loop {
                    let attachment =
                        Attachment::from_bytes(filename.to_string(), file_data.clone(), 1);
                    let attachments = [attachment];
                    let req = http
                        .create_message(Id::<ChannelMarker>::new(channel_id))
                        .attachments(&attachments);
                    let req = if let Some(cap) = caption {
                        req.content(cap)
                    } else {
                        req
                    };
                    match req.await {
                        Ok(_) => break,
                        Err(e) => {
                            attempts += 1;
                            if attempts >= 3 {
                                return Err(format!(
                                    "Discord send_file failed after 3 retries: {e}"
                                ));
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(
                                1 << (attempts - 1),
                            ))
                            .await;
                        }
                    }
                }
                Ok(format!(
                    "Sent file '{}' to Discord channel {}",
                    filename, channel_id
                ))
            }

            _ => Err(format!(
                "File sending not supported on channel: '{}'",
                channel
            )),
        }
    }

    fn file_capable_channels(&self) -> Vec<String> {
        let mut channels = Vec::new();
        if self.telegram_bot().is_some() {
            channels.push("telegram".to_string());
        }
        #[cfg(target_os = "macos")]
        channels.push("imessage".to_string());
        if self.discord_http().is_some() {
            channels.push("discord".to_string());
        }
        channels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_telegram_recipient_default() {
        let result = validate_telegram_recipient("default");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), TelegramRecipient::Default));
    }

    #[test]
    fn validate_telegram_recipient_numeric_chat_id() {
        let result = validate_telegram_recipient("12345");
        assert!(result.is_ok());
        match result.unwrap() {
            TelegramRecipient::ChatId(id) => assert_eq!(id, 12345),
            _ => panic!("expected ChatId"),
        }
    }

    #[test]
    fn validate_telegram_recipient_negative_group_chat_id() {
        let result = validate_telegram_recipient("-100123");
        assert!(result.is_ok());
        match result.unwrap() {
            TelegramRecipient::ChatId(id) => assert_eq!(id, -100123),
            _ => panic!("expected ChatId"),
        }
    }

    #[test]
    fn validate_telegram_recipient_rejects_username() {
        let result = validate_telegram_recipient("@someuser");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("@username"));
        assert!(err.contains("chat_id"));
    }

    #[test]
    fn validate_telegram_recipient_rejects_non_numeric() {
        let result = validate_telegram_recipient("abc");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Invalid Telegram chat_id"));
    }

    // ── Discord recipient validation tests ──

    #[test]
    fn validate_discord_recipient_default() {
        let result = validate_discord_recipient("default");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), DiscordRecipient::Default));
    }

    #[test]
    fn validate_discord_recipient_numeric_channel_id() {
        let result = validate_discord_recipient("123456789012345678");
        assert!(result.is_ok());
        match result.unwrap() {
            DiscordRecipient::ChannelId(id) => assert_eq!(id, 123456789012345678),
            _ => panic!("expected ChannelId"),
        }
    }

    #[test]
    fn validate_discord_recipient_rejects_non_numeric() {
        let result = validate_discord_recipient("general");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Invalid Discord channel_id"));
        assert!(err.contains("snowflake"));
    }

    #[test]
    fn validate_discord_recipient_rejects_negative() {
        // Discord snowflakes are unsigned — negative numbers should fail
        let result = validate_discord_recipient("-12345");
        assert!(result.is_err());
    }
}
