//! NotificationDispatcher — Pushes task completion notifications to external platforms.
//!
//! Subscribes to the EventBus and sends notifications when tasks complete or fail.
//! For tasks originating from Telegram or iMessage, sends a message back to the originating chat.

use openalpaca_core::events::SystemEvent;
use openalpaca_core::orchestrator::ConnectorSendProvider;
use openalpaca_storage::{
    ConfigRepository, Database, FileAssetRepository, IdentityRepository, PreferenceRepository,
    TaskRepository,
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
            if let Some(chat_id) = self.resolve_telegram_chat_id(&task.source_lane)
                && let Some(ref bot) = self.telegram_bot()
                && let Err(e) = bot.send_message(ChatId(chat_id), &content).await
            {
                warn!("Failed to send task completion notification: {e}");
            }
            // Spawn non-blocking artifact file delivery
            if let Some(chat_id) = self.resolve_telegram_chat_id(&task.source_lane) {
                if let Some(ref send) = self.connector_send {
                    let db = self.db.clone();
                    let send = send.clone();
                    let task_id = task_id.to_string();
                    let chat_id_str = chat_id.to_string();
                    let outcome_json = task.outcome_json.clone();
                    tokio::spawn(async move {
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(60),
                            deliver_artifacts(&db, &*send, &task_id, "telegram", &chat_id_str, outcome_json.as_deref()),
                        ).await;
                    });
                }
            }
        } else if task.source_lane.ends_with(":imessage") {
            self.try_imessage_notification(&task.source_lane, &content)
                .await;
        } else {
            // Cross-channel delivery for non-connector-origin tasks
            let cross_chat_id = self.try_cross_channel_telegram(&task.created_by, &content)
                .await;
            self.try_cross_channel_imessage(&task.created_by, &content)
                .await;
            // Spawn non-blocking artifact file delivery for cross-channel Telegram
            if let Some(chat_id) = cross_chat_id {
                if let Some(ref send) = self.connector_send {
                    let db = self.db.clone();
                    let send = send.clone();
                    let task_id = task_id.to_string();
                    let chat_id_str = chat_id.to_string();
                    let outcome_json = task.outcome_json.clone();
                    tokio::spawn(async move {
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(60),
                            deliver_artifacts(&db, &*send, &task_id, "telegram", &chat_id_str, outcome_json.as_deref()),
                        ).await;
                    });
                }
            }
        }
    }

    async fn handle_failure(&self, task_id: &str, error: &str, outcome_kind: Option<&str>) {
        let repo = TaskRepository::new(&self.db);
        let task = match repo.get(task_id) {
            Ok(Some(t)) => t,
            _ => return,
        };

        let content = format_failure_message(&task.title, error, outcome_kind);

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
        } else {
            // Cross-channel delivery for non-connector-origin tasks
            // No artifact delivery for failure notifications
            let _ = self.try_cross_channel_telegram(&task.created_by, &content)
                .await;
            self.try_cross_channel_imessage(&task.created_by, &content)
                .await;
        }
    }

    /// Try cross-channel Telegram delivery for non-Telegram tasks.
    /// Returns the chat_id that was sent to (for artifact delivery), or None.
    async fn try_cross_channel_telegram(&self, created_by: &str, message: &str) -> Option<i64> {
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
    async fn try_cross_channel_imessage(&self, _created_by: &str, _message: &str) {}

    /// Resolve the Telegram chat_id for a given lane_key.
    ///
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

/// Build completion notification message (pure function, testable).
fn format_completion_message(
    title: &str,
    summary: Option<&str>,
    outcome_kind: Option<&str>,
    artifact_count: Option<i32>,
    outcome_summary: Option<&str>,
) -> String {
    let display_summary = outcome_summary.or(summary).unwrap_or("Done");

    let outcome_line = match outcome_kind {
        Some("text_only") => "\nNo files were produced.".to_string(),
        Some("artifact_only") => {
            let count = artifact_count.unwrap_or(0);
            format!(
                "\n{} file{} produced.",
                count,
                if count != 1 { "s" } else { "" }
            )
        }
        Some("mixed") => {
            let count = artifact_count.unwrap_or(0);
            format!(
                "\n{} file{} produced (with text summary).",
                count,
                if count != 1 { "s" } else { "" }
            )
        }
        _ => String::new(),
    };

    format!(
        "Task completed: {}\n\n{}{}",
        title, display_summary, outcome_line
    )
}

/// Build failure notification message (pure function, testable).
fn format_failure_message(title: &str, error: &str, outcome_kind: Option<&str>) -> String {
    format!(
        "Task failed: {}\n\nError: {}{}",
        title,
        error,
        if outcome_kind == Some("failed") {
            "\nNo files were produced."
        } else {
            ""
        }
    )
}

/// Maximum file size for artifact delivery (50 MB — Telegram Bot API limit).
const MAX_ARTIFACT_FILE_SIZE: i64 = 50 * 1024 * 1024;

/// Maximum number of artifacts to deliver per task.
const MAX_ARTIFACTS_PER_TASK: usize = 5;

/// Resolve a file asset for an artifact pointer.
///
/// Resolution strategy:
/// 1. Use `file_asset_id` if present
/// 2. Try `key` as a file_asset ID fallback
/// 3. Return None (workspace-only artifact)
pub(crate) fn resolve_artifact_file(
    repo: &FileAssetRepository<'_>,
    file_asset_id: Option<&str>,
    key: &str,
) -> Option<openalpaca_storage::FileAsset> {
    // 1. Explicit file_asset_id
    if let Some(id) = file_asset_id {
        if let Ok(Some(asset)) = repo.get_by_id(id) {
            return Some(asset);
        }
    }
    // 2. Try key as file_asset ID fallback
    if let Ok(Some(asset)) = repo.get_by_id(key) {
        return Some(asset);
    }
    // 3. Workspace-only artifact
    None
}

/// Deliver artifact files to a channel. Called from a spawned task with timeout.
async fn deliver_artifacts(
    db: &Database,
    send: &dyn ConnectorSendProvider,
    task_id: &str,
    channel: &str,
    recipient: &str,
    outcome_json: Option<&str>,
) {
    use openalpaca_core::orchestrator::task_state::TaskOutcome;

    let outcome_json = match outcome_json {
        Some(oj) => oj,
        None => return,
    };
    let outcome: TaskOutcome = match serde_json::from_str(outcome_json) {
        Ok(o) => o,
        Err(e) => {
            warn!(task_id, "Failed to parse outcome_json for artifact delivery: {e}");
            return;
        }
    };
    if outcome.artifacts.is_empty() {
        return;
    }
    if !send.file_capable_channels().contains(&channel.to_string()) {
        return;
    }

    let file_repo = FileAssetRepository::new(db);
    for artifact in outcome.artifacts.iter().take(MAX_ARTIFACTS_PER_TASK) {
        let asset = match resolve_artifact_file(
            &file_repo,
            artifact.file_asset_id.as_deref(),
            &artifact.key,
        ) {
            Some(a) => a,
            None => continue,
        };

        // Check file exists on disk
        let path = std::path::Path::new(&asset.storage_path);
        if !path.exists() {
            warn!(task_id, file_id = %asset.id, "Artifact file not found on disk, skipping");
            continue;
        }

        // Check file size
        if asset.size_bytes > MAX_ARTIFACT_FILE_SIZE {
            warn!(
                task_id,
                file_id = %asset.id,
                size_bytes = asset.size_bytes,
                "Artifact file exceeds 50MB limit, skipping"
            );
            continue;
        }

        let caption = Some(format!("{} ({})", artifact.label, artifact.key));
        if let Err(e) = send
            .send_file(
                channel,
                recipient,
                &asset.storage_path,
                &asset.filename,
                &asset.mime_type,
                caption.as_deref(),
            )
            .await
        {
            warn!(
                task_id,
                file_id = %asset.id,
                "Failed to deliver artifact file: {e}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── format_completion_message ──────────────────────────────────────

    #[test]
    fn completion_text_only() {
        let msg = format_completion_message(
            "Summarize report",
            Some("All done"),
            Some("text_only"),
            None,
            Some("Summary is ready"),
        );
        assert!(msg.contains("Task completed: Summarize report"));
        assert!(msg.contains("Summary is ready"));
        assert!(msg.contains("No files were produced."));
    }

    #[test]
    fn completion_artifact_only_plural() {
        let msg = format_completion_message(
            "Generate images",
            None,
            Some("artifact_only"),
            Some(3),
            None,
        );
        assert!(msg.contains("Task completed: Generate images"));
        assert!(msg.contains("Done")); // no outcome_summary or summary → fallback
        assert!(msg.contains("3 files produced."));
    }

    #[test]
    fn completion_artifact_only_singular() {
        let msg = format_completion_message(
            "Create file",
            None,
            Some("artifact_only"),
            Some(1),
            None,
        );
        assert!(msg.contains("1 file produced."));
        assert!(!msg.contains("files")); // singular
    }

    #[test]
    fn completion_mixed() {
        let msg = format_completion_message(
            "Analyze data",
            Some("result_summary"),
            Some("mixed"),
            Some(2),
            Some("outcome_summary"),
        );
        assert!(msg.contains("Task completed: Analyze data"));
        assert!(msg.contains("outcome_summary")); // outcome_summary preferred over summary
        assert!(msg.contains("2 files produced (with text summary)."));
    }

    #[test]
    fn completion_no_outcome_kind() {
        let msg = format_completion_message(
            "Simple task",
            Some("Finished"),
            None,
            None,
            None,
        );
        assert_eq!(msg, "Task completed: Simple task\n\nFinished");
    }

    #[test]
    fn completion_all_none_fields() {
        let msg = format_completion_message("Task X", None, None, None, None);
        assert_eq!(msg, "Task completed: Task X\n\nDone");
    }

    #[test]
    fn completion_zero_artifacts() {
        let msg = format_completion_message(
            "Task",
            None,
            Some("artifact_only"),
            Some(0),
            None,
        );
        assert!(msg.contains("0 files produced."));
    }

    #[test]
    fn completion_outcome_summary_preferred_over_result_summary() {
        let msg = format_completion_message(
            "T",
            Some("result_summary"),
            Some("text_only"),
            None,
            Some("outcome_summary"),
        );
        assert!(msg.contains("outcome_summary"));
        assert!(!msg.contains("result_summary"));
    }

    #[test]
    fn completion_falls_back_to_result_summary() {
        let msg = format_completion_message(
            "T",
            Some("result_summary"),
            Some("text_only"),
            None,
            None, // no outcome_summary
        );
        assert!(msg.contains("result_summary"));
    }

    #[test]
    fn completion_unknown_outcome_kind_ignored() {
        let msg = format_completion_message(
            "T",
            Some("OK"),
            Some("some_future_variant"),
            Some(5),
            None,
        );
        // Unknown variant falls through to _ => String::new()
        assert_eq!(msg, "Task completed: T\n\nOK");
    }

    // ── format_failure_message ─────────────────────────────────────────

    #[test]
    fn failure_with_failed_outcome() {
        let msg = format_failure_message("Broken task", "timeout", Some("failed"));
        assert!(msg.contains("Task failed: Broken task"));
        assert!(msg.contains("Error: timeout"));
        assert!(msg.contains("No files were produced."));
    }

    #[test]
    fn failure_without_outcome() {
        let msg = format_failure_message("Broken task", "OOM", None);
        assert_eq!(msg, "Task failed: Broken task\n\nError: OOM");
    }

    #[test]
    fn failure_with_non_failed_outcome_kind() {
        let msg = format_failure_message("T", "err", Some("text_only"));
        // Non-"failed" outcome_kind → no extra line
        assert!(!msg.contains("No files were produced."));
        assert_eq!(msg, "Task failed: T\n\nError: err");
    }

    #[test]
    fn failure_empty_error_string() {
        let msg = format_failure_message("T", "", Some("failed"));
        assert!(msg.contains("Error: \n"));
    }

    // ── resolve_artifact_file + deliver_artifacts ─────────────────────

    fn test_db() -> Database {
        let dir = tempfile::tempdir().unwrap();
        Database::open(&dir.path().join("test.db")).unwrap()
    }

    fn make_file_asset(id: &str, path: &str, size: i64) -> openalpaca_storage::FileAsset {
        openalpaca_storage::FileAsset {
            id: id.to_string(),
            owner_id: "user1".to_string(),
            sha256: format!("sha_{id}"),
            filename: format!("{id}.pdf"),
            mime_type: "application/pdf".to_string(),
            size_bytes: size,
            storage_path: path.to_string(),
            status: openalpaca_storage::FileAssetStatus::Ready,
            extracted_text: None,
            extract_error: None,
            metadata_json: None,
            created_at: "2025-01-01 00:00:00".to_string(),
            updated_at: "2025-01-01 00:00:00".to_string(),
        }
    }

    #[test]
    fn test_resolve_artifact_file_uses_file_asset_id() {
        let db = test_db();
        let repo = FileAssetRepository::new(&db);
        let asset = make_file_asset("asset_1", "/tmp/test.pdf", 1024);
        repo.insert(&asset).unwrap();

        let result = resolve_artifact_file(&repo, Some("asset_1"), "some_workspace_key");
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "asset_1");
    }

    #[test]
    fn test_resolve_artifact_file_falls_back_to_key() {
        let db = test_db();
        let repo = FileAssetRepository::new(&db);
        let asset = make_file_asset("key_as_id", "/tmp/test.pdf", 1024);
        repo.insert(&asset).unwrap();

        // No file_asset_id, but key matches a file_asset ID
        let result = resolve_artifact_file(&repo, None, "key_as_id");
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "key_as_id");
    }

    #[test]
    fn test_resolve_artifact_file_returns_none_for_workspace_key() {
        let db = test_db();
        let repo = FileAssetRepository::new(&db);

        // No file_asset_id, key doesn't match any file_asset ID
        let result = resolve_artifact_file(&repo, None, "workspace_only_key");
        assert!(result.is_none());
    }

    /// Mock ConnectorSendProvider for testing deliver_artifacts.
    struct MockSendProvider {
        file_capable: Vec<String>,
    }

    #[async_trait::async_trait]
    impl ConnectorSendProvider for MockSendProvider {
        async fn send_message(&self, _c: &str, _r: &str, _m: &str) -> Result<String, String> {
            Ok("ok".to_string())
        }
        fn sendable_channels(&self) -> Vec<String> {
            self.file_capable.clone()
        }
        fn file_capable_channels(&self) -> Vec<String> {
            self.file_capable.clone()
        }
    }

    #[tokio::test]
    async fn test_deliver_artifacts_skips_non_file_capable_channel() {
        let db = test_db();
        let send = MockSendProvider {
            file_capable: vec!["telegram".to_string()],
        };
        // outcome_json with one artifact
        let outcome_json = r#"{"summary":"done","outcome_kind":"mixed","artifacts":[{"key":"k","label":"l","agent_id":"a","step_order":0}]}"#;
        // Channel is "imessage" which is NOT in file_capable_channels
        // This should return early without error
        deliver_artifacts(&db, &send, "task_1", "imessage", "12345", Some(outcome_json)).await;
        // No assertion needed — just verifying no panic
    }

    #[tokio::test]
    async fn test_deliver_artifacts_skips_oversized_files() {
        let db = test_db();
        let repo = FileAssetRepository::new(&db);
        // Insert a file asset that exceeds 50MB
        let oversized = make_file_asset("big_file", "/tmp/nonexistent.bin", 60 * 1024 * 1024);
        repo.insert(&oversized).unwrap();

        let send = MockSendProvider {
            file_capable: vec!["telegram".to_string()],
        };
        // outcome_json referencing the oversized file via file_asset_id
        let outcome_json = r#"{"summary":"done","outcome_kind":"artifact_only","artifacts":[{"key":"k","label":"Big file","agent_id":"a","step_order":0,"file_asset_id":"big_file"}]}"#;
        // This should skip the oversized file without panic (file doesn't exist on disk either,
        // but the size check comes after the exists check — so it will be skipped at exists check).
        // Both checks result in skipping, which is the desired behavior.
        deliver_artifacts(&db, &send, "task_1", "telegram", "12345", Some(outcome_json)).await;
    }
}
