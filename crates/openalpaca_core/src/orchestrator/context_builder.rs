use super::working_state::WorkingState;
use super::{ConversationContext, Orchestrator};
use openalpaca_llm::{ChatMessage, ContentPart};
use openalpaca_storage::ConversationRepository;
use std::sync::Arc;

/// H1 — the one line that says where a replayed delegation came from.
///
/// A turn that delegated stores its assistant row carrying the run's id
/// (GAP-23, `gateway/persistence.rs`). Replayed as bare prose, "Started a
/// background workflow called … (task id: …)" is just a sentence the model can
/// imitate — and a local model did, in a turn that called no tool at all. The
/// line is appended to the **replayed copy only**: the stored row, the
/// transcript and everything the user sees are untouched, and an ordinary
/// assistant row gets nothing.
///
/// Deterministic and fixed: one line, no timestamps, no counts, so a cache
/// keyed on the lane tip still sees byte-identical history for the same rows.
/// One recent row on its way to becoming a [`ChatMessage`]: `(id, role,
/// content, content_json, task_id)`.
type RecentRow = (i64, String, String, Option<String>, Option<String>);

pub(super) fn delegation_provenance_line(task_id: &str) -> String {
    format!("[This run was started by a start_workflow tool call, which returned task {task_id}.]")
}

impl Orchestrator {
    /// Build the full conversation context for a turn: loads history, deduplicates
    /// the current user message (Bug A fix, D6), loads the rows between the
    /// working state's `through` and the recent window via ID-range query
    /// (fixes 120-window bug), and reads the working state from the session row.
    pub(super) fn build_context(&self, lane_key: &str, current_query: &str) -> ConversationContext {
        let db = match &self.db {
            Some(db) => db,
            None => return ConversationContext::default(),
        };

        let repo = ConversationRepository::new(db);

        // The lane's active session (migration 039). Everything below reads
        // that conversation's own transcript, so a new session on a lane
        // starts clean instead of inheriting the previous one's tail. `None`
        // — a lane that has never persisted a turn through the gateway —
        // falls back to the lane-wide read, which is what the rows written
        // before any session existed are reachable by.
        //
        // Step 1: the working state comes from the same read of the row
        // (design 11 §2), so the id the turn end writes to and the version it
        // locks on belong to one another.
        let session = repo
            .get_active_session_for_lane(lane_key)
            .unwrap_or_default();
        let session_id = session.as_ref().map(|s| s.id.clone());
        let (working_state, working_state_updated_at, version, through) = match &session {
            Some(s) => (
                stored_working_state(&s.working_state, &s.id),
                s.working_state_updated_at.clone().unwrap_or_default(),
                s.working_state_version,
                s.working_state_through,
            ),
            None => (None, String::new(), 0, 0),
        };

        // Step 2: Load recent messages (40, not 120)
        let dcfg = self.daemon_config.load();
        let recent_limit = dcfg.orchestrator.memory.prompt_recent_messages as i64;
        let raw_messages = match &session_id {
            Some(id) => repo.list_recent_by_session(id, recent_limit),
            None => repo.list_recent_by_lane(lane_key, recent_limit),
        };
        let raw_messages = match raw_messages {
            Ok(msgs) => msgs,
            Err(_) => return ConversationContext::default(),
        };

        // Step 3: Build canonical list and dedup current query
        // Include content_json for multimodal message reconstruction
        // `task_id` rides along (H1): an assistant row that carries one was the
        // turn that started a workflow, and the replay says so.
        let mut chat_rows: Vec<RecentRow> = raw_messages
            .iter()
            .filter(|msg| {
                (msg.role == "user" || msg.role == "assistant")
                    && (!msg.content.is_empty() || msg.content_json.is_some())
            })
            .map(|msg| {
                (
                    msg.id,
                    msg.role.clone(),
                    msg.content.clone(),
                    msg.content_json.clone(),
                    msg.task_id.clone(),
                )
            })
            .collect();

        // Dedup (D6) — if the last row matches current_query, drop it (Bug A fix).
        let should_dedup = chat_rows
            .last()
            .map(|(_, role, content, _, _)| role == "user" && content == current_query)
            .unwrap_or(false);
        if should_dedup {
            tracing::debug!("Dedup: dropping duplicate user message from recent window");
            chat_rows.pop();
        }

        // Design 11 §4 (c), CQ-4 — after a compaction only: when the state on
        // the row is the one a compacting turn wrote, the window starts after
        // the rows it covers, keeping `min_recent_messages` rows verbatim, so
        // a small window does not re-send (and re-compact) what the state
        // already carries. A lag refresh or a close leaves the window as it
        // is: their write changes the row, and the mark no longer matches.
        let compacted = session_id.as_ref().is_some_and(|id| {
            self.compacted_states
                .get(id)
                .is_some_and(|mark| *mark == (version, through))
        });
        if compacted {
            let floor = dcfg.execution.context.min_recent_messages;
            let after_through = chat_rows
                .iter()
                .position(|(id, _, _, _, _)| *id > through)
                .unwrap_or(chat_rows.len());
            let start = after_through.min(chat_rows.len().saturating_sub(floor));
            chat_rows.drain(..start);
        }

        // Step 4: Get first_recent_id for the ID-range query
        let first_recent_id = chat_rows
            .first()
            .map(|(id, _, _, _, _)| *id)
            .unwrap_or(i64::MAX);

        // Step 5: Load the rows the state does not cover yet and the window
        // does not hold, via ID-range query (fixes 120-window bug)
        let older_window = if through < first_recent_id {
            let older = match &session_id {
                Some(id) => repo.list_by_session_id_range(id, through, first_recent_id, 500),
                None => repo.list_by_lane_id_range(lane_key, through, first_recent_id, 500),
            };
            match older {
                Ok(msgs) => msgs
                    .into_iter()
                    .filter(|msg| {
                        (msg.role == "user" || msg.role == "assistant") && !msg.content.is_empty()
                    })
                    .map(|msg| (msg.id, msg.role, msg.content))
                    .collect(),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };

        // Step 6: Convert recent chat_rows to ChatMessage, restoring multimodal parts
        let recent_messages: Vec<ChatMessage> = chat_rows
            .iter()
            .map(|(_, role, content, content_json, task_id)| {
                let mut msg = match role.as_str() {
                    "user" => ChatMessage::user(content),
                    // H1 — a delegating assistant row replays with its
                    // provenance. Only this branch: a user row's `task_id`
                    // (there is none today) would be somebody else's fact.
                    _ => match task_id.as_deref().filter(|id| !id.is_empty()) {
                        Some(id) => ChatMessage::assistant(&format!(
                            "{content}\n{}",
                            delegation_provenance_line(id)
                        )),
                        None => ChatMessage::assistant(content),
                    },
                };
                // Reconstruct parts from content_json when present
                if let Some(json_str) = content_json
                    && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str)
                    && let Some(parts_arr) = parsed.get("parts")
                    && let Ok(parts) = serde_json::from_value::<Vec<ContentPart>>(parts_arr.clone())
                {
                    msg.parts = Some(parts);
                }
                msg
            })
            .collect();

        ConversationContext {
            session_id,
            working_state,
            working_state_updated_at,
            working_state_version: version,
            working_state_through: through,
            recent_messages,
            recent_row_ids: chat_rows.iter().map(|(id, ..)| *id).collect(),
            older_window,
        }
    }
}

/// The session row's `working_state` as a state: `None` while it is empty,
/// and — logged — when the text is not one, so the next write starts over
/// rather than feeding the writer something it cannot read.
fn stored_working_state(text: &str, session_id: &str) -> Option<Arc<WorkingState>> {
    if text.is_empty() {
        return None;
    }
    match serde_json::from_str::<WorkingState>(text) {
        Ok(state) => Some(Arc::new(state)),
        Err(e) => {
            tracing::warn!(
                session_id,
                "the session's working state is not readable, ignored: {e}"
            );
            None
        }
    }
}
