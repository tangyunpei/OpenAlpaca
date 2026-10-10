//! Gateway-level message persistence
//!
//! Persists user and assistant messages at the Gateway layer so ALL sources
//! (GUI, Telegram, CLI, etc.) get automatic conversation persistence.

use crate::gateway::router::ResolvedAttachment;
use crate::memory::learning::is_slash_turn;
use crate::utils::text::one_line;
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
    /// The row read said `title = ''` — the one case a turn names it (G5).
    untitled: bool,
}

/// The most characters of a message a conversation's title keeps (G5).
const TITLE_MAX_CHARS: usize = 60;

/// A conversation's title from one of its user messages (G5, Round G): the
/// first line with anything to read on it, as [`one_line`] leaves it (hidden
/// and control characters dropped, whitespace collapsed), cut to at most
/// [`TITLE_MAX_CHARS`] characters — at the last space that fits when the words
/// before it keep at least half the limit, else at the limit itself (CJK text
/// has no spaces, and a first word followed by a URL or a run of Chinese would
/// otherwise leave the title one word long) — and followed by `…` when cut.
/// Characters, never bytes: a cut cannot land inside one.
///
/// `None` for a message that starts with `/` — a slash command (`/remember …`,
/// `/status`), by [`is_slash_turn`], the test learning leaves such a turn out
/// by — and for a message with no text (a turn of attachments alone): neither
/// says what the conversation is about, and the next user message is tried at
/// its own turn.
fn derive_title(message: &str) -> Option<String> {
    if is_slash_turn(message) {
        return None;
    }
    let line = message
        .lines()
        .map(one_line)
        .find(|line| !line.is_empty())?;
    if line.chars().count() <= TITLE_MAX_CHARS {
        return Some(line);
    }
    let kept: String = line.chars().take(TITLE_MAX_CHARS).collect();
    // `one_line` leaves single spaces only, so a space is the word boundary;
    // the kept text already ends on one when the next character is a space.
    let at_word_end = line.chars().nth(TITLE_MAX_CHARS) == Some(' ');
    // The words before the last space that fits, kept only when they hold at
    // least half the limit.
    let words = kept.rfind(' ').map(|space| &kept[..space]);
    let cut = match words {
        Some(words) if !at_word_end && words.chars().count() >= TITLE_MAX_CHARS / 2 => words,
        _ => kept.as_str(),
    };
    Some(format!("{cut}…"))
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
                untitled: fresh.title.is_empty(),
                id: fresh.id,
                project_switched: true,
            });
        }
        let session = repo.get_or_create_active_session(lane_key, source, workspace_path)?;
        Ok(TurnSession {
            untitled: session.title.is_empty(),
            id: session.id,
            project_switched: false,
        })
    }

    /// Name an untitled conversation after its user's words (G5, Round G):
    /// [`derive_title`] of the first of its user rows that yields one — this
    /// turn's own message for a new conversation, the first stored one for a
    /// conversation an older build left untitled. Runs after the turn's row is
    /// written, and only when the session's row said `title = ''`, so a named
    /// conversation costs nothing; the write is guarded in its statement
    /// ([`ConversationRepository::set_title_if_empty`]), so a rename is never
    /// overwritten and two turns cannot both name it.
    ///
    /// A failure is logged, never returned: the user's message is already
    /// stored, and a conversation without a name is not a failed turn.
    fn title_if_untitled(&self, repo: &ConversationRepository<'_>, session: &TurnSession) {
        if !session.untitled {
            return;
        }
        let named = repo
            .title_from_user_rows(&session.id, derive_title)
            .and_then(|title| match title {
                Some(title) => repo.set_title_if_empty(&session.id, &title),
                None => Ok(false),
            });
        if let Err(e) = named {
            tracing::warn!(session_id = %session.id, "could not name the conversation: {e}");
        }
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
    ///
    /// `request_id` is the turn's id and `workspace_path` its project's root —
    /// canonical when it was recorded, as written once the project is missing
    /// (ST-23);
    /// both are written on the row (migration 044, design 09 §1.3) — the id
    /// marks the row as a turn's, and the root is the project this row
    /// learns into, whatever the session's binding says. `None` for a row no
    /// turn wrote.
    pub fn persist_user_message(
        &self,
        lane_key: &str,
        content: &str,
        source: &str,
        workspace_path: Option<&str>,
        request_id: Option<uuid::Uuid>,
    ) -> Result<PersistedUserMessage> {
        let repo = ConversationRepository::new(&self.db);
        let session = self.resolve_turn_session(&repo, lane_key, source, workspace_path)?;
        let message_id = repo.insert(&ConversationMessage {
            lane_key: lane_key.to_string(),
            role: "user".to_string(),
            content: content.to_string(),
            source: Some(source.to_string()),
            session_id: Some(session.id.clone()),
            request_id: request_id.map(|id| id.to_string()),
            workspace_root: workspace_path.map(str::to_string),
            ..Default::default()
        })?;
        repo.increment_message_count_for_session(&session.id)?;
        self.title_if_untitled(&repo, &session);
        Ok(PersistedUserMessage {
            message_id,
            session_id: session.id,
            project_switched: session.project_switched,
        })
    }

    /// Persist a user message with file attachments. `request_id` and
    /// `workspace_path` as for [`Self::persist_user_message`].
    pub fn persist_user_message_with_attachments(
        &self,
        lane_key: &str,
        content: &str,
        source: &str,
        workspace_path: Option<&str>,
        attachments: &[ResolvedAttachment],
        request_id: Option<uuid::Uuid>,
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
            request_id: request_id.map(|id| id.to_string()),
            workspace_root: workspace_path.map(str::to_string),
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
        self.title_if_untitled(&repo, &session);
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
    /// message the delegation came from. It is `None` for ordinary chat.
    ///
    /// `session_id` is the turn's session, as the user-message persist
    /// resolved it. It is passed explicitly — the same way
    /// `persist_completion_report` pins — because the handler in between takes
    /// as long as an agentic loop takes, and re-resolving the lane here would
    /// file the answer in whatever conversation a mid-turn "New chat" left
    /// active. `None` (a turn whose user half failed to persist) falls back to
    /// the lane's active session.
    ///
    /// The answer arrives as the whole [`HandleResult`], not as a fan of
    /// positional pieces: the two fields read off it — the run a delegating
    /// turn started, and the model that answered — both have to come from
    /// *this* result rather than from whatever the lane happens to be running
    /// or defaulting to, and taking the struct is what makes that structural.
    /// `model` fills a column that has always existed and that the transcript
    /// has always rendered, but that nothing wrote: a reload dropped the
    /// answer's model on the floor. GAP-13 makes that visible, since a turn can
    /// now run on a model the picker is not showing. It stays `None` on the
    /// non-LLM paths, which is honest — a slash command was answered by no
    /// model at all.
    ///
    /// `tokens_in`/`tokens_out` are the **turn's** usage, summed over every
    /// round of the main loop (`LoopResult::total_input_tokens`, carried here
    /// through `LlmMetadata`), not the last round's. They used to be dropped,
    /// so every reloaded assistant row read `0/0` while `llm_call_log` held the
    /// real numbers (G4). `None` on the non-LLM paths — a slash command spent
    /// no tokens, which is a different fact from "spent none that we counted",
    /// and the column stays NULL so a client can tell them apart.
    ///
    /// `request_id` is the turn's id (migration 044): the answer carries the
    /// same id as its question, which is how learning pairs them. It never
    /// carries a root — an answer learns under its question's.
    pub fn persist_assistant_message(
        &self,
        lane_key: &str,
        result: &super::HandleResult,
        duration_ms: Option<i64>,
        source: &str,
        session_id: Option<&str>,
        request_id: Option<uuid::Uuid>,
    ) -> Result<i64> {
        let content = result.content.as_str();
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
            model: result.model.clone(),
            tokens_in: result.tokens_in.map(i64::from),
            tokens_out: result.tokens_out.map(i64::from),
            duration_ms,
            task_id: result.delegation.as_ref().map(|d| d.task_id.clone()),
            session_id: session_id.map(str::to_string),
            request_id: request_id.map(|id| id.to_string()),
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
            .persist_user_message_with_attachments(
                "user1:gui",
                "",
                "gui",
                None,
                &[sample_attachment()],
                None,
            )
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
                None,
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
                None,
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

    /// **G4.** The turn's usage — summed over every round of the main loop by
    /// the time it reaches `HandleResult` — is written to the row, so a
    /// reloaded transcript shows what the turn cost instead of `0/0`.
    #[test]
    fn an_assistant_row_keeps_the_turns_token_counts() {
        let (_tmp, db) = make_db();
        let persistence = GatewayPersistence::new(db.clone());

        let id = persistence
            .persist_assistant_message(
                "user4:gui",
                &super::super::HandleResult {
                    content: "the answer".to_string(),
                    model: Some("qwen3:8b".to_string()),
                    // Two rounds' worth: 2 279 + 5 342 in, 209 + 129 out.
                    tokens_in: Some(7621),
                    tokens_out: Some(338),
                    attachments_used: Vec::new(),
                    attachments_skipped: Vec::new(),
                    memory_used: Vec::new(),
                    delegation: None,
                    continuity: Default::default(),
                },
                Some(1234),
                "gui",
                None,
                None,
            )
            .expect("persist assistant message");
        assert!(id > 0);

        let repo = ConversationRepository::new(&db);
        let msgs = repo
            .list_recent_by_lane("user4:gui", 10)
            .expect("load recent messages");
        let msg = msgs.iter().find(|m| m.role == "assistant").expect("row");
        assert_eq!(msg.tokens_in, Some(7621), "input tokens are persisted");
        assert_eq!(msg.tokens_out, Some(338), "output tokens are persisted");
        assert_eq!(msg.model.as_deref(), Some("qwen3:8b"));
    }

    /// A turn no model answered (a slash command) records no usage: NULL, not
    /// a zero that would read as "the model spent nothing".
    #[test]
    fn a_non_llm_answer_records_no_token_counts() {
        let (_tmp, db) = make_db();
        let persistence = GatewayPersistence::new(db.clone());

        persistence
            .persist_assistant_message(
                "user5:gui",
                &super::super::HandleResult::text("3 tasks are running".to_string()),
                None,
                "cli",
                None,
                None,
            )
            .expect("persist assistant message");

        let repo = ConversationRepository::new(&db);
        let msgs = repo
            .list_recent_by_lane("user5:gui", 10)
            .expect("load recent messages");
        let msg = msgs.iter().find(|m| m.role == "assistant").expect("row");
        assert_eq!(msg.tokens_in, None);
        assert_eq!(msg.tokens_out, None);
    }

    /// **V3, the other half.** An empty answer writes no row at all — which is
    /// why a turn that reached no answer left no trace in the transcript. The
    /// runtime line the main loop now returns instead is an ordinary answer
    /// here, and gets an ordinary row.
    #[test]
    fn a_runtime_no_answer_line_is_persisted_and_an_empty_one_is_not() {
        let (_tmp, db) = make_db();
        let persistence = GatewayPersistence::new(db.clone());

        let empty = persistence
            .persist_assistant_message(
                "user6:gui",
                &super::super::HandleResult::text(String::new()),
                None,
                "gui",
                None,
                None,
            )
            .expect("an empty answer is not an error");
        assert_eq!(empty, 0, "nothing was written");

        let line = "I stopped after 8 tool rounds without reaching an answer.";
        let id = persistence
            .persist_assistant_message(
                "user6:gui",
                &super::super::HandleResult::text(line.to_string()),
                None,
                "gui",
                None,
                None,
            )
            .expect("persist assistant message");
        assert!(id > 0);

        let repo = ConversationRepository::new(&db);
        let msgs = repo
            .list_recent_by_lane("user6:gui", 10)
            .expect("load recent messages");
        let msg = msgs.iter().find(|m| m.role == "assistant").expect("row");
        assert_eq!(msg.content, line, "the transcript keeps what the user read");
    }

    // ── G5 (Round G): a conversation is named from the user's first message ──

    fn title_of(db: &Database, session_id: &str) -> String {
        ConversationRepository::new(db)
            .get_session(session_id)
            .expect("read the session")
            .expect("the session exists")
            .title
    }

    fn user_turn(persistence: &GatewayPersistence, lane: &str, content: &str) -> String {
        persistence
            .persist_user_message(lane, content, "gui", None, None)
            .expect("persist the user message")
            .session_id
    }

    #[test]
    fn the_first_user_message_titles_the_session_and_the_second_does_not() {
        let (_tmp, db) = make_db();
        let persistence = GatewayPersistence::new(db.clone());

        let session = user_turn(&persistence, "user7:gui", "Plan pasta night for six");
        assert_eq!(title_of(&db, &session), "Plan pasta night for six");

        let again = user_turn(&persistence, "user7:gui", "And a dessert, please");
        assert_eq!(again, session, "the same conversation");
        assert_eq!(title_of(&db, &session), "Plan pasta night for six");
    }

    #[test]
    fn a_renamed_session_is_never_retitled() {
        let (_tmp, db) = make_db();
        let persistence = GatewayPersistence::new(db.clone());

        // A command names nothing, so the conversation is still untitled
        // when the user renames it.
        let session = user_turn(&persistence, "user8:gui", "/status");
        assert_eq!(title_of(&db, &session), "");
        assert!(
            ConversationRepository::new(&db)
                .update_session(&session, Some("My dinner plans"), None)
                .expect("rename")
        );

        user_turn(&persistence, "user8:gui", "Plan pasta night for six");
        assert_eq!(title_of(&db, &session), "My dinner plans");
    }

    #[test]
    fn a_slash_command_and_an_attachment_only_turn_title_nothing_and_the_next_sentence_does() {
        let (_tmp, db) = make_db();
        let persistence = GatewayPersistence::new(db.clone());

        let session = user_turn(&persistence, "user9:gui", "/remember I cook for six");
        assert_eq!(title_of(&db, &session), "", "a command is not a title");

        let files = persistence
            .persist_user_message_with_attachments(
                "user9:gui",
                "",
                "gui",
                None,
                &[sample_attachment()],
                None,
            )
            .expect("persist an attachment-only turn");
        assert_eq!(files.session_id, session);
        assert_eq!(
            title_of(&db, &session),
            "",
            "a file alone is not a title, nor is its `[Attachments: …]` text"
        );

        user_turn(&persistence, "user9:gui", "Plan pasta night for six");
        assert_eq!(title_of(&db, &session), "Plan pasta night for six");
    }

    /// Characters, never bytes: a 200-character message with no space in it
    /// is cut at its 60th character. The leading ASCII letter puts byte 60
    /// inside a three-byte character, so a cut by bytes would panic there
    /// (and one at a boundary would keep 20 characters, not 60).
    #[test]
    fn a_long_chinese_message_is_cut_at_60_characters() {
        let (_tmp, db) = make_db();
        let persistence = GatewayPersistence::new(db.clone());
        let message: String = std::iter::once('A')
            .chain("我想为六个人计划一个意大利面之夜".chars().cycle())
            .take(200)
            .collect();
        assert!(!message.is_char_boundary(60), "byte 60 is mid-character");

        let session = user_turn(&persistence, "user10:gui", &message);

        let expected: String = message.chars().take(60).chain(['…']).collect();
        assert_eq!(title_of(&db, &session), expected);
    }

    /// A turn of a file and a sentence is named by the sentence: the
    /// attachment writer runs the same title step as the plain one.
    #[test]
    fn a_turn_with_a_file_and_a_sentence_is_named_by_the_sentence() {
        let (_tmp, db) = make_db();
        let persistence = GatewayPersistence::new(db.clone());

        let turn = persistence
            .persist_user_message_with_attachments(
                "user12:gui",
                "Summarize this resume for me",
                "gui",
                None,
                &[sample_attachment_with_text("Jane Doe, engineer")],
                None,
            )
            .expect("persist a turn with a file");

        assert_eq!(
            title_of(&db, &turn.session_id),
            "Summarize this resume for me"
        );
    }

    /// A follow-up the daemon re-enters on the user's behalf is stored as a
    /// `user` row of source `internal` — the assistant's own words — and
    /// names nothing; the user's next sentence does.
    #[test]
    fn a_follow_ups_re_entered_turn_names_nothing() {
        let (_tmp, db) = make_db();
        let persistence = GatewayPersistence::new(db.clone());

        let session = user_turn(&persistence, "user13:gui", "/status");
        let followup = persistence
            .persist_user_message(
                "user13:gui",
                "Check the build again tomorrow and report back",
                crate::memory::learning::INTERNAL_SOURCE,
                None,
                None,
            )
            .expect("persist the follow-up's turn");
        assert_eq!(followup.session_id, session);
        assert_eq!(title_of(&db, &session), "", "not the user's words");

        user_turn(&persistence, "user13:gui", "Plan pasta night for six");
        assert_eq!(title_of(&db, &session), "Plan pasta night for six");
    }

    /// A conversation an older build left untitled is named from its first
    /// stored user row, not from the turn that happens to come next.
    #[test]
    fn an_older_untitled_session_takes_its_first_stored_user_row() {
        let (_tmp, db) = make_db();
        let repo = ConversationRepository::new(&db);
        let session = repo
            .get_or_create_active_session("user11:gui", "gui", None)
            .expect("an untitled session");
        for (role, content) in [
            ("user", "Plan the Lisbon trip"),
            ("assistant", "Happy to. When are you going?"),
            ("user", "In October"),
        ] {
            repo.insert(&ConversationMessage {
                lane_key: "user11:gui".to_string(),
                role: role.to_string(),
                content: content.to_string(),
                session_id: Some(session.id.clone()),
                ..Default::default()
            })
            .expect("an older row");
        }
        assert_eq!(title_of(&db, &session.id), "");

        let persistence = GatewayPersistence::new(db.clone());
        let turn = user_turn(&persistence, "user11:gui", "What about flights?");

        assert_eq!(turn, session.id);
        assert_eq!(title_of(&db, &session.id), "Plan the Lisbon trip");
    }

    #[test]
    fn derive_title_reads_the_first_line_on_one_line_and_cuts_at_a_word() {
        // The first line with anything on it, its hidden characters dropped
        // and its whitespace collapsed.
        assert_eq!(
            derive_title("\n  \n Plan\u{202E} pasta\tnight \nfor six").as_deref(),
            Some("Plan pasta night")
        );
        // Over the limit: cut at the last space that fits, then `…`.
        let long = "Plan a pasta night for six people with a starter a main course \
                    and a dessert";
        let title = derive_title(long).expect("a title");
        assert_eq!(
            title,
            "Plan a pasta night for six people with a starter a main…"
        );
        assert!(title.chars().count() <= 61);
        // A cut that lands on a word's end keeps the whole word.
        let exact = format!("{} tail", "a".repeat(60));
        assert_eq!(derive_title(&exact), Some(format!("{}…", "a".repeat(60))));
        // The last space is used only when the words before it keep at least
        // half the limit (30 characters); otherwise the cut is at the limit.
        let thirty = format!("{} {}", "a".repeat(30), "b".repeat(40));
        assert_eq!(derive_title(&thirty), Some(format!("{}…", "a".repeat(30))));
        let twenty_nine = format!("{} {}", "a".repeat(29), "b".repeat(41));
        assert_eq!(
            derive_title(&twenty_nine),
            Some(format!("{} {}…", "a".repeat(29), "b".repeat(30)))
        );
        // So one space near the start leaves more than its first word: in
        // Chinese around an English word, after a full-width space (U+3000,
        // which `one_line` makes a space), and before a URL.
        assert_eq!(
            derive_title(
                "帮我用 Rust写一个能够解析命令行参数的小工具，支持子命令、默认值、\
                 环境变量覆盖和自动生成的帮助信息，并且附上完整的单元测试"
            )
            .as_deref(),
            Some(
                "帮我用 Rust写一个能够解析命令行参数的小工具，支持子命令、默认值、\
                 环境变量覆盖和自动生成的帮助信息，并且附上完整的单…"
            )
        );
        assert_eq!(
            derive_title(
                "你好\u{3000}我想为六个人计划一个意大利面之夜，需要一份购物清单、\
                 每道菜的做法和时间安排，还要考虑一位素食者和一位对坚果过敏的朋友"
            )
            .as_deref(),
            Some(
                "你好 我想为六个人计划一个意大利面之夜，需要一份购物清单、\
                 每道菜的做法和时间安排，还要考虑一位素食者和一位对坚果过敏的朋…"
            )
        );
        assert_eq!(
            derive_title(
                "Summarize https://example.com/a/very/long/path/to/a/report/that/\
                 keeps/going/and/going.html for me"
            )
            .as_deref(),
            Some("Summarize https://example.com/a/very/long/path/to/a/report/t…")
        );
        // Characters, not bytes: 27 three-byte characters are 81 bytes, under
        // the limit all the same, and are not cut.
        let short_chinese = "我想为六个人计划一个意大利面之夜还要一份购物清单和做法";
        assert_eq!(
            (short_chinese.chars().count(), short_chinese.len()),
            (27, 81)
        );
        assert_eq!(derive_title(short_chinese).as_deref(), Some(short_chinese));
        // Nothing to read, or a message that starts with `/` (a command, or
        // a path): no title.
        assert_eq!(derive_title(""), None);
        assert_eq!(derive_title(" \n\u{200B}\n "), None);
        assert_eq!(derive_title("  /status"), None);
        assert_eq!(derive_title("/usr/local/bin is not on my PATH"), None);
    }
}
