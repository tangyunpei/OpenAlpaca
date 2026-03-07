//! Telegram cross-channel notification delivery.

use super::NotificationDispatcher;
use openalpaca_storage::{ConfigRepository, PreferenceRepository};
use teloxide::prelude::*;
use tracing::warn;

impl NotificationDispatcher {
    /// Try cross-channel Telegram delivery for non-Telegram tasks.
    /// Returns the chat_id that was sent to (for artifact delivery), or None.
    pub(super) async fn try_cross_channel_telegram(&self, created_by: &str, message: &str) -> Option<i64> {
        let bot = match self.telegram_bot() {
            Some(b) => b,
            None => return None,
        };
        let pref_repo = PreferenceRepository::new(&self.db);

        let should_notify = pref_repo
            .get(created_by, "telegram.notify_task_completion")
            .ok()
            .flatten()
            .map(|p| p.value == "true")
            .unwrap_or_else(|| {
                // Fallback: check global default in system_config
                ConfigRepository::new(&self.db)
                    .get("telegram.notify_task_completion")
                    .ok()
                    .flatten()
                    .map(|v| v == "true")
                    .unwrap_or(false)
            });
        if !should_notify {
            return None;
        }

        let chat_id = match pref_repo
            .get(created_by, "telegram.last_chat_id")
            .ok()
            .flatten()
            .and_then(|p| p.value.parse::<i64>().ok())
        {
            Some(id) => id,
            None => return None,
        };

        if let Err(e) = bot.send_message(ChatId(chat_id), message).await {
            warn!("Failed to send cross-channel notification: {e}");
        }
        Some(chat_id)
    }
}
