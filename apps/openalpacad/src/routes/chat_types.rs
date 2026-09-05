//! Request/response types and helpers for chat endpoints.

use axum::{http::StatusCode, response::IntoResponse};
use openalpaca_core::security::confirmation::ApprovalScope;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct ChatSendRequest {
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<openalpaca_storage::AttachmentRef>,
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
}

#[derive(Deserialize)]
pub struct ConversationsQuery {
    pub source: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize)]
pub struct ConversationsResponse {
    pub conversations: Vec<openalpaca_storage::Conversation>,
}

#[derive(Serialize)]
pub struct ConversationMessagesResponse {
    pub messages: Vec<openalpaca_storage::ConversationMessage>,
    pub total: i64,
}

#[derive(Serialize)]
pub struct ChatHistoryResponse {
    pub messages: Vec<openalpaca_storage::ConversationMessage>,
    pub total: i64,
    pub lane_key: String,
}

#[derive(Deserialize)]
pub struct DeleteHistoryQuery {
    pub lane_key: Option<String>,
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
}
