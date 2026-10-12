use super::working_state::{self, PreviousConversation};
use super::{ConversationContext, Orchestrator};
use openalpaca_llm::{ChatMessage, ContentPart};
use openalpaca_storage::ConversationRepository;

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

/// A delegating row's replayed text: `content` with its provenance line
/// appended — once. A model that saw the line on a replayed row can echo it
/// into its own answer, and that answer is stored; a row whose text already
/// ends with its line (trailing whitespace aside) replays as stored, so the
/// history never carries the line twice (G3b). The stored row is untouched
/// either way.
fn with_delegation_provenance(content: &str, task_id: &str) -> String {
    let line = delegation_provenance_line(task_id);
    if content.trim_end().ends_with(&line) {
        content.to_string()
    } else {
        format!("{content}\n{line}")
    }
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
                working_state::stored(&s.working_state, &s.id),
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

        // Design 11 §4 (b), owner decision CQ-5: a conversation that has no
        // state of its own yet is given the previous one on this lane and
        // project — read live, every turn — before its `carry_over_turns`-th
        // user turn has been answered, unless it was started fresh.
        let previous = match &session {
            Some(s) if s.inherit && working_state.is_none() => {
                let user_turns = chat_rows
                    .iter()
                    .filter(|(_, role, ..)| role == "user")
                    .count();
                (user_turns < dcfg.orchestrator.memory.carry_over_turns)
                    .then(|| {
                        previous_conversation(
                            &repo,
                            lane_key,
                            s.workspace_id.as_deref(),
                            recent_limit,
                        )
                    })
                    .flatten()
            }
            _ => None,
        };

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
                        Some(id) => {
                            ChatMessage::assistant(&with_delegation_provenance(content, id))
                        }
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
            previous,
        }
    }
}

/// The previous conversation on `lane_key` and `workspace_id` (design 11 §4
/// (b)): the lane's newest archived session there, with its state and its
/// last exchange after what that state covers — or nothing, when there is no
/// such session or it has neither (an older session is never read instead).
fn previous_conversation(
    repo: &ConversationRepository<'_>,
    lane_key: &str,
    workspace_id: Option<&str>,
    recent_limit: i64,
) -> Option<PreviousConversation> {
    let session = repo.previous_session(lane_key, workspace_id).ok()??;
    let state = working_state::stored(&session.working_state, &session.id);
    let rows: Vec<_> = repo
        .list_recent_by_session(&session.id, recent_limit)
        .unwrap_or_default()
        .into_iter()
        .filter(|m| {
            m.id > session.working_state_through
                && (m.role == "user" || m.role == "assistant")
                && !m.content.is_empty()
        })
        .collect();
    let exchange: Vec<(String, String)> = match rows.iter().rposition(|m| m.role == "user") {
        Some(i) => std::iter::once(&rows[i])
            .chain(rows[i + 1..].iter().find(|m| m.role == "assistant"))
            .map(|m| (m.role.clone(), working_state::exchange_text(&m.content)))
            .collect(),
        None => Vec::new(),
    };
    if state.is_none() && exchange.is_empty() {
        return None;
    }
    let ended_at = session.ended_at.unwrap_or(session.updated_at);
    let date = session
        .last_message_at
        .as_deref()
        .unwrap_or(&ended_at)
        .chars()
        .take(10)
        .collect();
    Some(PreviousConversation {
        session_id: session.id,
        ended_at,
        date,
        episode_note_id: session.episode_note_id,
        state,
        exchange,
    })
}
