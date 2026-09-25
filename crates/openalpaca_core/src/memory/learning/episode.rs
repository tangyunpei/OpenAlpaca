//! Conversation summaries (design 09 §13.3, owner decision MD-16, slice
//! L10c): one `episode` note per stretch of conversation — what was
//! discussed, what was decided, what was left open — findable by search, the
//! timeline, the list and the Memory view, and never injected on its own.
//!
//! The learning queue decides **when** (at least four of the user's turns
//! since the session's last summary; a quiet period of 30 minutes, a switch
//! of conversation, the stop drain and its boot catch-up). This module is
//! **what**: the prompt, the answer's shape, the note, and the one blocking
//! task that writes it and moves the session's episode cursor — the same
//! at-least-once shape as learning's store-and-advance (design 09 §1.4).
//!
//! The note:
//! - title `Conversation on <YYYY-MM-DD>: <topic>`, the date the last turn's;
//! - `## Knowledge`: what was discussed, decided and left open, at most
//!   1 KiB of prose;
//! - `observed_at` the last turn's time; `sources` the window's message rows
//!   (at most eight, spread from its first to its last — design 03 §7a's
//!   cap on `sources`); no `fact_key`, no aliases, no `supersedes`;
//! - origin `extracted_other`, status `active`
//!   ([`crate::memory::service::ExtractionProvenance::Summary`]).

use super::EPISODE_MAX_KNOWLEDGE_BYTES;
use super::extractor::{StoreEnd, fill_template, one_line, root_gone};
use super::window::{LearnRow, RowKind};
use crate::memory::service::{
    ExtractionProvenance, MemoryAccessContext, MemoryError, MemoryService, StoreInput,
};
use chrono::{DateTime, NaiveDateTime, Utc};
use openalpaca_llm::{ChatMessage, RequestContext, RouterRequest};
use openalpaca_storage::memory_files::{Dedup, MAX_SOURCES, NoteKind, NoteSource, SourceType};
use openalpaca_storage::{ConversationRepository, Database, LearningCursorRepository};
use std::path::PathBuf;
use std::sync::Arc;

/// The usage label conversation summaries are billed and gated under: their
/// own daily count (`episode_max_per_day`), and — summed with chat
/// learning's — the extraction's daily caps (design 09 §13.3, §1.8).
pub(crate) const EPISODE_LABEL: &str = "orchestrator_episode";

/// The prompt, committed verbatim: the system text, a `=== user ===` line,
/// then the user message's template (`the_episode_prompt_matches_its_fixture`).
const PROMPT: &str = include_str!("episode_prompt.txt");
const USER_MARKER: &str = "=== user ===\n";
/// A topic is cut to this many characters, so the title stays one short line.
const MAX_TOPIC_CHARS: usize = 60;

/// The system text and the rendered user message.
pub(crate) fn render_prompt(today: &str, conversation: &str) -> (String, String) {
    let (system, template) = PROMPT
        .split_once(USER_MARKER)
        .expect("episode_prompt.txt has a `=== user ===` line");
    let user = fill_template(template, |name| match name {
        "today" => Some(today),
        "conversation" => Some(conversation),
        _ => None,
    });
    (system.trim_end().to_string(), user.trim_end().to_string())
}

/// One episode's request: the fixture's prompt, `thinking: Disabled` (M2),
/// temperature 0, no caching and no fallbacks — as learning's.
pub(crate) fn build_request(
    today: &str,
    conversation: &str,
    out_tokens: u32,
    model: Option<String>,
) -> RouterRequest {
    let (system, user) = render_prompt(today, conversation);
    RouterRequest {
        model,
        messages: Arc::new(vec![ChatMessage::system(&system), ChatMessage::user(&user)]),
        tools: Arc::new(vec![]),
        temperature: Some(0.0),
        max_tokens: Some(out_tokens),
        context: RequestContext {
            agent_id: Some(EPISODE_LABEL.to_string()),
            task_id: None,
        },
        tool_choice: None,
        tools_token_estimate: None,
        enable_caching: false,
        thinking: Some(openalpaca_llm::ThinkingConfig::Disabled),
        context_management: None,
        fallback_models: Vec::new(),
        ephemeral_system_notice: None,
    }
}

/// What the model answered, cleaned: one line each; `decided` and `open`
/// empty when nothing was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Summary {
    pub topic: String,
    pub discussed: String,
    pub decided: String,
    pub open: String,
}

/// A value that says there is nothing: the model was told to answer `""`,
/// and small models answer "none" as often.
fn says_nothing(text: &str) -> bool {
    let t = text
        .trim()
        .trim_end_matches(['.', '。'])
        .trim()
        .to_lowercase();
    matches!(
        t.as_str(),
        "" | "none" | "nothing" | "n/a" | "na" | "-" | "—" | "null" | "无" | "没有" | "暂无"
    )
}

/// The answer must be one JSON object whose `topic` and `discussed` are
/// non-empty strings; `decided` and `open` are optional strings. Anything
/// else is `Err` — a reachable model's unusable answer (ruling LR20).
pub(crate) fn parse_summary(content: &str) -> Result<Summary, String> {
    let Some(value) = crate::memory::task_extraction::parse_json_response(content) else {
        return Err("the answer is not JSON".to_string());
    };
    if !value.is_object() {
        return Err("the answer is not one JSON object".to_string());
    }
    let text = |key: &str| -> Result<String, String> {
        match value.get(key) {
            None | Some(serde_json::Value::Null) => Ok(String::new()),
            Some(serde_json::Value::String(s)) => Ok(one_line(s)),
            Some(_) => Err(format!("`{key}` is not a string")),
        }
    };
    let topic = text("topic")?;
    let discussed = text("discussed")?;
    if says_nothing(&topic) || says_nothing(&discussed) {
        return Err("the answer names no topic or nothing discussed".to_string());
    }
    let decided = text("decided")?;
    let open = text("open")?;
    let keep = |s: String| if says_nothing(&s) { String::new() } else { s };
    Ok(Summary {
        topic,
        discussed,
        decided: keep(decided),
        open: keep(open),
    })
}

/// `Conversation on <YYYY-MM-DD>: <topic>`, the topic cut to 60 characters.
pub(crate) fn title(date: &str, topic: &str) -> String {
    let topic: String = topic.chars().take(MAX_TOPIC_CHARS).collect();
    format!("Conversation on {date}: {}", topic.trim())
}

/// The `## Knowledge` prose: one line each for what was discussed, decided
/// and left open (the last two only when there was something), at most
/// [`EPISODE_MAX_KNOWLEDGE_BYTES`] — cut on a character boundary, the cut
/// marked with `…`.
pub(crate) fn knowledge(summary: &Summary) -> String {
    let mut lines = vec![format!("Discussed: {}", summary.discussed)];
    if !summary.decided.is_empty() {
        lines.push(format!("Decided: {}", summary.decided));
    }
    if !summary.open.is_empty() {
        lines.push(format!("Left open: {}", summary.open));
    }
    let text = lines.join("\n");
    if text.len() <= EPISODE_MAX_KNOWLEDGE_BYTES {
        return text;
    }
    let cut = text.floor_char_boundary(EPISODE_MAX_KNOWLEDGE_BYTES - '…'.len_utf8());
    format!("{}…", text[..cut].trim_end())
}

/// The note's body (design 03 §4's sections): the knowledge, then whose
/// words these are and where the note is found.
pub(crate) fn body(title: &str, knowledge: &str, date: &str, user_turns: usize) -> String {
    let turns = if user_turns == 1 {
        "1 turn".to_string()
    } else {
        format!("{user_turns} turns")
    };
    format!(
        "# {title}\n\n## Knowledge\n{knowledge}\n\n## Evidence and limits\n\
         A summary the assistant wrote of {turns} of the conversation on {date}; not said by \
         you. It is never recalled on its own: memory search, the memory timeline and the \
         Memory view find it.\n"
    )
}

/// UTC, SQLite's `YYYY-MM-DD HH:MM:SS`.
fn row_time(created_at: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(created_at, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|t| t.and_utc())
}

/// The last turn's time: the note's `observed_at`.
pub(crate) fn observed_at(rows: &[LearnRow]) -> Option<DateTime<Utc>> {
    rows.last().and_then(|r| row_time(&r.created_at))
}

/// The user's own turns among `rows` (design 09 §13.3's "at least 4 user
/// turns"): typed rows, long or not — never the assistant's own follow-up
/// text or an answer.
pub(crate) fn user_turns(rows: &[LearnRow]) -> usize {
    rows.iter()
        .filter(|r| !r.context && matches!(r.kind, RowKind::User | RowKind::UserLong))
        .count()
}

/// The window's message rows as the note's `sources`: all of them when they
/// fit design 03 §7a's cap of eight, else eight spread evenly from the first
/// to the last — every one a row of this session, which is what deleting the
/// conversation forgets the summary by.
pub(crate) fn sources(rows: &[LearnRow]) -> Vec<NoteSource> {
    let rows: Vec<&LearnRow> = rows.iter().filter(|r| !r.context).collect();
    let n = rows.len();
    let picked: Vec<&LearnRow> = if n <= MAX_SOURCES {
        rows
    } else {
        (0..MAX_SOURCES)
            .map(|i| rows[i * (n - 1) / (MAX_SOURCES - 1)])
            .collect()
    };
    picked
        .into_iter()
        .map(|r| NoteSource {
            kind: SourceType::Message,
            id: r.id.to_string(),
            observed_at: row_time(&r.created_at),
        })
        .collect()
}

/// What the note is made of.
pub(crate) fn store_input(summary: &Summary, rows: &[LearnRow], today: &str) -> StoreInput {
    let observed = observed_at(rows);
    let date = observed
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| today.to_string());
    let title = title(&date, &summary.topic);
    StoreInput {
        body: body(&title, &knowledge(summary), &date, user_turns(rows)),
        title,
        kind: Some(NoteKind::Episode),
        sources: sources(rows),
        observed_at: observed,
        provenance: Some(ExtractionProvenance::Summary),
        ..Default::default()
    }
}

/// How the episode's write ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EpisodeWrite {
    /// A new summary.
    Written,
    /// A byte-identical one was there already.
    Existing,
    /// The service refused it — a cited row deleted since the window was
    /// read, say. Nothing was written; the cursor still moves.
    Refused,
    /// Not attempted: the window's project directory was deleted while the
    /// model wrote the summary ([`StoreEnd::RootGone`]).
    NotAttempted,
}

/// Everything the closure needs, owned so it can run on the blocking pool.
pub(crate) struct EpisodeJob {
    pub db: Database,
    pub service: Arc<MemoryService>,
    pub ctx: MemoryAccessContext,
    pub session_id: String,
    /// The window's project directory, checked again right before the write.
    pub root: Option<PathBuf>,
    pub input: StoreInput,
    pub through: i64,
    pub expected: i64,
    pub hooks: super::queue::StoreHooks,
}

/// The write and the episode cursor's advance, as **one** blocking task:
/// a cancellation after the model answered cannot separate them (design 09
/// §1.4's shape). A session deleted since the window was read gets nothing
/// (LQ4), and so does a project directory deleted while the model wrote:
/// the window is skipped as the check before the call skips it, and the
/// summary's write never re-creates the directory.
pub(crate) fn store_and_advance(job: EpisodeJob) -> (EpisodeWrite, StoreEnd) {
    match ConversationRepository::new(&job.db).get_session(&job.session_id) {
        Ok(Some(_)) => {}
        Ok(None) => return (EpisodeWrite::Refused, StoreEnd::SessionGone),
        Err(e) => {
            return (
                EpisodeWrite::Refused,
                StoreEnd::Held(format!("session lookup failed: {e:#}")),
            );
        }
    }
    if root_gone(job.root.as_deref()) {
        let end = advance_past(
            &job.db,
            &job.session_id,
            job.through,
            job.expected,
            StoreEnd::RootGone,
        );
        return (EpisodeWrite::NotAttempted, end);
    }
    let written = match job.service.store(&job.ctx, job.input) {
        Ok(out) if out.dedup == Dedup::Existing => EpisodeWrite::Existing,
        Ok(_) => EpisodeWrite::Written,
        Err(e @ (MemoryError::Internal(_) | MemoryError::Quarantined { .. })) => {
            return (EpisodeWrite::Refused, StoreEnd::Held(e.to_string()));
        }
        Err(e) => {
            tracing::warn!(
                session_id = %job.session_id,
                "conversation summary: the service refused the note: {e}"
            );
            EpisodeWrite::Refused
        }
    };
    (job.hooks.after_store_loop)();
    if job.hooks.skip_advance {
        return (written, StoreEnd::Held("injected fault".to_string()));
    }
    let end = advance_past(
        &job.db,
        &job.session_id,
        job.through,
        job.expected,
        StoreEnd::Advanced,
    );
    (written, end)
}

/// The episode cursor's advance: `done` when it moved; another writer's
/// move, or the session deleted meanwhile, otherwise.
fn advance_past(
    db: &Database,
    session_id: &str,
    through: i64,
    expected: i64,
    done: StoreEnd,
) -> StoreEnd {
    match LearningCursorRepository::new(db).advance_episode_cas(session_id, through, expected) {
        Ok(true) => done,
        Ok(false) => match ConversationRepository::new(db).get_session(session_id) {
            Ok(None) => StoreEnd::SessionGone,
            _ => StoreEnd::CursorMoved,
        },
        Err(e) => StoreEnd::Held(format!("episode cursor advance failed: {e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, kind: RowKind, at: &str) -> LearnRow {
        LearnRow {
            id,
            kind,
            context: false,
            text: format!("row {id}"),
            full_len: 5,
            created_at: at.to_string(),
            root: None,
            request_id: Some(format!("r{id}")),
        }
    }

    #[test]
    fn the_answer_needs_a_topic_and_what_was_discussed() {
        let s = parse_summary(
            r#"{"topic": "Lisbon  trip", "discussed": "Flights\nand hotels.", "decided": "none", "open": ""}"#,
        )
        .unwrap();
        assert_eq!(
            s,
            Summary {
                topic: "Lisbon trip".into(),
                discussed: "Flights and hotels.".into(),
                decided: String::new(),
                open: String::new(),
            }
        );
        for bad in [
            "not json",
            r#"["a list"]"#,
            r#"{"topic": "", "discussed": "x"}"#,
            r#"{"topic": "x", "discussed": "none"}"#,
            r#"{"topic": "x", "discussed": 3}"#,
        ] {
            assert!(parse_summary(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_knowledge_is_at_most_1_kib_and_never_splits_a_character() {
        let s = Summary {
            topic: "t".into(),
            discussed: "讨论".repeat(400),
            decided: "d".into(),
            open: String::new(),
        };
        let k = knowledge(&s);
        assert!(k.len() <= EPISODE_MAX_KNOWLEDGE_BYTES, "{}", k.len());
        assert!(k.ends_with('…'), "{k}");
        let short = Summary {
            topic: "t".into(),
            discussed: "a".into(),
            decided: "b".into(),
            open: "c".into(),
        };
        assert_eq!(knowledge(&short), "Discussed: a\nDecided: b\nLeft open: c");
    }

    #[test]
    fn the_title_names_the_date_and_a_short_topic() {
        assert_eq!(
            title("2026-09-20", "Lisbon trip"),
            "Conversation on 2026-09-20: Lisbon trip"
        );
        assert_eq!(
            title("2026-09-20", &"x".repeat(200)).chars().count(),
            "Conversation on 2026-09-20: ".len() + MAX_TOPIC_CHARS
        );
    }

    #[test]
    fn sources_are_every_row_up_to_eight_else_eight_spread_first_to_last() {
        let at = "2026-09-20 10:00:00";
        let few: Vec<LearnRow> = (1..=6).map(|i| row(i, RowKind::User, at)).collect();
        assert_eq!(
            sources(&few)
                .iter()
                .map(|s| s.id.clone())
                .collect::<Vec<_>>(),
            (1..=6).map(|i| i.to_string()).collect::<Vec<_>>()
        );
        let many: Vec<LearnRow> = (1..=20).map(|i| row(i, RowKind::Reply, at)).collect();
        let picked: Vec<i64> = sources(&many)
            .iter()
            .map(|s| s.id.parse().unwrap())
            .collect();
        assert_eq!(picked.len(), MAX_SOURCES);
        assert_eq!(picked.first(), Some(&1));
        assert_eq!(picked.last(), Some(&20));
        assert!(picked.windows(2).all(|w| w[0] < w[1]), "{picked:?}");
    }
}
