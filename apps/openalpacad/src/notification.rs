//! NotificationDispatcher — Pushes task completion notifications to external platforms.
//!
//! Subscribes to the EventBus and sends notifications when tasks complete or fail.
//! For tasks originating from Telegram, sends a message back to the originating chat.

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

        // source_lane format: "{user_id}:telegram"
        if task.source_lane.ends_with(":telegram") {
            if let Some(chat_id) = self.resolve_telegram_chat_id(&task.source_lane)
                && let Some(ref bot) = self.telegram_bot
            {
                let content = format!(
                    "Task completed: {}\n\n{}",
                    task.title,
                    summary.unwrap_or("Done")
                );
                if let Err(e) = bot.send_message(ChatId(chat_id), content).await {
                    warn!("Failed to send task completion notification: {e}");
                }
            }
        } else {
            // Cross-channel delivery for non-Telegram-origin tasks
            let content = format!(
                "Task completed: {}\n\n{}",
                task.title,
                summary.unwrap_or("Done")
            );
            self.try_cross_channel_telegram(&task.created_by, &content)
                .await;
        }
    }

    async fn handle_failure(&self, task_id: &str, error: &str) {
        let repo = TaskRepository::new(&self.db);
        let task = match repo.get(task_id) {
            Ok(Some(t)) => t,
            _ => return,
        };

        if task.source_lane.ends_with(":telegram") {
            if let Some(chat_id) = self.resolve_telegram_chat_id(&task.source_lane)
                && let Some(ref bot) = self.telegram_bot
            {
                let content = format!("Task failed: {}\n\nError: {}", task.title, error);
                if let Err(e) = bot.send_message(ChatId(chat_id), content).await {
                    warn!("Failed to send task failure notification: {e}");
                }
            }
        } else {
            // Cross-channel delivery for non-Telegram-origin tasks
            let content = format!("Task failed: {}\n\nError: {}", task.title, error);
            self.try_cross_channel_telegram(&task.created_by, &content)
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

    fn resolve_telegram_chat_id(&self, lane_key: &str) -> Option<i64> {
        let identity_repo = IdentityRepository::new(&self.db);
        identity_repo
            .get_chat_id_by_lane_key(lane_key, "telegram")
            .ok()
            .flatten()
    }
}
