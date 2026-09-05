//! Discord notification delivery.

use super::NotificationDispatcher;
use openalpaca_storage::{ConfigRepository, PreferenceRepository};
use tracing::warn;

impl NotificationDispatcher {
    /// Send a notification to the Discord channel that originated the task.
    /// Uses the user's stored `discord.last_channel_id` preference (set by
    /// the connector on each incoming message) to resolve the target channel.
    pub(super) async fn try_discord_notification(&self, source_lane: &str, message: &str) {
        if let Some(ref send) = self.connector_send {
            let user_id = source_lane.strip_suffix(":discord").unwrap_or(source_lane);
            let pref_repo = PreferenceRepository::new(&self.db);
            if let Some(channel_id) = pref_repo
                .get(user_id, "discord.last_channel_id")
                .ok()
                .flatten()
                .map(|p| p.value)
                && let Err(e) = send.send_message("discord", &channel_id, message).await
            {
                warn!("Failed to send Discord notification: {e}");
            }
        }
    }

    /// Cross-channel Discord delivery for tasks not originating from Discord.
    pub(super) async fn try_cross_channel_discord(&self, created_by: &str, message: &str) {
        let send = match self.connector_send {
            Some(ref s) => s,
            None => return,
        };
        let pref_repo = PreferenceRepository::new(&self.db);

        let should_notify = pref_repo
            .get(created_by, "discord.notify_task_completion")
            .ok()
            .flatten()
            .map(|p| p.value == "true")
            .unwrap_or_else(|| {
                // Fallback: check global default in system_config
                ConfigRepository::new(&self.db)
                    .get("discord.notify_task_completion")
                    .ok()
                    .flatten()
                    .map(|v| v == "true")
                    .unwrap_or(false)
            });
        if !should_notify {
            return;
        }

        let channel_id = match pref_repo
            .get(created_by, "discord.last_channel_id")
            .ok()
            .flatten()
            .map(|p| p.value)
        {
            Some(id) => id,
            None => return,
        };

        if let Err(e) = send.send_message("discord", &channel_id, message).await {
            warn!("Failed to send cross-channel Discord notification: {e}");
        }
    }
}
