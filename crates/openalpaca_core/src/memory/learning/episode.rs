//! Conversation summaries (design 09 §13.3, owner decision MD-16, slice
//! L10c): one `episode` note per stretch of conversation — what was
//! discussed, what was decided, what was left open — findable by search, the
//! timeline, the list and the Memory view, and never injected on its own.
//!
//! The learning queue decides **when** (at least four of the user's turns
//! since the session's last summary, at a conversation's close: a quiet
//! period of 30 minutes, a switch of conversation, the stop drain and its
//! boot catch-up). This module is **what**: the note, **projected** from the
//! conversation's working state with no model call (design 11 §3.4, owner
//! decision CQ-2), and the one blocking task that writes it and moves the
//! session's episode cursor — the same at-least-once shape as learning's
//! store-and-advance (design 09 §1.4).
//!
//! The note:
//! - title `Conversation on <YYYY-MM-DD>: <topic>`, the date the last turn's;
//! - `## Knowledge`: what was discussed (the state's goal and what is done),
//!   decided (its decisions) and left open (what is open, and its next
//!   step), at most 1 KiB of prose;
//! - `observed_at` the last turn's time; `sources` the window's message rows
//!   (at most eight, spread from its first to its last — design 03 §7a's
//!   cap on `sources`); no `fact_key`, no aliases, no `supersedes`;
//! - origin `extracted_other`, status `active`
//!   ([`crate::memory::service::ExtractionProvenance::Summary`]).

use super::EPISODE_MAX_KNOWLEDGE_BYTES;
use super::extractor::{StoreEnd, root_gone};
use super::window::{LearnRow, RowKind};
use crate::memory::service::{
    ExtractionProvenance, MemoryAccessContext, MemoryError, MemoryService, StoreInput,
};
use crate::orchestrator::working_state::WorkingState;
use chrono::{DateTime, NaiveDateTime, Utc};
use openalpaca_storage::memory_files::{Dedup, MAX_SOURCES, NoteKind, NoteSource, SourceType};
use openalpaca_storage::{ConversationRepository, Database, LearningCursorRepository};
use std::path::PathBuf;
use std::sync::Arc;

/// A topic is cut to this many characters, so the title stays one short line.
const MAX_TOPIC_CHARS: usize = 60;

/// What the note says, one line each: the projection of a working state
/// ([`project`]). `decided` and `open` are empty when nothing was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Summary {
    pub topic: String,
    pub discussed: String,
    pub decided: String,
    pub open: String,
}

/// The working state as the note's lines (design 11 §3.4): the topic;
/// `Discussed:` its goal and what is done; `Decided:` its decisions; `Left
/// open:` what is open and `Next: <next>`. Items are joined with `; `. A
/// state with no goal and nothing done is discussed by its topic, so the
/// note never says it discussed nothing.
pub(crate) fn project(state: &WorkingState) -> Summary {
    let join = |items: Vec<&str>| {
        items
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("; ")
    };
    let mut discussed = join(
        std::iter::once(state.goal.as_str())
            .chain(state.done.iter().map(String::as_str))
            .collect(),
    );
    if discussed.is_empty() {
        discussed = state.topic.clone();
    }
    let next = (!state.next.is_empty()).then(|| format!("Next: {}", state.next));
    Summary {
        topic: state.topic.clone(),
        discussed,
        decided: join(state.decisions.iter().map(String::as_str).collect()),
        open: join(
            state
                .open
                .iter()
                .map(String::as_str)
                .chain(next.as_deref())
                .collect(),
        ),
    }
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

/// What the note is made of: the working state's projection over the
/// window's rows.
pub(crate) fn store_input(state: &WorkingState, rows: &[LearnRow], today: &str) -> StoreInput {
    let summary = project(state);
    let observed = observed_at(rows);
    let date = observed
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| today.to_string());
    let title = title(&date, &summary.topic);
    StoreInput {
        body: body(&title, &knowledge(&summary), &date, user_turns(rows)),
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
    /// Not attempted: the window's project directory was deleted since it
    /// was checked ([`StoreEnd::RootGone`]).
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

/// The write, the session's `episode_note_id` and the episode cursor's
/// advance, as **one** blocking task: a cancellation cannot separate them
/// (design 09 §1.4's shape). A session deleted since the window was read gets
/// nothing (LQ4), and so does a project directory deleted since the checks
/// before the write: the window is skipped as those checks skip it, and the
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
        Ok(out) => {
            // Design 11 §2: the session names the summary last projected from
            // its state (a byte-identical one there already included).
            if let Err(e) = ConversationRepository::new(&job.db)
                .set_episode_note_id(&job.session_id, &out.id.to_string())
            {
                tracing::warn!(
                    session_id = %job.session_id,
                    "conversation summary: its id was not recorded on the session: {e:#}"
                );
            }
            if out.dedup == Dedup::Existing {
                EpisodeWrite::Existing
            } else {
                EpisodeWrite::Written
            }
        }
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

    /// Design 11 §3.4: the note's lines come from the working state —
    /// `Discussed:` its goal and what is done, `Decided:` its decisions,
    /// `Left open:` what is open and its next step — with no model call.
    #[test]
    fn the_note_is_projected_from_the_working_state() {
        let state = WorkingState {
            v: 1,
            topic: "Lisbon trip".into(),
            goal: "Book flights and a hotel".into(),
            done: vec!["Friday flight booked".into(), "Budget set".into()],
            decisions: vec!["Alfama, not Baixa".into()],
            open: vec!["Which museum pass".into()],
            next: "Compare three hotels".into(),
            refs: vec!["task 1a2b3c4d".into()],
        };
        let summary = project(&state);
        assert_eq!(
            summary,
            Summary {
                topic: "Lisbon trip".into(),
                discussed: "Book flights and a hotel; Friday flight booked; Budget set".into(),
                decided: "Alfama, not Baixa".into(),
                open: "Which museum pass; Next: Compare three hotels".into(),
            }
        );
        assert_eq!(
            knowledge(&summary),
            "Discussed: Book flights and a hotel; Friday flight booked; Budget set\n\
             Decided: Alfama, not Baixa\n\
             Left open: Which museum pass; Next: Compare three hotels"
        );

        // Nothing decided, open or next: those lines go; no goal and nothing
        // done: the topic is what was discussed.
        let bare = WorkingState {
            goal: String::new(),
            done: Vec::new(),
            decisions: Vec::new(),
            open: Vec::new(),
            next: String::new(),
            ..state.clone()
        };
        assert_eq!(knowledge(&project(&bare)), "Discussed: Lisbon trip");
        let only_next = WorkingState {
            next: "Pick a date".into(),
            ..bare
        };
        assert_eq!(project(&only_next).open, "Next: Pick a date");
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
