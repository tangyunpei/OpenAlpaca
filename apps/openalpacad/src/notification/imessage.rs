//! iMessage notification delivery (macOS only).

use super::NotificationDispatcher;
use openalpaca_storage::{ConfigRepository, PreferenceRepository};
use tracing::warn;

impl NotificationDispatcher {
    /// Send a notification to the iMessage chat that originated the task.
    /// Uses the user's stored `imessage.last_reply_target` and `imessage.last_is_group`
    /// preferences (set by the connector on each incoming message) to resolve the
    /// correct target and addressing mode (DM vs group).
    #[cfg(target_os = "macos")]
    pub(super) async fn try_imessage_notification(&self, source_lane: &str, message: &str) {
        // source_lane format: "{user_id}:imessage" — extract user_id
        let user_id = source_lane.strip_suffix(":imessage").unwrap_or(source_lane);
        let pref_repo = PreferenceRepository::new(&self.db);

        let (target, is_group) = if let Some(t) = pref_repo
            .get(user_id, "imessage.last_reply_target")
            .ok()
            .flatten()
            .map(|p| p.value)
        {
            let ig = pref_repo
                .get(user_id, "imessage.last_is_group")
                .ok()
                .flatten()
                .map(|p| p.value == "true")
                .unwrap_or_else(|| t.starts_with("chat"));
            (Some(t), ig)
        } else if let Some(chat_id) = pref_repo
            .get(user_id, "imessage.last_chat_id")
            .ok()
            .flatten()
            .map(|p| p.value)
        {
            // Legacy fallback: infer is_group from chat_id format
            let ig = chat_id.starts_with("chat");
            (Some(chat_id), ig)
        } else {
            (None, false)
        };

        if let Some(target) = target {
            let from_account = pref_repo
                .get(user_id, "imessage.last_send_account")
                .ok()
                .flatten()
                .map(|p| p.value);
            if let Err(e) =
                openalpaca_connectors::imessage::IMessageSender::send_from(
                    &target, message, is_group, from_account.as_deref(),
                )
                .await
            {
                warn!("Failed to send iMessage notification: {e}");
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) async fn try_imessage_notification(&self, _source_lane: &str, _message: &str) {}

    /// Cross-channel iMessage delivery for tasks not originating from iMessage.
    #[cfg(target_os = "macos")]
    pub(super) async fn try_cross_channel_imessage(&self, created_by: &str, message: &str) {
        let pref_repo = PreferenceRepository::new(&self.db);

        let should_notify = pref_repo
            .get(created_by, "imessage.notify_task_completion")
            .ok()
            .flatten()
            .map(|p| p.value == "true")
            .unwrap_or_else(|| {
                // Fallback: check global default in system_config
                ConfigRepository::new(&self.db)
                    .get("imessage.notify_task_completion")
                    .ok()
                    .flatten()
                    .map(|v| v == "true")
                    .unwrap_or(false)
            });
        if !should_notify {
            return;
        }

        let (target, is_group) = if let Some(t) = pref_repo
            .get(created_by, "imessage.last_reply_target")
            .ok()
            .flatten()
            .map(|p| p.value)
        {
            let ig = pref_repo
                .get(created_by, "imessage.last_is_group")
                .ok()
                .flatten()
                .map(|p| p.value == "true")
                .unwrap_or_else(|| t.starts_with("chat"));
            (Some(t), ig)
        } else if let Some(chat_id) = pref_repo
            .get(created_by, "imessage.last_chat_id")
            .ok()
            .flatten()
            .map(|p| p.value)
        {
            // Legacy fallback: infer is_group from chat_id format
            let ig = chat_id.starts_with("chat");
            (Some(chat_id), ig)
        } else {
            (None, false)
        };

        if let Some(target) = target {
            let from_account = pref_repo
                .get(created_by, "imessage.last_send_account")
                .ok()
                .flatten()
                .map(|p| p.value);
            if let Err(e) =
                openalpaca_connectors::imessage::IMessageSender::send_from(
                    &target, message, is_group, from_account.as_deref(),
                )
                .await
            {
                warn!("Failed to send cross-channel iMessage notification: {e}");
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) async fn try_cross_channel_imessage(&self, _created_by: &str, _message: &str) {}
}
