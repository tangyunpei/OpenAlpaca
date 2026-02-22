use super::{ConversationContext, Orchestrator};
use openalpaca_llm::ChatMessage;
use openalpaca_storage::ConversationRepository;

impl Orchestrator {
    /// Build the full conversation context for a turn: loads history, deduplicates
    /// the current user message (Bug A fix, D6), loads unsummarized older messages
    /// via ID-range query (fixes 120-window bug), and loads the summary from the
    /// conversations table.
    pub(super) fn build_context(&self, lane_key: &str, current_query: &str) -> ConversationContext {
        let empty = ConversationContext {
            summary: None,
            recent_messages: Vec::new(),
            older_window: Vec::new(),
            summary_version: 0,
            last_summarized_id: 0,
            old_summary_text: String::new(),
        };
        let db = match &self.db {
            Some(db) => db,
            None => return empty,
        };

        let repo = ConversationRepository::new(db);

        // Step 1: Load summary from conversations table
        let (summary_text, summary_version, last_summarized_id) =
            repo.get_summary(lane_key).unwrap_or_default();

        // Step 2: Load recent messages (40, not 120)
        let dcfg = self.daemon_config.load();
        let raw_messages = match repo.list_recent_by_lane(
            lane_key,
            dcfg.orchestrator.memory.prompt_recent_messages as i64,
        ) {
            Ok(msgs) => msgs,
            Err(_) => return empty,
        };

        // Step 3: Build canonical list and dedup current query
        let mut chat_rows: Vec<(i64, String, String)> = raw_messages
            .iter()
            .filter(|msg| {
                (msg.role == "user" || msg.role == "assistant") && !msg.content.is_empty()
            })
            .map(|msg| (msg.id, msg.role.clone(), msg.content.clone()))
            .collect();

        // Dedup (D6) — if the last row matches current_query, drop it (Bug A fix).
        let should_dedup = chat_rows
            .last()
            .map(|(_, role, content)| role == "user" && content == current_query)
            .unwrap_or(false);
        if should_dedup {
            tracing::debug!("Dedup: dropping duplicate user message from recent window");
            chat_rows.pop();
        }

        // Step 4: Get first_recent_id for the ID-range query
        let first_recent_id = chat_rows.first().map(|(id, _, _)| *id).unwrap_or(i64::MAX);

        // Step 5: Load unsummarized older messages via ID-range query (fixes 120-window bug)
        let older_window = if last_summarized_id < first_recent_id {
            match repo.list_by_lane_id_range(lane_key, last_summarized_id, first_recent_id, 500) {
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

        // Step 6: Convert recent chat_rows to ChatMessage
        let recent_messages: Vec<ChatMessage> = chat_rows
            .iter()
            .map(|(_, role, content)| match role.as_str() {
                "user" => ChatMessage::user(content),
                _ => ChatMessage::assistant(content),
            })
            .collect();

        let summary = if summary_text.is_empty() {
            None
        } else {
            Some(summary_text.clone())
        };

        ConversationContext {
            summary,
            recent_messages,
            older_window,
            summary_version,
            last_summarized_id,
            old_summary_text: summary_text,
        }
    }
}
