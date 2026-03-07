//! NotificationDispatcher — Pushes task completion notifications to external platforms.
//!
//! Subscribes to the EventBus and sends notifications when tasks complete or fail.
//! For tasks originating from Telegram or iMessage, sends a message back to the originating chat.

pub(crate) mod artifacts;
mod discord;
mod formatting;
mod imessage;
mod telegram;

#[cfg(test)]
mod tests;

use artifacts::deliver_artifacts;
use formatting::{format_completion_message, format_failure_message};
use openalpaca_core::events::SystemEvent;
use openalpaca_core::orchestrator::ConnectorSendProvider;
use openalpaca_storage::{
    Database, IdentityRepository, PreferenceRepository, TaskRepository,
};
use std::sync::Arc;
use teloxide::prelude::*;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Dispatches task completion/failure notifications to external platforms.
///
/// Reads the Telegram bot token lazily from `ConfigRepository` on each
/// notification so that token changes take effect without daemon restart.
pub struct NotificationDispatcher {
    bus_rx: broadcast::Receiver<SystemEvent>,
    db: Database,
    cancel_token: CancellationToken,
    connector_send: Option<Arc<dyn ConnectorSendProvider>>,
}

impl NotificationDispatcher {
    pub fn new(
        bus_rx: broadcast::Receiver<SystemEvent>,
        db: Database,
        cancel_token: CancellationToken,
        connector_send: Option<Arc<dyn ConnectorSendProvider>>,
    ) -> Self {
        Self {
            bus_rx,
            db,
            cancel_token,
            connector_send,
        }
    }

    /// Resolve a fresh Telegram Bot from the current config.
    fn telegram_bot(&self) -> Option<Bot> {
        let config_repo = openalpaca_storage::ConfigRepository::new(&self.db);
        config_repo
            .get("telegram.token")
            .ok()
            .flatten()
            .filter(|t| !t.is_empty())
            .map(Bot::new)
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
                    outcome_kind,
                    artifact_count,
                    outcome_summary,
                    ..
                } => {
                    self.handle_completion(
                        &task_id,
                        result_summary.as_deref(),
                        outcome_kind.as_deref(),
                        artifact_count,
                        outcome_summary.as_deref(),
                    )
                    .await;
                }
                SystemEvent::TaskFailed {
                    task_id,
                    error,
                    outcome_kind,
                    ..
                } => {
                    self.handle_failure(&task_id, &error, outcome_kind.as_deref())
                        .await;
                }
                _ => {}
            }
        }
    }

    async fn handle_completion(
        &self,
        task_id: &str,
        summary: Option<&str>,
        outcome_kind: Option<&str>,
        artifact_count: Option<i32>,
        outcome_summary: Option<&str>,
    ) {
        let repo = TaskRepository::new(&self.db);
        let task = match repo.get(task_id) {
            Ok(Some(t)) => t,
            _ => return,
        };

        let content = format_completion_message(
            &task.title,
            summary,
            outcome_kind,
            artifact_count,
            outcome_summary,
        );

        // source_lane format: "{user_id}:telegram" or "{user_id}:imessage"
        if task.source_lane.ends_with(":telegram") {
            let chat_id = self.resolve_telegram_chat_id(&task.source_lane);
            if let Some(chat_id) = chat_id
                && let Some(ref bot) = self.telegram_bot()
                && let Err(e) = bot.send_message(ChatId(chat_id), &content).await
            {
                warn!("Failed to send task completion notification: {e}");
            }
            // Spawn non-blocking artifact file delivery
            if let Some(chat_id) = chat_id {
                self.spawn_artifact_delivery(task_id, chat_id, task.outcome_json.as_deref(), &task.created_by);
            }
        } else if task.source_lane.ends_with(":imessage") {
            self.try_imessage_notification(&task.source_lane, &content)
                .await;
        } else if task.source_lane.ends_with(":discord") {
            self.try_discord_notification(&task.source_lane, &content)
                .await;
        } else {
            // Cross-channel delivery for non-connector-origin tasks
            let cross_chat_id = self.try_cross_channel_telegram(&task.created_by, &content)
                .await;
            self.try_cross_channel_imessage(&task.created_by, &content)
                .await;
            self.try_cross_channel_discord(&task.created_by, &content)
                .await;
            // Spawn non-blocking artifact file delivery for cross-channel Telegram
            if let Some(chat_id) = cross_chat_id {
                self.spawn_artifact_delivery(task_id, chat_id, task.outcome_json.as_deref(), &task.created_by);
            }
        }
    }

    async fn handle_failure(&self, task_id: &str, error: &str, outcome_kind: Option<&str>) {
        let repo = TaskRepository::new(&self.db);
        let task = match repo.get(task_id) {
            Ok(Some(t)) => t,
            _ => return,
        };

        // Parse artifact_count from outcome_json (partially-failed pipelines may have artifacts)
        let artifact_count: Option<i32> = task
            .outcome_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
            .and_then(|v| v["artifacts"].as_array().map(|a| a.len() as i32));

        let content = format_failure_message(&task.title, error, outcome_kind, artifact_count);

        if task.source_lane.ends_with(":telegram") {
            if let Some(chat_id) = self.resolve_telegram_chat_id(&task.source_lane)
                && let Some(ref bot) = self.telegram_bot()
                && let Err(e) = bot.send_message(ChatId(chat_id), &content).await
            {
                warn!("Failed to send task failure notification: {e}");
            }
        } else if task.source_lane.ends_with(":imessage") {
            self.try_imessage_notification(&task.source_lane, &content)
                .await;
        } else if task.source_lane.ends_with(":discord") {
            self.try_discord_notification(&task.source_lane, &content)
                .await;
        } else {
            // Cross-channel delivery for non-connector-origin tasks
            // No artifact delivery for failure notifications
            let _ = self.try_cross_channel_telegram(&task.created_by, &content)
                .await;
            self.try_cross_channel_imessage(&task.created_by, &content)
                .await;
            self.try_cross_channel_discord(&task.created_by, &content)
                .await;
        }
    }

    /// Spawn a non-blocking artifact delivery task to a Telegram chat.
    fn spawn_artifact_delivery(&self, task_id: &str, chat_id: i64, outcome_json: Option<&str>, owner: &str) {
        if let Some(ref send) = self.connector_send {
            let db = self.db.clone();
            let send = send.clone();
            let task_id = task_id.to_string();
            let chat_id_str = chat_id.to_string();
            let outcome_json = outcome_json.map(|s| s.to_string());
            let owner = owner.to_string();
            tokio::spawn(async move {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(60),
                    deliver_artifacts(&db, &*send, &task_id, "telegram", &chat_id_str, outcome_json.as_deref(), &owner),
                ).await;
            });
        }
    }

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
