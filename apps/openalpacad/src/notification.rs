//! NotificationDispatcher — Pushes task completion notifications to external platforms.
//!
//! Subscribes to the EventBus and sends notifications when tasks complete or fail.
//! For tasks originating from Telegram or iMessage, sends a message back to the originating chat.

use openalpaca_core::events::SystemEvent;
use openalpaca_storage::{Database, IdentityRepository, PreferenceRepository, TaskRepository};
use teloxide::prelude::*;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Dispatches task completion/failure notifications to external platforms.
pub struct NotificationDispatcher {
    bus_rx: broadcast::Receiver<SystemEvent>,
    telegram_bot: Option<Bot>,
    db: Database,
    cancel_token: CancellationToken,
}

impl NotificationDispatcher {
    pub fn new(
        bus_rx: broadcast::Receiver<SystemEvent>,
        telegram_bot: Option<Bot>,
        db: Database,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            bus_rx,
            telegram_bot,
            db,
            cancel_token,
        }
    }

    /// Run the notification loop. Blocks until cancelled or the bus sender is dropped.
    pub async fn run(mut self) {
        info!("NotificationDispatcher started");
        loop {
            let event = tokio::select! {
                result = self.bus_rx.recv() => match result {
                    Ok(ev) => ev,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("NotificationDispatcher lagged by {n} events");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("NotificationDispatcher: bus closed, shutting down");
                        break;
                    }
                },
                _ = self.cancel_token.cancelled() => {
                    info!("NotificationDispatcher: cancelled, shutting down");
                    break;
                }
            };
            match event {
                SystemEvent::TaskCompleted {
                    task_id,
                    result_summary,
                    ..
                } => {
                    self.handle_completion(&task_id, result_summary.as_deref())
                        .await;
                }
                SystemEvent::TaskFailed { task_id, error, .. } => {
                    self.handle_failure(&task_id, &error).await;
                }
                _ => {}
            }
        }
    }

    async fn handle_completion(&self, task_id: &str, summary: Option<&str>) {
        let repo = TaskRepository::new(&self.db);
        let task = match repo.get(task_id) {
            Ok(Some(t)) => t,
            _ => return,
        };

        let content = format!(
            "Task completed: {}\n\n{}",
            task.title,
            summary.unwrap_or("Done")
        );

        // source_lane format: "{user_id}:telegram" or "{user_id}:imessage"
        if task.source_lane.ends_with(":telegram") {
            if let Some(chat_id) = self.resolve_telegram_chat_id(&task.source_lane)
                && let Some(ref bot) = self.telegram_bot
                && let Err(e) = bot.send_message(ChatId(chat_id), &content).await
            {
                warn!("Failed to send task completion notification: {e}");
            }
        } else if task.source_lane.ends_with(":imessage") {
            self.try_imessage_notification(&task.source_lane, &content)
                .await;
        } else {
            // Cross-channel delivery for non-connector-origin tasks
            self.try_cross_channel_telegram(&task.created_by, &content)
                .await;
            self.try_cross_channel_imessage(&task.created_by, &content)
                .await;
        }
    }

    async fn handle_failure(&self, task_id: &str, error: &str) {
        let repo = TaskRepository::new(&self.db);
        let task = match repo.get(task_id) {
            Ok(Some(t)) => t,
            _ => return,
        };

        let content = format!("Task failed: {}\n\nError: {}", task.title, error);

        if task.source_lane.ends_with(":telegram") {
            if let Some(chat_id) = self.resolve_telegram_chat_id(&task.source_lane)
                && let Some(ref bot) = self.telegram_bot
                && let Err(e) = bot.send_message(ChatId(chat_id), &content).await
            {
                warn!("Failed to send task failure notification: {e}");
            }
        } else if task.source_lane.ends_with(":imessage") {
            self.try_imessage_notification(&task.source_lane, &content)
                .await;
        } else {
            // Cross-channel delivery for non-connector-origin tasks
            self.try_cross_channel_telegram(&task.created_by, &content)
                .await;
            self.try_cross_channel_imessage(&task.created_by, &content)
                .await;
        }
    }

    /// Try cross-channel Telegram delivery for non-Telegram tasks.
    async fn try_cross_channel_telegram(&self, created_by: &str, message: &str) {
        let bot = match &self.telegram_bot {
            Some(b) => b,
            None => return,
        };
        let pref_repo = PreferenceRepository::new(&self.db);

        let should_notify = pref_repo
            .get(created_by, "telegram.notify_task_completion")
            .ok()
            .flatten()
            .map(|p| p.value == "true")
            .unwrap_or(false);
        if !should_notify {
            return;
        }

        let chat_id = match pref_repo
            .get(created_by, "telegram.last_chat_id")
            .ok()
            .flatten()
            .and_then(|p| p.value.parse::<i64>().ok())
        {
            Some(id) => id,
            None => return,
        };

        if let Err(e) = bot.send_message(ChatId(chat_id), message).await {
            warn!("Failed to send cross-channel notification: {e}");
        }
    }

    /// Send a notification to the iMessage chat that originated the task.
    /// Uses the user's stored `imessage.last_reply_target` and `imessage.last_is_group`
    /// preferences (set by the connector on each incoming message) to resolve the
    /// correct target and addressing mode (DM vs group).
    #[cfg(target_os = "macos")]
    async fn try_imessage_notification(&self, source_lane: &str, message: &str) {
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

        if let Some(target) = target
            && let Err(e) =
                openalpaca_connectors::imessage::IMessageSender::send(&target, message, is_group)
                    .await
        {
            warn!("Failed to send iMessage notification: {e}");
        }
    }

    #[cfg(not(target_os = "macos"))]
    async fn try_imessage_notification(&self, _source_lane: &str, _message: &str) {}

    /// Cross-channel iMessage delivery for tasks not originating from iMessage.
    #[cfg(target_os = "macos")]
    async fn try_cross_channel_imessage(&self, created_by: &str, message: &str) {
        let pref_repo = PreferenceRepository::new(&self.db);

        let should_notify = pref_repo
            .get(created_by, "imessage.notify_task_completion")
            .ok()
            .flatten()
            .map(|p| p.value == "true")
            .unwrap_or(false);
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

        if let Some(target) = target
            && let Err(e) =
                openalpaca_connectors::imessage::IMessageSender::send(&target, message, is_group)
                    .await
        {
            warn!("Failed to send cross-channel iMessage notification: {e}");
        }
    }

    #[cfg(not(target_os = "macos"))]
    async fn try_cross_channel_imessage(&self, _created_by: &str, _message: &str) {}

    /// Resolve the Telegram chat_id for a given lane_key.
    ///
    /// Strategy: prefer the user's `telegram.last_chat_id` preference
    /// (updated on every incoming message), then fall back to `conversation_map`.
    /// This avoids routing ambiguity for users who interact from multiple
    /// Telegram chats, since the preference always reflects the most recent chat.
    fn resolve_telegram_chat_id(&self, lane_key: &str) -> Option<i64> {
        // Prefer the user's last active chat (stored per-message)
        let user_id = lane_key.strip_suffix(":telegram")?;
        let pref_repo = PreferenceRepository::new(&self.db);
        if let Some(chat_id) = pref_repo
            .get(user_id, "telegram.last_chat_id")
            .ok()
            .flatten()
            .and_then(|p| p.value.parse::<i64>().ok())
        {
            return Some(chat_id);
        }

        // Fallback: conversation_map (arbitrary if multiple chats)
        let identity_repo = IdentityRepository::new(&self.db);
        identity_repo
            .get_chat_id_by_lane_key(lane_key, "telegram")
            .ok()
            .flatten()
    }
}
