//! Artifact file delivery for task notifications.

use openalpaca_core::orchestrator::ConnectorSendProvider;
use openalpaca_storage::{Database, FileAssetRepository};
use tracing::warn;

/// Maximum file size for artifact delivery (50 MB — Telegram Bot API limit).
const MAX_ARTIFACT_FILE_SIZE: i64 = 50 * 1024 * 1024;

/// Maximum number of artifacts to deliver per task.
const MAX_ARTIFACTS_PER_TASK: usize = 5;

/// Resolve a file asset for an artifact pointer with owner validation.
///
/// Resolution strategy:
/// 1. Use `file_asset_id` if present
/// 2. Try `key` as a file_asset ID fallback
/// 3. Return None (workspace-only artifact)
///
/// After resolving, validates that the asset belongs to `expected_owner`.
/// Returns None (with a warning) if the owner doesn't match — prevents
/// delivering files belonging to another user.
pub(crate) fn resolve_artifact_file(
    repo: &FileAssetRepository<'_>,
    file_asset_id: Option<&str>,
    key: &str,
    expected_owner: &str,
) -> Option<openalpaca_storage::FileAsset> {
    // 1. Explicit file_asset_id
    let asset = if let Some(id) = file_asset_id {
        repo.get_by_id(id).ok().flatten()
    } else {
        None
    };
    // 2. Try key as file_asset ID fallback
    let asset = asset.or_else(|| repo.get_by_id(key).ok().flatten());

    // 3. Validate owner
    match asset {
        Some(a) if a.owner_id == expected_owner => Some(a),
        Some(a) => {
            warn!(
                file_id = %a.id,
                actual_owner = %a.owner_id,
                expected_owner,
                "Artifact file owner mismatch — skipping delivery"
            );
            None
        }
        None => None,
    }
}

/// Deliver artifact files to a channel. Called from a spawned task with timeout.
pub(super) async fn deliver_artifacts(
    db: &Database,
    send: &dyn ConnectorSendProvider,
    task_id: &str,
    channel: &str,
    recipient: &str,
    outcome_json: Option<&str>,
    expected_owner: &str,
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
            expected_owner,
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
    use openalpaca_core::orchestrator::task_state::{ArtifactPointer, TaskOutcome};
    use openalpaca_storage::{ArtifactKind, ArtifactStore, NewArtifact, OutcomeKind};
    use openalpaca_storage::store::StoreScope;
    use std::sync::Mutex;

    const OWNER: &str = "owner-1";
    const TASK_ID: &str = "t-5555eeee-0000-0000-0000-000000000000";

    /// Records what the connector was asked to send.
    #[derive(Default)]
    struct RecordingSender {
        files: Mutex<Vec<(String, String, String)>>,
    }

    #[async_trait::async_trait]
    impl ConnectorSendProvider for RecordingSender {
        async fn send_message(&self, _: &str, _: &str, _: &str) -> Result<String, String> {
            Ok("sent".to_string())
        }
        fn sendable_channels(&self) -> Vec<String> {
            vec!["telegram".to_string()]
        }
        async fn send_file(
            &self,
            _channel: &str,
            _recipient: &str,
            file_path: &str,
            filename: &str,
            mime_type: &str,
            _caption: Option<&str>,
        ) -> Result<String, String> {
            self.files.lock().unwrap().push((
                file_path.to_string(),
                filename.to_string(),
                mime_type.to_string(),
            ));
            Ok("sent".to_string())
        }
        fn file_capable_channels(&self) -> Vec<String> {
            vec!["telegram".to_string()]
        }
    }

    fn task_row(db: &Database) {
        let now = chrono::Utc::now();
        openalpaca_storage::repository::TaskRepository::new(db)
            .create(&openalpaca_storage::Task {
                id: TASK_ID.to_string(),
                title: "Delivery run".to_string(),
                description: None,
                status: openalpaca_storage::TaskStatus::Completed,
                priority: 0,
                progress_current: None,
                progress_total: None,
                result_summary: None,
                created_by: OWNER.to_string(),
                source_lane: "telegram".to_string(),
                created_at: now,
                updated_at: now,
                completed_at: None,
                state_json: None,
                state_version: 0,
                outcome_json: None,
                outcome_kind: None,
                artifact_count: 0,
            })
            .unwrap();
    }

    /// A produced artifact, exactly as `artifact_write` / the `workspace_write`
    /// spill writes one.
    fn produce(db: &Database, project: &std::path::Path, title: &str, body: &str) -> String {
        let scope = StoreScope::Project(project.to_path_buf());
        let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, title, body.as_bytes());
        new.task_id = Some(TASK_ID);
        new.task_title = Some("Delivery run");
        ArtifactStore::new(db).put(new).unwrap().0.id
    }

    fn outcome_json(pointer: ArtifactPointer) -> String {
        serde_json::to_string(&TaskOutcome {
            summary: "done".to_string(),
            outcome_kind: OutcomeKind::ArtifactOnly,
            artifacts: vec![pointer],
            no_artifact_reason: None,
        })
        .unwrap()
    }

    /// The payoff of plan §4.6: with a real `file_asset_id` on the pointer,
    /// `deliver_artifacts` resolves the asset and sends the file instead of
    /// silently `continue`-ing past it.
    #[tokio::test]
    async fn delivers_a_produced_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let project = project.path().canonicalize().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        task_row(&db);
        let id = produce(&db, &project, "final-report", "# Report\n");

        let send = RecordingSender::default();
        deliver_artifacts(
            &db,
            &send,
            TASK_ID,
            "telegram",
            "12345",
            Some(&outcome_json(ArtifactPointer {
                key: "final_report".to_string(),
                label: "final_report".to_string(),
                agent_id: "writing_agent".to_string(),
                step_order: -1,
                file_asset_id: Some(id),
            })),
            OWNER,
        )
        .await;

        let files = send.files.lock().unwrap();
        assert_eq!(files.len(), 1, "the artifact should have been delivered");
        let (path, filename, mime) = &files[0];
        assert!(
            std::path::Path::new(path)
                .starts_with(project.join(".openalpaca").join("artifacts")),
            "delivered {path}"
        );
        assert_eq!(filename, "01-final-report.md");
        assert_eq!(mime, "text/markdown");
    }

    /// The pre-§4.6 shape — a pointer with no id — still delivers nothing.
    #[tokio::test]
    async fn an_unbacked_pointer_delivers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        task_row(&db);

        let send = RecordingSender::default();
        deliver_artifacts(
            &db,
            &send,
            TASK_ID,
            "telegram",
            "12345",
            Some(&outcome_json(ArtifactPointer {
                key: "notes".to_string(),
                label: "notes".to_string(),
                agent_id: "writing_agent".to_string(),
                step_order: -1,
                file_asset_id: None,
            })),
            OWNER,
        )
        .await;

        assert!(send.files.lock().unwrap().is_empty());
    }

    /// Owner validation still bites: another user's artifact is never sent.
    #[tokio::test]
    async fn refuses_to_deliver_another_owners_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let project = project.path().canonicalize().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        task_row(&db);
        let id = produce(&db, &project, "secret", "classified\n");

        let send = RecordingSender::default();
        deliver_artifacts(
            &db,
            &send,
            TASK_ID,
            "telegram",
            "12345",
            Some(&outcome_json(ArtifactPointer {
                key: "secret".to_string(),
                label: "secret".to_string(),
                agent_id: "writing_agent".to_string(),
                step_order: -1,
                file_asset_id: Some(id),
            })),
            "someone-else",
        )
        .await;

        assert!(send.files.lock().unwrap().is_empty());
    }
}
