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
                SystemEvent::WorkflowProgress {
                    lane_key, message, ..
                } => {
                    self.handle_progress(&lane_key, &message).await;
                }
                SystemEvent::ExtensionCapabilityWithdrawn {
                    ref extension,
                    ref state,
                    cause,
                    ref affected_cron_skills,
                    ref notice_lane,
                    ..
                } => {
                    self.handle_extension_notice(
                        extension,
                        state,
                        cause,
                        affected_cron_skills,
                        notice_lane,
                    )
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

    /// **T1 step 3's owner notice** (extension design §7.3 step 2).
    ///
    /// A cron skill runs unattended: the event log alone is not enough for the
    /// one failure mode with no human in the loop. So when a transition leaves
    /// a cron-scheduled skill wholly unsatisfiable, the owner gets **one**
    /// notice per transition — never per fire.
    ///
    /// Not `SystemEvent::WorkflowProgress`: `handle_progress` dispatches only to
    /// lane keys ending `:telegram`, `:imessage` or `:discord`, so the default
    /// `:gui` lane falls through all three branches and the notice would be a
    /// silent no-op for the default user. This does what `post_update` does —
    /// **write** the conversation row, **then** fan out cross-channel.
    async fn handle_extension_notice(
        &self,
        extension: &openalpaca_core::tools::extensions::ExtensionId,
        state: &openalpaca_core::tools::extensions::ExtensionState,
        cause: openalpaca_core::tools::extensions::WithdrawalCause,
        affected_cron_skills: &[String],
        notice_lane: &str,
    ) {
        if affected_cron_skills.is_empty() || notice_lane.is_empty() {
            return;
        }

        let one = affected_cron_skills.len() == 1;
        let mut content = format!(
            "Scheduled {} {} can no longer run — {} '{}': {}. {} run again once it is \
             available; nothing else was changed.",
            if one { "skill" } else { "skills" },
            affected_cron_skills
                .iter()
                .map(|s| format!("'{s}'"))
                .collect::<Vec<_>>()
                .join(", "),
            extension.kind.prose(),
            extension.name,
            // Detail-free: this row is chat-visible and the model reads it back,
            // so the free-text `detail` travels below, wrapped (§7.1).
            cause.wording_without_detail(extension),
            if one { "It will" } else { "They will" },
        );
        if let openalpaca_core::tools::extensions::ExtensionState::Failed { detail, .. } = state
            && !detail.is_empty()
        {
            content.push_str("\n\n");
            content.push_str(&openalpaca_core::tools::extensions::describe::wrap_detail(
                detail,
            ));
        }

        // Write — the half that reaches the default lane and `GET
        // /v1/chat/history`. `"gui"` is the lane's own source, passed as the
        // `source` **column**: if the default lane has no conversation row yet,
        // `get_or_create_conversation` creates one with whatever source it is
        // handed, and a `"system"`-sourced default lane would be wrong forever.
        openalpaca_core::orchestrator::dispatcher::persist_conversation(
            &self.db,
            notice_lane,
            "gui",
            content.clone(),
            None,
            0,
            0,
            0,
        );

        // Push — the same cross-channel fan-out `handle_failure` uses for
        // non-connector-origin tasks. The dispatcher has no `local_user_id`
        // field, so the user id is derived from the lane the same way
        // `resolve_telegram_chat_id` derives it from a `:telegram` lane.
        let Some(user_id) = notice_lane.strip_suffix(":gui") else {
            return;
        };
        let _ = self.try_cross_channel_telegram(user_id, &content).await;
        self.try_cross_channel_imessage(user_id, &content).await;
        self.try_cross_channel_discord(user_id, &content).await;
    }

    /// Push a mid-workflow progress update to the lane's originating channel.
    /// Same per-lane suffix routing as completions, but terse: the message is
    /// sent as-is, with no cross-channel fan-out or artifact delivery.
    async fn handle_progress(&self, lane_key: &str, content: &str) {
        if lane_key.ends_with(":telegram") {
            if let Some(chat_id) = self.resolve_telegram_chat_id(lane_key)
                && let Some(ref bot) = self.telegram_bot()
                && let Err(e) = bot.send_message(ChatId(chat_id), content).await
            {
                warn!("Failed to send workflow progress notification: {e}");
            }
        } else if lane_key.ends_with(":imessage") {
            self.try_imessage_notification(lane_key, content).await;
        } else if lane_key.ends_with(":discord") {
            self.try_discord_notification(lane_key, content).await;
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
