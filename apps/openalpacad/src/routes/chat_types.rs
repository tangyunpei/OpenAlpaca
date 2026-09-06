//! Request/response types and helpers for chat endpoints.

use axum::{http::StatusCode, response::IntoResponse};
use openalpaca_core::security::confirmation::ApprovalScope;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct ChatSendRequest {
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<openalpaca_storage::AttachmentRef>,
    /// The conversation this turn belongs to (§5.7). Absent means the lane's
    /// active session, created on demand — today's behaviour exactly.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Re-open `session_id` if it is archived. Absent, an archived target is
    /// refused with `409 SESSION_ARCHIVED` rather than silently re-homing the
    /// lane.
    #[serde(default)]
    pub activate: bool,
}

#[derive(Serialize)]
pub struct ChatSendResponseBody {
    pub stream_id: String,
    pub lane_key: String,
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub lane_key: Option<String>,
    /// Which conversation on that lane; absent means its active one.
    pub session_id: Option<String>,
}

/// A stored message as the two history routes answer it (GAP-23).
///
/// Additive by construction: the row is flattened, so every field a client
/// already reads — `task_id` among them, straight off the 038 column — keeps
/// its place, and `artifacts` is the one thing added. It is the message's
/// `role='artifact'` links, resolved to what a transcript chip needs, and it is
/// always present (`[]` for the overwhelming majority of rows) so the client
/// never has to distinguish "no artifacts" from "not served".
#[derive(Serialize)]
pub struct ConversationMessageView {
    #[serde(flatten)]
    pub message: openalpaca_storage::ConversationMessage,
    pub artifacts: Vec<openalpaca_storage::MessageArtifact>,
}

/// Resolve a whole page of messages' artifact links in **one** query, so the
/// client gets the run link and its chips in one round trip and the daemon
/// spends one statement, not one per message.
pub(super) fn with_artifacts(
    db: &openalpaca_storage::Database,
    messages: Vec<openalpaca_storage::ConversationMessage>,
) -> Vec<ConversationMessageView> {
    let ids: Vec<i64> = messages.iter().map(|m| m.id).collect();
    let mut links = openalpaca_storage::FileAssetRepository::new(db)
        .artifact_links_for_messages(&ids)
        .unwrap_or_else(|e| {
            // The transcript is the payload; its chips are decoration. A read
            // failure answers messages without chips, never a 500.
            tracing::warn!("Failed to read artifact links for a history page: {e}");
            Default::default()
        });
    messages
        .into_iter()
        .map(|message| ConversationMessageView {
            artifacts: links.remove(&message.id).unwrap_or_default(),
            message,
        })
        .collect()
}

#[derive(Serialize)]
pub struct ChatHistoryResponse {
    pub messages: Vec<ConversationMessageView>,
    pub total: i64,
    pub lane_key: String,
    /// The conversation these messages came from; `null` on a lane that has
    /// never held a turn.
    pub session_id: Option<String>,
}

#[derive(Deserialize)]
pub struct DeleteHistoryQuery {
    pub lane_key: Option<String>,
    /// Which conversation on that lane; absent means its active one.
    pub session_id: Option<String>,
}

#[derive(Serialize)]
pub struct ChatDeleteResponse {
    pub deleted: u64,
}

#[derive(Deserialize)]
pub struct FeedbackRequest {
    pub feedback: String, // "positive" | "negative"
    pub comment: Option<String>,
}

#[derive(Serialize)]
pub struct FeedbackResponse {
    pub message_id: i64,
    pub feedback: String,
    pub comment: Option<String>,
}

#[derive(Serialize)]
pub struct FeedbackDeleteResponse {
    pub deleted: bool,
}

#[derive(Deserialize)]
pub struct ConfirmationBody {
    pub approved: bool,
    /// GAP-01: granularity of the approval, forwarded to
    /// `ConfirmationResponse` — omitted or `null` is safe: the sandbox
    /// defaults to `ApprovalScope::TheseArgs` when this is `None`.
    #[serde(default)]
    pub approval_scope: Option<ApprovalScope>,
}

/// Check if the given lane_key belongs to the specified user.
/// Lane key format is "{user_id}:{source_name}".
pub(super) fn is_lane_owned_by(lane_key: &str, user_id: &str) -> bool {
    lane_key.starts_with(&format!("{}:", user_id))
}

/// Delegates to the shared `{"error":{"code","message"}}` envelope in
/// `routes::api_error` — collapses what used to be a byte-identical copy of
/// the struct + builder duplicated in `files_types.rs`.
pub(super) fn error_response(status: StatusCode, code: &str, message: &str) -> impl IntoResponse {
    super::api_error(status, code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GAP-01: the three shapes the GUI actually sends — approve/deny with no
    /// scope (approved/denied buttons, Enter/Escape) and "Always allow"'s
    /// `approval_scope: "entire_tool"`.
    #[test]
    fn test_confirmation_body_approved_without_scope() {
        let body: ConfirmationBody = serde_json::from_str(r#"{"approved":true}"#).unwrap();
        assert!(body.approved);
        assert!(body.approval_scope.is_none());
    }

    #[test]
    fn test_confirmation_body_denied_without_scope() {
        let body: ConfirmationBody = serde_json::from_str(r#"{"approved":false}"#).unwrap();
        assert!(!body.approved);
        assert!(body.approval_scope.is_none());
    }

    #[test]
    fn test_confirmation_body_approved_with_entire_tool_scope() {
        let body: ConfirmationBody =
            serde_json::from_str(r#"{"approved":true,"approval_scope":"entire_tool"}"#).unwrap();
        assert!(body.approved);
        assert_eq!(body.approval_scope, Some(ApprovalScope::EntireTool));
    }

    #[test]
    fn test_confirmation_body_approved_with_these_args_scope() {
        let body: ConfirmationBody =
            serde_json::from_str(r#"{"approved":true,"approval_scope":"these_args"}"#).unwrap();
        assert_eq!(body.approval_scope, Some(ApprovalScope::TheseArgs));
    }

    // ── GAP-23: what a reloaded transcript reads ────────────────────

    /// A lane with three messages: a plain turn, a delegating turn carrying
    /// only its run id, and the completion report carrying the run *and* the
    /// two files it produced.
    fn history_fixture() -> (tempfile::TempDir, openalpaca_storage::Database) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).expect("db");
        db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO task (id, title, created_by, source_lane)
                 VALUES ('task-1', 'A run', 'user1', 'user1:gui')",
                [],
            )?;
            for (id, name) in [("produced-1", "notes.md"), ("produced-2", "diff.patch")] {
                conn.execute(
                    "INSERT INTO file_assets (id, owner_id, sha256, filename, mime_type, size_bytes, storage_path, status, origin, task_id, kind)
                     VALUES (?1, 'user1', ?1, ?2, 'text/markdown', 10, ?3, 'ready', 'produced', 'task-1', 'markdown')",
                    [id.to_string(), name.to_string(), format!("/tmp/{name}")],
                )?;
            }
            Ok(())
        })
        .expect("seed");

        let repo = openalpaca_storage::ConversationRepository::new(&db);
        repo.insert(&openalpaca_storage::ConversationMessage {
            lane_key: "user1:gui".to_string(),
            role: "user".to_string(),
            content: "do the thing".to_string(),
            ..Default::default()
        })
        .expect("user turn");
        repo.insert(&openalpaca_storage::ConversationMessage {
            lane_key: "user1:gui".to_string(),
            role: "assistant".to_string(),
            content: "Starting that now.".to_string(),
            task_id: Some("task-1".to_string()),
            ..Default::default()
        })
        .expect("delegating turn");
        let report = repo
            .insert(&openalpaca_storage::ConversationMessage {
                lane_key: "user1:gui".to_string(),
                role: "assistant".to_string(),
                content: "Done.".to_string(),
                task_id: Some("task-1".to_string()),
                ..Default::default()
            })
            .expect("report");

        let files = openalpaca_storage::FileAssetRepository::new(&db);
        for (i, id) in ["produced-1", "produced-2"].iter().enumerate() {
            files
                .link_to_message_with_role(
                    report,
                    id,
                    i as i32,
                    None,
                    openalpaca_storage::ARTIFACT_ROLE,
                )
                .expect("link");
        }
        (dir, db)
    }

    #[test]
    fn test_history_view_serialises_the_run_link_and_the_artifact_chips() {
        let (_dir, db) = history_fixture();
        let messages = openalpaca_storage::ConversationRepository::new(&db)
            .list_by_lane("user1:gui", 50, 0)
            .expect("history");

        let views = with_artifacts(&db, messages);
        let json: serde_json::Value =
            serde_json::to_value(&views).expect("the view serialises");

        // The plain turn: no run, no chips — and every field it always had.
        assert_eq!(json[0]["role"], "user");
        assert_eq!(json[0]["content"], "do the thing");
        assert!(json[0]["task_id"].is_null());
        assert_eq!(json[0]["artifacts"], serde_json::json!([]));
        // The row's own columns survive the flatten.
        for field in ["id", "lane_key", "created_at", "content_json"] {
            assert!(
                json[0].get(field).is_some(),
                "{field} should still be on the wire"
            );
        }

        // The delegating turn: the run link, and nothing it could not know.
        assert_eq!(json[1]["task_id"], "task-1");
        assert_eq!(json[1]["artifacts"], serde_json::json!([]));

        // The completion report: the run link and one chip per produced file.
        assert_eq!(json[2]["task_id"], "task-1");
        assert_eq!(
            json[2]["artifacts"],
            serde_json::json!([
                {"id": "produced-1", "name": "notes.md", "kind": "markdown"},
                {"id": "produced-2", "name": "diff.patch", "kind": "markdown"},
            ])
        );
    }

    #[test]
    fn test_history_view_of_an_empty_page_is_empty() {
        let (_dir, db) = history_fixture();
        assert!(with_artifacts(&db, Vec::new()).is_empty());
    }
}
