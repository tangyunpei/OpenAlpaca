//! The learning window and its chunks (design 09 §1.2, §1.6).
//!
//! A **window** is a session's rows past its `learning_cursor`, in id order,
//! read through [`ConversationRepository::learning_window`] — which reads the
//! message `content` only, never `content_json` or `display_text`, so an
//! attachment's extracted text never reaches the extractor (U2). Each row is
//! classified by what wrote it; a **chunk** is the next run of rows that fits
//! the byte budget and shares one project root.
//!
//! [`ConversationRepository::learning_window`]: openalpaca_storage::ConversationRepository::learning_window

use super::{
    CONTEXT_ROW_BYTES, INTERNAL_SOURCE, LONG_USER_ROW_BYTES, REPLY_CUT_BYTES, is_slash_turn,
};
use openalpaca_storage::LearningRow;
use std::collections::HashSet;

/// What wrote a row the window keeps (design §1.2's table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowKind {
    /// A turn the user typed, at most 8 KiB.
    User,
    /// The same, over 8 KiB: a pipe or a paste may be quoting other text, so
    /// an item that cites it is never the user's own words (ruling LR21).
    UserLong,
    /// A follow-up's re-entered turn: the assistant's own `queue_followup`
    /// text, source `internal` (F7).
    Task,
    /// The answer to an included turn.
    Reply,
}

impl RowKind {
    /// The words the transcript labels the row with.
    fn label(self) -> &'static str {
        match self {
            RowKind::User => "user",
            RowKind::UserLong => "user (long; may quote other text)",
            RowKind::Task => "queued-task (written by the assistant)",
            RowKind::Reply => "assistant",
        }
    }

    /// Whether the row is the user's own text, for the chunk's minimum.
    pub(crate) fn is_users_text(self) -> bool {
        matches!(self, RowKind::User | RowKind::UserLong)
    }
}

/// One row the window keeps, already cut to its per-row bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LearnRow {
    pub id: i64,
    pub kind: RowKind,
    /// `true` for a row from before the cursor, shown for context only: it is
    /// never a source.
    pub context: bool,
    pub text: String,
    /// The row's own length before any cut, in bytes.
    pub full_len: usize,
    /// UTC, SQLite's `YYYY-MM-DD HH:MM:SS`.
    pub created_at: String,
    /// The project root this row learns into: a user row's own, or its
    /// question's for an answer (ruling LR1).
    pub root: Option<String>,
    pub request_id: Option<String>,
}

/// A row of the window, kept or left out. A left-out row still moves the
/// cursor past it.
#[derive(Debug, Clone)]
enum Entry {
    Keep(LearnRow),
    Skip(i64),
}

impl Entry {
    fn id(&self) -> i64 {
        match self {
            Entry::Keep(r) => r.id,
            Entry::Skip(id) => *id,
        }
    }
}

/// The next chunk to extract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chunk {
    /// Up to two rows from before the chunk: the last kept turn and its
    /// answer, each cut to 1 KiB.
    pub context: Vec<LearnRow>,
    /// The kept rows, in id order.
    pub rows: Vec<LearnRow>,
    /// The id the cursor advances to once the chunk is learned: its last row,
    /// kept or left out.
    pub through: i64,
    /// The chunk's one project root; `None` learns into the home partition
    /// only.
    pub root: Option<String>,
}

impl Chunk {
    /// Bytes of the user's own text in the chunk, for the minimum
    /// (`extract_min_content_len`, applied per chunk).
    pub(crate) fn users_text_bytes(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.kind.is_users_text())
            .map(|r| r.full_len)
            .sum()
    }

    /// The UTC date of the chunk's last row, `YYYY-MM-DD`: the prompt's
    /// `<today>` (design 09 §2.1).
    pub(crate) fn today(&self) -> String {
        self.rows
            .last()
            .and_then(|r| r.created_at.get(..10))
            .map(str::to_string)
            .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string())
    }

    /// The newest turn id of the chunk, for the extraction's attribution.
    pub(crate) fn last_request_id(&self) -> Option<&str> {
        self.rows.iter().rev().find_map(|r| r.request_id.as_deref())
    }

    /// The transcript the extractor reads, and the row each `R<n>` names:
    /// context rows first, then the chunk's own.
    pub(crate) fn transcript(&self) -> (String, Vec<&LearnRow>) {
        let numbered: Vec<&LearnRow> = self.context.iter().chain(self.rows.iter()).collect();
        let mut out = String::new();
        for (i, row) in numbered.iter().enumerate() {
            let label = if row.context {
                format!("context {}", row.kind.label())
            } else {
                row.kind.label().to_string()
            };
            out.push_str(&format!(
                "R{} {label} {}: {}\n",
                i + 1,
                iso_utc(&row.created_at),
                row.text
            ));
        }
        (out, numbered)
    }
}

/// What the next chunk of a window is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NextChunk {
    /// Nothing past the cursor: the session is learned.
    Empty,
    /// The first row past the cursor is a turn still in flight: its answer
    /// has not landed, so nothing can be learned yet.
    Blocked,
    Chunk(Chunk),
}

/// `YYYY-MM-DD HH:MM:SS` (SQLite's `datetime('now')`, UTC) as RFC 3339 `Z`.
fn iso_utc(created_at: &str) -> String {
    match created_at.split_once(' ') {
        Some((date, time)) => format!("{date}T{time}Z"),
        None => created_at.to_string(),
    }
}

/// `text` whole when it fits `head + tail` bytes, else its first `head` and
/// last `tail` bytes around a marked `[… N bytes omitted …]`. Cuts land on
/// character boundaries, so CJK and emoji are never split.
pub(crate) fn cut_middle(text: &str, head: usize, tail: usize) -> String {
    if text.len() <= head + tail {
        return text.to_string();
    }
    let head_end = text.floor_char_boundary(head);
    let tail_start = text.ceil_char_boundary(text.len() - tail);
    let omitted = tail_start - head_end;
    format!(
        "{} [… {omitted} bytes omitted …] {}",
        &text[..head_end],
        &text[tail_start..]
    )
}

/// The per-row cuts (design §1.6): a long user row keeps 6 KiB + 2 KiB, an
/// answer or an assistant-written task over 4 KiB keeps 3 KiB + 1 KiB, and a
/// context row keeps 1 KiB. Never 800/400 (MD-10).
fn cut_for(kind: RowKind, context: bool, text: &str) -> String {
    if context {
        return cut_middle(text, CONTEXT_ROW_BYTES, 0);
    }
    match kind {
        RowKind::User => text.to_string(),
        RowKind::UserLong => cut_middle(text, 6 * 1024, 2 * 1024),
        RowKind::Task | RowKind::Reply => {
            cut_middle(text, REPLY_CUT_BYTES * 3 / 4, REPLY_CUT_BYTES / 4)
        }
    }
}

/// Classifies rows (design §1.2). `in_flight` holds the request ids of turns
/// whose answer has not landed; `None` means the rows come from before the
/// cursor, where nothing is in flight any more.
///
/// Returns the entries and whether the classification stopped at an
/// in-flight user row.
fn classify(
    rows: &[LearningRow],
    in_flight: Option<&HashSet<String>>,
    context: bool,
) -> (Vec<Entry>, bool) {
    let mut entries = Vec::with_capacity(rows.len());
    // Turns whose user half was kept, so their answer is kept too, and the
    // root each learns under.
    let mut kept_turns: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    for row in rows {
        let request = row.request_id.as_deref();
        match row.role.as_str() {
            "user" => {
                let Some(request) = request else {
                    // No turn wrote it.
                    entries.push(Entry::Skip(row.id));
                    continue;
                };
                if in_flight.is_some_and(|set| set.contains(request)) {
                    // Its answer has not landed: the window ends before it.
                    return (entries, true);
                }
                if is_slash_turn(&row.content) {
                    entries.push(Entry::Skip(row.id));
                    continue;
                }
                let kind = if row.source.as_deref() == Some(INTERNAL_SOURCE) {
                    RowKind::Task
                } else if row.content.len() > LONG_USER_ROW_BYTES {
                    RowKind::UserLong
                } else {
                    RowKind::User
                };
                kept_turns.insert(request.to_string(), row.workspace_root.clone());
                entries.push(Entry::Keep(LearnRow {
                    id: row.id,
                    kind,
                    context,
                    text: cut_for(kind, context, &row.content),
                    full_len: row.content.len(),
                    created_at: row.created_at.clone(),
                    root: row.workspace_root.clone(),
                    request_id: Some(request.to_string()),
                }));
            }
            "assistant" => match request.and_then(|r| kept_turns.get(r).map(|root| (r, root))) {
                Some((request, root)) => entries.push(Entry::Keep(LearnRow {
                    id: row.id,
                    kind: RowKind::Reply,
                    context,
                    text: cut_for(RowKind::Reply, context, &row.content),
                    full_len: row.content.len(),
                    created_at: row.created_at.clone(),
                    root: root.clone(),
                    request_id: Some(request.to_string()),
                })),
                // No turn id (a notice, a `post_update`, a completion report),
                // or the answer to a left-out `/slash` turn.
                None => entries.push(Entry::Skip(row.id)),
            },
            _ => entries.push(Entry::Skip(row.id)),
        }
    }
    (entries, false)
}

/// The context rows for a chunk: the last kept turn before it and its
/// answer, from `before` (rows at or before the cursor, in id order).
pub(crate) fn context_rows(before: &[LearningRow]) -> Vec<LearnRow> {
    let (entries, _) = classify(before, None, true);
    let kept: Vec<LearnRow> = entries
        .into_iter()
        .filter_map(|e| match e {
            Entry::Keep(r) => Some(r),
            Entry::Skip(_) => None,
        })
        .collect();
    let Some(last_turn) = kept.iter().rposition(|r| r.kind != RowKind::Reply) else {
        return Vec::new();
    };
    let request = kept[last_turn].request_id.clone();
    kept[last_turn..]
        .iter()
        .filter(|r| r.request_id == request)
        .take(2)
        .cloned()
        .collect()
}

/// Bytes a row costs in the transcript: its text plus its label line.
fn row_cost(row: &LearnRow) -> usize {
    row.text.len() + 48
}

/// One turn and its answer, plus the left-out rows that follow them.
struct Group {
    entries: Vec<Entry>,
    root: Option<String>,
    bytes: usize,
    /// Whether the group holds a kept row at all.
    kept: bool,
}

fn group(entries: Vec<Entry>) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for entry in entries {
        let starts_turn = matches!(&entry, Entry::Keep(r) if r.kind != RowKind::Reply);
        if starts_turn || groups.is_empty() {
            let root = match &entry {
                Entry::Keep(r) => r.root.clone(),
                Entry::Skip(_) => None,
            };
            groups.push(Group {
                entries: Vec::new(),
                root,
                bytes: 0,
                kept: false,
            });
        }
        let g = groups.last_mut().expect("a group was just pushed");
        if let Entry::Keep(r) = &entry {
            if !g.kept {
                g.root = r.root.clone();
            }
            g.kept = true;
            g.bytes += row_cost(r);
        }
        g.entries.push(entry);
    }
    groups
}

/// Re-cuts a group's kept rows so the whole group fits `budget`: a single
/// turn always fits a chunk, however small the model's window (design §1.6).
fn fit_group(group: &mut Group, budget: usize) {
    if group.bytes <= budget {
        return;
    }
    let rows = group
        .entries
        .iter()
        .filter(|e| matches!(e, Entry::Keep(_)))
        .count()
        .max(1);
    // Room for each row's label line and its omission marker.
    let per_row = (budget / rows).saturating_sub(128).max(256);
    let mut bytes = 0;
    for entry in &mut group.entries {
        if let Entry::Keep(r) = entry {
            if r.text.len() > per_row {
                r.text = cut_middle(&r.text, per_row * 3 / 4, per_row / 4);
            }
            bytes += row_cost(r);
        }
    }
    group.bytes = bytes;
}

/// The next chunk of a window (design §1.6).
///
/// `rows` are the session's rows past the cursor, in id order, at most
/// `read_limit` of them; `before` the rows at or before the cursor, for the
/// context; `in_flight` the request ids of turns whose answer has not landed.
/// A chunk never mixes roots, never splits a turn from its answer, and fits
/// `transcript_bytes`.
pub(crate) fn next_chunk(
    rows: &[LearningRow],
    before: &[LearningRow],
    in_flight: &HashSet<String>,
    transcript_bytes: usize,
    read_limit: usize,
) -> NextChunk {
    if rows.is_empty() {
        return NextChunk::Empty;
    }
    let (entries, stopped) = classify(rows, Some(in_flight), false);
    if entries.is_empty() {
        return if stopped {
            NextChunk::Blocked
        } else {
            NextChunk::Empty
        };
    }
    let mut groups = group(entries);
    // A read that hit its limit may have cut the last turn from its answer;
    // leave that turn for the next read, unless it is the only one.
    if !stopped && rows.len() >= read_limit && groups.len() > 1 {
        groups.pop();
    }

    let mut chunk_entries: Vec<Entry> = Vec::new();
    let mut bytes = 0usize;
    let mut root: Option<Option<String>> = None;
    for mut g in groups {
        if g.kept {
            match &root {
                None => {
                    fit_group(&mut g, transcript_bytes);
                    root = Some(g.root.clone());
                }
                Some(r) if *r != g.root => break,
                Some(_) if bytes + g.bytes > transcript_bytes => break,
                Some(_) => {}
            }
            bytes += g.bytes;
        }
        chunk_entries.extend(g.entries);
    }
    let through = chunk_entries
        .last()
        .map(Entry::id)
        .expect("a non-empty window yields at least one entry");
    let rows: Vec<LearnRow> = chunk_entries
        .into_iter()
        .filter_map(|e| match e {
            Entry::Keep(r) => Some(r),
            Entry::Skip(_) => None,
        })
        .collect();
    NextChunk::Chunk(Chunk {
        context: context_rows(before),
        rows,
        through,
        root: root.flatten(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, role: &str, content: &str, request: Option<&str>) -> LearningRow {
        LearningRow {
            id,
            role: role.to_string(),
            content: content.to_string(),
            source: Some("gui".to_string()),
            created_at: "2026-09-24 10:00:00".to_string(),
            request_id: request.map(str::to_string),
            workspace_root: None,
        }
    }

    fn chunk(rows: &[LearningRow]) -> Chunk {
        match next_chunk(rows, &[], &HashSet::new(), 24 * 1024, 400) {
            NextChunk::Chunk(c) => c,
            other => panic!("expected a chunk, got {other:?}"),
        }
    }

    #[test]
    fn cut_middle_keeps_head_and_tail_on_character_boundaries() {
        assert_eq!(cut_middle("short", 3, 3), "short");
        let text = "日本語".repeat(10);
        let cut = cut_middle(&text, 10, 5);
        assert!(cut.starts_with("日本語"), "{cut}");
        assert!(cut.contains("bytes omitted"), "{cut}");
        assert!(cut.ends_with('語'), "{cut}");
    }

    #[test]
    fn an_answer_is_kept_only_with_its_question() {
        let c = chunk(&[
            row(1, "assistant", "orphan", Some("gone")),
            row(2, "user", "question", Some("a")),
            row(3, "assistant", "answer", Some("a")),
        ]);
        assert_eq!(c.rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2, 3]);
        assert_eq!(c.through, 3);
    }

    #[test]
    fn the_budget_splits_at_turn_boundaries() {
        let long = "x".repeat(3000);
        let rows = [
            row(1, "user", &long, Some("a")),
            row(2, "assistant", "ok", Some("a")),
            row(3, "user", &long, Some("b")),
            row(4, "assistant", "ok", Some("b")),
        ];
        let NextChunk::Chunk(c) = next_chunk(&rows, &[], &HashSet::new(), 4000, 400) else {
            panic!("a chunk");
        };
        assert_eq!(c.rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(c.through, 2);
    }

    #[test]
    fn one_turn_always_fits_even_a_small_budget() {
        let rows = [
            row(1, "user", &"y".repeat(8000), Some("a")),
            row(2, "assistant", &"z".repeat(4000), Some("a")),
        ];
        let NextChunk::Chunk(c) = next_chunk(&rows, &[], &HashSet::new(), 3000, 400) else {
            panic!("a chunk");
        };
        assert_eq!(c.rows.len(), 2);
        let total: usize = c.rows.iter().map(row_cost).sum();
        assert!(total <= 3000, "{total}");
    }

    #[test]
    fn context_is_the_last_kept_turn_and_its_answer() {
        let before = [
            row(1, "user", "old question", Some("a")),
            row(2, "assistant", "old answer", Some("a")),
            row(3, "user", "last question", Some("b")),
            row(4, "assistant", "last answer", Some("b")),
            row(5, "assistant", "a notice", None),
        ];
        let context = context_rows(&before);
        assert_eq!(context.iter().map(|r| r.id).collect::<Vec<_>>(), vec![3, 4]);
        assert!(context.iter().all(|r| r.context));
    }

    #[test]
    fn a_read_at_its_limit_leaves_a_split_turn_for_the_next_read() {
        let rows = [
            row(1, "user", "q1", Some("a")),
            row(2, "assistant", "a1", Some("a")),
            row(3, "user", "q2", Some("b")),
        ];
        let NextChunk::Chunk(c) = next_chunk(&rows, &[], &HashSet::new(), 24 * 1024, 3) else {
            panic!("a chunk");
        };
        assert_eq!(c.through, 2);
    }
}
