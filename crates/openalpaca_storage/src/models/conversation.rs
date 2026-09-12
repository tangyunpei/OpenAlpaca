//! Conversation message model for chat persistence

use serde::{Deserialize, Serialize};

/// A single message in a conversation, persisted to SQLite.
///
/// `Default` is derived deliberately: every writer names the three or four
/// columns it actually sets and takes the rest from `..Default::default()`, so
/// a new nullable column is one field on this struct and one line in the
/// queries that read it — not an edit at every literal in the workspace.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub id: i64,
    pub lane_key: String,
    pub role: String,
    pub content: String,
    pub source: Option<String>,
    pub model: Option<String>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub duration_ms: Option<i64>,
    pub created_at: String,
    pub content_json: Option<String>,
    pub display_text: Option<String>,
    /// The run this message belongs to (GAP-23, migration 038): the turn that
    /// *started* a workflow, and the completion report that closed it. `None`
    /// for ordinary chat. Not a foreign key — a pruned run leaves a dangling
    /// id rather than taking the transcript with it.
    pub task_id: Option<String>,
    /// The session — the conversation epoch — this message belongs to
    /// (migration 039). Resolved from the lane's active session when the
    /// writer does not name one. `None` only for rows written before a lane
    /// had any session at all.
    pub session_id: Option<String>,
}

/// A session: one conversation transcript — an epoch of a lane, bound to at
/// most one workspace, with an `active` → `archived` lifecycle.
///
/// Migration 039 rebuilt `conversations` as `session`; the Rust name stayed
/// (renaming it is churn with no behaviour change), so this struct is what
/// `/v1/sessions` serves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub lane_key: String,
    pub source: String,
    pub title: String,
    pub message_count: i64,
    pub last_message_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub summary: String,
    pub summary_version: i64,
    pub last_summarized_message_id: i64,
    pub summary_updated_at: Option<String>,
    /// Canonical project root this conversation is bound to; `None` = none.
    /// Set from the first `x-workspace-path` the session sees, `PATCH`-able.
    /// One workspace per session — changing project means a new session.
    pub workspace_id: Option<String>,
    /// `"active"` | `"archived"`. At most one active session per lane, and the
    /// database enforces it (`idx_session_active_lane`).
    pub status: String,
    /// When the session was archived; `None` while it is active.
    pub ended_at: Option<String>,
}
