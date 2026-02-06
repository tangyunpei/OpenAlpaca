//! NotificationDispatcher — Pushes task completion notifications to external platforms.
//!
//! Subscribes to the EventBus and sends notifications when tasks complete or fail.
//! For tasks originating from Telegram, sends a message back to the originating chat.

use openalpaca_core::events::SystemEvent;
use openalpaca_storage::{Database, IdentityRepository, TaskRepository};
use teloxide::prelude::*;
use tokio::sync::broadcast;
use tracing::{info, warn};

/// Dispatches task completion/failure notifications to external platforms.
pub struct NotificationDispatcher {
    bus_rx: broadcast::Receiver<SystemEvent>,
    telegram_bot: Option<Bot>,
    db: Database,
}

impl NotificationDispatcher {
    pub fn new(
        bus_rx: broadcast::Receiver<SystemEvent>,
        telegram_bot: Option<Bot>,
        db: Database,
    ) -> Self {
        Self {
            bus_rx,
            telegram_bot,
            db,
        }
    }

    /// Run the notification loop. Blocks until the bus sender is dropped.
    pub async fn run(mut self) {
        info!("NotificationDispatcher started");
        loop {
            match self.bus_rx.recv().await {
                Ok(event) => match event {
                    SystemEvent::TaskCompleted {
                        task_id,
                        result_summary,
                        ..
                    } => {
                        self.handle_completion(&task_id, result_summary.as_deref())
                            .await;
                    }
                    SystemEvent::TaskFailed {
                        task_id, error, ..
                    } => {
                        self.handle_failure(&task_id, &error).await;
                    }
                    _ => {}
                },
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("NotificationDispatcher lagged by {n} events");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    info!("NotificationDispatcher: bus closed, shutting down");
                    break;
                }
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
            if let Some(chat_id) = self.resolve_telegram_chat_id(&task.source_lane) {
                if let Some(ref bot) = self.telegram_bot {
                    let content = format!(
                        "Task completed: {}\n\n{}",
                        task.title,
                        summary.unwrap_or("Done")
                    );
                    if let Err(e) = bot.send_message(ChatId(chat_id), content).await {
                        warn!("Failed to send task completion notification: {e}");
                    }
                }
            }
        }
    }

    async fn handle_failure(&self, task_id: &str, error: &str) {
        let repo = TaskRepository::new(&self.db);
        let task = match repo.get(task_id) {
            Ok(Some(t)) => t,
            _ => return,
        };

        if task.source_lane.ends_with(":telegram") {
            if let Some(chat_id) = self.resolve_telegram_chat_id(&task.source_lane) {
                if let Some(ref bot) = self.telegram_bot {
                    let content = format!("Task failed: {}\n\nError: {}", task.title, error);
                    if let Err(e) = bot.send_message(ChatId(chat_id), content).await {
                        warn!("Failed to send task failure notification: {e}");
                    }
                }
            }
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
