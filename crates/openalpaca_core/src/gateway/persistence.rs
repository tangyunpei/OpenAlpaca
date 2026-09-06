//! Gateway-level message persistence
//!
//! Persists user and assistant messages at the Gateway layer so ALL sources
//! (GUI, Telegram, CLI, etc.) get automatic conversation persistence.

use crate::gateway::router::ResolvedAttachment;
use anyhow::Result;
use openalpaca_storage::{
    ConversationMessage, ConversationRepository, Database, FileAssetRepository,
};

/// Handles persisting messages to the conversation_messages table.
pub struct GatewayPersistence {
    db: Database,
}

/// What a turn's user-message persist resolved.
///
/// `session_id` is the load-bearing field. §5.1 resolves lane → session **once
/// per turn**, and the assistant half of that turn is written seconds to
/// minutes later — long enough for a `POST /v1/sessions` ("New chat") to have
/// re-pointed the lane. So the resolve hands its answer back here and the
/// caller pins it, instead of letting the later insert re-read whatever the
/// lane's active session has become by then.
#[derive(Debug, Clone)]
pub struct PersistedUserMessage {
    /// Row id of the message that was written.
    pub message_id: i64,
    /// The session the turn belongs to, for the rest of the turn.
    pub session_id: String,
    /// `true` when this turn's project differed from the one the lane's active
    /// session was bound to, so a new session was opened for it (§5.1). The
    /// caller announces the switch; nothing else about the turn changes.
    pub project_switched: bool,
}

/// The session a turn resolved to, before anything is written to it.
struct TurnSession {
    id: String,
    project_switched: bool,
}

impl GatewayPersistence {
    const PERSISTED_ATTACHMENT_TEXT_CHARS: usize = 4000;

    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Access the underlying database (for cross-repository operations).
    pub fn db(&self) -> &Database {
        &self.db
    }

    /// The turn's session: the lane's active one, or a **new** one when the
    /// request's project is not the project that session is bound to.
    ///
    /// §5.1 gives a session exactly one project and answers a project change
    /// with a new session, never a re-pointed one.
    /// `get_or_create_active_session` owns the "never re-bind" half — it binds
    /// only an unbound session — and this owns the other half: somebody has to
    /// open the new conversation, and the one resolve step a turn goes through
    /// is the place. Without it, a turn sent with `/repo/two` would land in the
    /// session bound to `/repo/one` while the runs it starts record
    /// `workspace_id = /repo/two`, and `GET /v1/sessions?workspace_id=/repo/two`
    /// would not list the conversation those runs came from.
    ///
    /// `workspace_path` is already the canonical project root (R22). `None` —
    /// a connector lane, a scheduled skill, any turn without a workspace —
    /// never switches: an absent project is not a different one.
    fn resolve_turn_session(
        &self,
        repo: &ConversationRepository<'_>,
        lane_key: &str,
        source: &str,
        workspace_path: Option<&str>,
    ) -> Result<TurnSession> {
        if let Some(path) = workspace_path
            && let Some(active) = repo.get_active_session_for_lane(lane_key)?
            && active
                .workspace_id
                .as_deref()
                .is_some_and(|bound| bound != path)
        {
            let fresh = repo.create_session(lane_key, source, Some(path), None)?;
            tracing::info!(
                lane_key,
                previous_session = %active.id,
                previous_workspace = ?active.workspace_id,
                session_id = %fresh.id,
                workspace = path,
                "project changed; opened a new session for the lane"
            );
            return Ok(TurnSession {
                id: fresh.id,
                project_switched: true,
            });
        }
        let session = repo.get_or_create_active_session(lane_key, source, workspace_path)?;
        Ok(TurnSession {
            id: session.id,
            project_switched: false,
        })
    }

    /// Persist a user message, ensuring the lane has an active session.
    ///
    /// This is the one place per turn that resolves lane → session (§5.1):
    /// the runtime stays lane-keyed, persistence becomes session-keyed, and
    /// the message rows below take their `session_id` from the row this call
    /// guarantees exists. `workspace_path` binds the session's project the
    /// first time one is seen and is ignored thereafter.
    ///
    /// The resolved id comes back to the caller so the assistant half of the
    /// same turn can be pinned to it — see [`PersistedUserMessage`].
    pub fn persist_user_message(
        &self,
        lane_key: &str,
        content: &str,
        source: &str,
        workspace_path: Option<&str>,
    ) -> Result<PersistedUserMessage> {
        let repo = ConversationRepository::new(&self.db);
        let session = self.resolve_turn_session(&repo, lane_key, source, workspace_path)?;
        let message_id = repo.insert(&ConversationMessage {
            lane_key: lane_key.to_string(),
            role: "user".to_string(),
            content: content.to_string(),
            source: Some(source.to_string()),
            session_id: Some(session.id.clone()),
            ..Default::default()
        })?;
        repo.increment_message_count_for_session(&session.id)?;
        Ok(PersistedUserMessage {
            message_id,
            session_id: session.id,
            project_switched: session.project_switched,
        })
    }

    /// Persist a user message with file attachments.
    pub fn persist_user_message_with_attachments(
        &self,
        lane_key: &str,
        content: &str,
        source: &str,
        workspace_path: Option<&str>,
        attachments: &[ResolvedAttachment],
    ) -> Result<PersistedUserMessage> {
        let repo = ConversationRepository::new(&self.db);
        let session = self.resolve_turn_session(&repo, lane_key, source, workspace_path)?;

        // Build content_json
        let mut parts = Vec::with_capacity(attachments.len() + 1);
        if !content.trim().is_empty() {
            parts.push(serde_json::json!({"type": "text", "text": content}));
        }
        for att in attachments {
            let extracted = att
                .extracted_text
                .as_ref()
                .map(|t| {
                    t.chars()
                        .take(Self::PERSISTED_ATTACHMENT_TEXT_CHARS)
                        .collect::<String>()
                })
                .filter(|t| !t.trim().is_empty());

            if !att.mime_type.starts_with("image/")
                && !att.mime_type.starts_with("audio/")
                && extracted.is_some()
            {
                parts.push(serde_json::json!({
                    "type": "document",
                    "file_id": att.file_id,
                    "filename": att.filename,
                    "mime_type": att.mime_type,
                    "extracted_text": extracted,
                }));
            } else {
                parts.push(serde_json::json!({
                    "type": "file_ref",
                    "file_id": att.file_id,
                    "filename": att.filename,
                    "mime_type": att.mime_type,
                }));
            }
        }
        let content_json = serde_json::json!({"v": 1, "parts": parts}).to_string();

        // Build display_text
        let filenames: Vec<&str> = attachments.iter().map(|a| a.filename.as_str()).collect();
        let display_text = if filenames.is_empty() {
            content.to_string()
        } else if content.trim().is_empty() {
            format!("[Attachments: {}]", filenames.join(", "))
        } else {
            format!("{}\n[Attachments: {}]", content, filenames.join(", "))
        };

        let msg = ConversationMessage {
            lane_key: lane_key.to_string(),
            role: "user".to_string(),
            content: content.to_string(),
            source: Some(source.to_string()),
            session_id: Some(session.id.clone()),
            ..Default::default()
        };

        let id = repo.insert_with_structured(&msg, &content_json, &display_text)?;

        // Link attachments
        let file_repo = FileAssetRepository::new(&self.db);
        for (i, att) in attachments.iter().enumerate() {
            if let Err(e) = file_repo.link_to_message(id, &att.file_id, i as i32, None) {
                tracing::warn!(
                    "Failed to link attachment {} to message {}: {e}",
                    att.file_id,
                    id
                );
            }
        }

        repo.increment_message_count_for_session(&session.id)?;
        Ok(PersistedUserMessage {
            message_id: id,
            session_id: session.id,
            project_switched: session.project_switched,
        })
    }

    /// Persist an assistant message. Skips empty content to avoid polluting history.
    ///
    /// `task_id` is GAP-23's first link: the turn that *started* a workflow is
    /// stored carrying that run's id, so a reload can tell which assistant
    /// message the delegation came from. It is `None` for ordinary chat — the
    /// caller reads it off `HandleResult::delegation`, never off whatever the
    /// lane happens to be running.
    ///
    /// `session_id` is the turn's session, as the user-message persist
    /// resolved it. It is passed explicitly — the same way
    /// `persist_completion_report` pins — because the handler in between takes
    /// as long as an agentic loop takes, and re-resolving the lane here would
    /// file the answer in whatever conversation a mid-turn "New chat" left
    /// active. `None` (a turn whose user half failed to persist) falls back to
    /// the lane's active session.
    pub fn persist_assistant_message(
        &self,
        lane_key: &str,
        content: &str,
        duration_ms: Option<i64>,
        source: &str,
        task_id: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<i64> {
        if content.trim().is_empty() {
            tracing::debug!("Skipping empty assistant message for lane {}", lane_key);
            return Ok(0);
        }
        let repo = ConversationRepository::new(&self.db);
        let id = repo.insert(&ConversationMessage {
            lane_key: lane_key.to_string(),
            role: "assistant".to_string(),
            content: content.to_string(),
            source: Some(source.to_string()),
            duration_ms,
            task_id: task_id.map(str::to_string),
            session_id: session_id.map(str::to_string),
            ..Default::default()
        })?;
        match session_id {
            Some(id) => repo.increment_message_count_for_session(id)?,
            None => repo.increment_message_count(lane_key)?,
        }
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_storage::Database;

    fn make_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("create tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open test db");
        (dir, db)
    }

    fn sample_attachment() -> ResolvedAttachment {
        ResolvedAttachment {
            file_id: "file-1".to_string(),
            filename: "sample.pdf".to_string(),
            mime_type: "application/pdf".to_string(),
            size_bytes: 123,
            extracted_text: None,
            storage_path: "/tmp/sample.pdf".to_string(),
        }
    }

    fn sample_attachment_with_text(text: &str) -> ResolvedAttachment {
        ResolvedAttachment {
            file_id: "file-doc".to_string(),
            filename: "resume.docx".to_string(),
            mime_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                .to_string(),
            size_bytes: text.len() as i64,
            extracted_text: Some(text.to_string()),
            storage_path: "/tmp/resume.docx".to_string(),
        }
    }

    #[test]
    fn persist_with_attachments_skips_empty_text_part() {
        let (_tmp, db) = make_db();
        let persistence = GatewayPersistence::new(db.clone());

        let id = persistence
            .persist_user_message_with_attachments("user1:gui", "", "gui", None, &[sample_attachment()])
            .expect("persist message");
        assert!(id.message_id > 0);

        let repo = ConversationRepository::new(&db);
        let msgs = repo
            .list_recent_by_lane("user1:gui", 10)
            .expect("load recent messages");
        assert_eq!(msgs.len(), 1);
        let msg = &msgs[0];
        let content_json = msg
            .content_json
            .as_deref()
            .expect("content_json should be present");
        let parsed: serde_json::Value =
            serde_json::from_str(content_json).expect("content_json should be valid json");
        let parts = parsed["parts"]
            .as_array()
            .expect("parts should be an array");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "file_ref");
        assert_eq!(
            msg.display_text.as_deref(),
            Some("[Attachments: sample.pdf]")
        );
    }

    #[test]
    fn persist_with_attachments_keeps_non_empty_text_part() {
        let (_tmp, db) = make_db();
        let persistence = GatewayPersistence::new(db.clone());

        let id = persistence
            .persist_user_message_with_attachments(
                "user2:gui",
                "please analyze",
                "gui",
                None,
                &[sample_attachment()],
            )
            .expect("persist message");
        assert!(id.message_id > 0);

        let repo = ConversationRepository::new(&db);
        let msgs = repo
            .list_recent_by_lane("user2:gui", 10)
            .expect("load recent messages");
        assert_eq!(msgs.len(), 1);
        let msg = &msgs[0];
        let content_json = msg
            .content_json
            .as_deref()
            .expect("content_json should be present");
        let parsed: serde_json::Value =
            serde_json::from_str(content_json).expect("content_json should be valid json");
        let parts = parsed["parts"]
            .as_array()
            .expect("parts should be an array");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "please analyze");
        assert_eq!(parts[1]["type"], "file_ref");
    }

    #[test]
    fn persist_with_attachments_stores_document_part_when_text_available() {
        let (_tmp, db) = make_db();
        let persistence = GatewayPersistence::new(db.clone());

        let id = persistence
            .persist_user_message_with_attachments(
                "user3:gui",
                "review this resume",
                "gui",
                None,
                &[sample_attachment_with_text(
                    "Candidate has 5 years experience",
                )],
            )
            .expect("persist message");
        assert!(id.message_id > 0);

        let repo = ConversationRepository::new(&db);
        let msgs = repo
            .list_recent_by_lane("user3:gui", 10)
            .expect("load recent messages");
        assert_eq!(msgs.len(), 1);
        let msg = &msgs[0];
        let content_json = msg
            .content_json
            .as_deref()
            .expect("content_json should be present");
        let parsed: serde_json::Value =
            serde_json::from_str(content_json).expect("content_json should be valid json");
        let parts = parsed["parts"]
            .as_array()
            .expect("parts should be an array");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[1]["type"], "document");
        assert_eq!(
            parts[1]["extracted_text"],
            "Candidate has 5 years experience"
        );
    }
}
