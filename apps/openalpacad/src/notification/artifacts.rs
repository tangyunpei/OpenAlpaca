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
