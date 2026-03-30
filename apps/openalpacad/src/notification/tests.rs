use super::artifacts::{deliver_artifacts, resolve_artifact_file};
use super::formatting::{format_completion_message, format_failure_message};
use openalpaca_core::orchestrator::ConnectorSendProvider;
use openalpaca_storage::{Database, FileAssetRepository};

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
    let msg = format_failure_message("Broken task", "timeout", Some("failed"), None);
    assert!(msg.contains("Task failed: Broken task"));
    assert!(msg.contains("Error: timeout"));
    assert!(msg.contains("No files were produced."));
}

#[test]
fn failure_with_failed_outcome_and_artifacts() {
    let msg = format_failure_message("Partial task", "step 3 failed", Some("failed"), Some(2));
    assert!(msg.contains("Task failed: Partial task"));
    assert!(msg.contains("2 files from earlier steps may still be available."));
    assert!(!msg.contains("No files were produced."));
}

#[test]
fn failure_with_failed_outcome_singular_artifact() {
    let msg = format_failure_message("T", "err", Some("failed"), Some(1));
    assert!(msg.contains("1 file from earlier steps may still be available."));
    assert!(!msg.contains("files")); // singular
}

#[test]
fn failure_without_outcome() {
    let msg = format_failure_message("Broken task", "OOM", None, None);
    assert_eq!(msg, "Task failed: Broken task\n\nError: OOM");
}

#[test]
fn failure_with_non_failed_outcome_kind() {
    let msg = format_failure_message("T", "err", Some("text_only"), None);
    // Non-"failed" outcome_kind → no extra line
    assert!(!msg.contains("No files were produced."));
    assert_eq!(msg, "Task failed: T\n\nError: err");
}

#[test]
fn failure_empty_error_string() {
    let msg = format_failure_message("T", "", Some("failed"), None);
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

    let result = resolve_artifact_file(&repo, Some("asset_1"), "some_workspace_key", "user1");
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
    let result = resolve_artifact_file(&repo, None, "key_as_id", "user1");
    assert!(result.is_some());
    assert_eq!(result.unwrap().id, "key_as_id");
}

#[test]
fn test_resolve_artifact_file_returns_none_for_workspace_key() {
    let db = test_db();
    let repo = FileAssetRepository::new(&db);

    // No file_asset_id, key doesn't match any file_asset ID
    let result = resolve_artifact_file(&repo, None, "workspace_only_key", "user1");
    assert!(result.is_none());
}

#[test]
fn test_resolve_artifact_file_rejects_wrong_owner() {
    let db = test_db();
    let repo = FileAssetRepository::new(&db);
    let asset = make_file_asset("asset_1", "/tmp/test.pdf", 1024);
    repo.insert(&asset).unwrap(); // owner_id = "user1"

    // Wrong owner — should return None
    let result = resolve_artifact_file(&repo, Some("asset_1"), "key", "user2");
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
    deliver_artifacts(&db, &send, "task_1", "imessage", "12345", Some(outcome_json), "user1").await;
    // No assertion needed — just verifying no panic
}

#[tokio::test]
async fn test_deliver_artifacts_skips_oversized_files() {
    let db = test_db();
    let repo = FileAssetRepository::new(&db);
    // Create a real (small) temp file so the existence check passes,
    // but set size_bytes > 50MB in the DB record so the size check fires.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"dummy").unwrap();
    let path_str = tmp.path().to_str().unwrap();
    let oversized = make_file_asset("big_file", path_str, 60 * 1024 * 1024);
    repo.insert(&oversized).unwrap();

    let send = MockSendProvider {
        file_capable: vec!["telegram".to_string()],
    };
    // outcome_json referencing the oversized file via file_asset_id
    let outcome_json = r#"{"summary":"done","outcome_kind":"artifact_only","artifacts":[{"key":"k","label":"Big file","agent_id":"a","step_order":0,"file_asset_id":"big_file"}]}"#;
    // File exists on disk → existence check passes → size check fires → skip logged.
    deliver_artifacts(&db, &send, "task_1", "telegram", "12345", Some(outcome_json), "user1").await;
}

// ── Phase 12-B: Edge-case notification tests ──────────────────────

#[test]
fn test_format_completion_handles_very_long_summary() {
    // Summary > 2000 chars should not panic or truncate at the format level
    let long_summary: String = "x".repeat(3000);
    let msg = format_completion_message(
        "Long task",
        Some(&long_summary),
        Some("text_only"),
        None,
        None,
    );
    assert!(msg.contains("Task completed: Long task"));
    // The long summary should appear in the message (format_completion_message
    // does not truncate — truncation is handled by finalize_task_with_outcome)
    assert!(msg.contains(&long_summary));
    assert!(msg.contains("No files were produced."));

    // Also test with outcome_summary being long
    let long_outcome: String = "y".repeat(5000);
    let msg2 = format_completion_message(
        "Long outcome",
        Some("short"),
        Some("mixed"),
        Some(2),
        Some(&long_outcome),
    );
    assert!(msg2.contains(&long_outcome));
    assert!(msg2.contains("2 files produced (with text summary)."));
}

#[tokio::test]
async fn test_deliver_artifacts_handles_malformed_outcome_json() {
    let db = test_db();
    let send = MockSendProvider {
        file_capable: vec!["telegram".to_string()],
    };

    // Completely invalid JSON — should return early without panic
    deliver_artifacts(&db, &send, "task_1", "telegram", "12345", Some("{invalid json!!!}"), "user1").await;

    // Valid JSON but wrong structure — should return early without panic
    deliver_artifacts(&db, &send, "task_2", "telegram", "12345", Some(r#"{"foo":"bar"}"#), "user1").await;

    // Empty string — should return early without panic
    deliver_artifacts(&db, &send, "task_3", "telegram", "12345", Some(""), "user1").await;

    // None — should return early without panic
    deliver_artifacts(&db, &send, "task_4", "telegram", "12345", None, "user1").await;
}
