//! One chunk's extraction (design 09 §2).
//!
//! The model is shown the chunk's transcript, the notes the owner already
//! has that the chunk's rows bring up (`known_notes`, referred to as
//! `n1`…`n20`) and the keys in use (`known_keys`), and asked what durable
//! knowledge the chunk carries about the user. The prompt's text is the
//! committed fixture `extractor_prompt.txt` (§2.1). Every item it names is
//! validated deterministically before anything is written (§2.4):
//!
//! 1. the top level is `{"items": [...]}`, else the whole answer failed;
//! 2. `content` is present and at most 1,000 characters;
//! 3. `kind` is one of the six when present (missing = `fact`);
//! 4. `rows` names 1–8 rows of the chunk, at least one not a context row —
//!    context rows are removed and never become sources;
//! 5. `stated_by: "user"` stands only when **every** remaining cited row is
//!    the user's own typed row; anything else is inferred (a candidate);
//! 6. an item under `memory_confidence_threshold` is dropped;
//! 7. aliases are trimmed to 1–60 characters, deduplicated without regard
//!    to case, the title removed, at most 8;
//! 8. `fact_key` is lowercased and must match the grammar, else it is
//!    dropped and the item kept;
//! 9. `replaces` must name a listed note, else it is dropped (counted);
//! 10. a validity bound is a date or RFC 3339 — a date-only `valid_to` is
//!     the next day's midnight, so the stated day is inside the half-open
//!     window — and an unparsable, inverted or empty pair drops both;
//! 11. one item per key and per target: the one whose newest cited row is
//!     latest stays (a tie keeps the model's first) — once on the model's
//!     keys and refs, again once each item's target is resolved;
//! 12. a cited row a forget suppressed is removed, and an item left citing
//!     nothing is dropped (§1.4(d));
//! 13. an item this chunk already taught — a note of this window with the
//!     item's identity and knowledge, or a candidate naming the item's
//!     target — is dropped (§1.4(b));
//! 14. at most `extract_max_items_per_call` items survive, in model order.
//!
//! §2.5 adds `unchanged`: an item that restates its target.

use super::window::{LearnRow, RowKind};
use crate::memory::service::{
    ExtractionProvenance, KnownNote, MemoryAccessContext, MemoryError, MemoryService, StoreInput,
};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use openalpaca_llm::{ChatMessage, RequestContext, RouterRequest};
use openalpaca_storage::memory_files::{
    Dedup, NoteKind, NoteScope, NoteSource, Origin, ReplacePolicy, SourceType, is_valid_fact_key,
    normalize_statement,
};
use openalpaca_storage::{ConversationRepository, Database, LearningCursorRepository};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The usage label chat learning's calls are billed and gated under.
pub(crate) const AGENT_LABEL: &str = "orchestrator_user_extract";

/// The most characters an item's `content` may have (§2.2).
const MAX_CONTENT_CHARS: usize = 1000;
/// The most rows an item may cite (§2.2).
const MAX_CITED_ROWS: usize = 8;
/// An item's title is cut to this many characters (§2.2).
const MAX_TITLE_CHARS: usize = 80;
/// The most aliases an item keeps (§2.2).
pub(crate) const MAX_ALIASES: usize = 8;
/// Each alias is cut to this many characters (§2.2).
const MAX_ALIAS_CHARS: usize = 60;
/// `replace_reason` is cut to this many characters (§2.2).
const MAX_REASON_CHARS: usize = 200;
/// The reason a replacement states when the model gave none (§2.2).
pub(crate) const DEFAULT_REPLACE_REASON: &str = "Restated in conversation.";

/// The prompt, committed verbatim (design 09 §2.1): the system text, a
/// `=== user ===` line, then the user message's template. Two implementers
/// cannot diverge from a committed file; `the_extractor_prompt_matches_its_fixture`
/// renders one fixed chunk through it and compares byte for byte.
const PROMPT: &str = include_str!("extractor_prompt.txt");
const USER_MARKER: &str = "=== user ===\n";

/// What one chunk's prompt is made of.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PromptInput<'a> {
    /// The UTC date of the chunk's last row, `YYYY-MM-DD`.
    pub today: &'a str,
    /// The `<known_notes>` body: empty, or a newline and one line per note.
    pub known_notes: &'a str,
    /// The `<known_keys>` body.
    pub known_keys: &'a str,
    /// The numbered transcript, one line per row.
    pub conversation: &'a str,
    /// Rule 9's "at most {items} items".
    pub max_items: usize,
}

/// The system text and the rendered user message. Placeholders are filled
/// in one pass, so a transcript that happens to contain `{{items}}` is
/// quoted, never expanded.
pub(crate) fn render_prompt(input: &PromptInput<'_>) -> (String, String) {
    let (system, template) = PROMPT
        .split_once(USER_MARKER)
        .expect("extractor_prompt.txt has a `=== user ===` line");
    let items = input.max_items.to_string();
    let user = fill_template(template, |name| match name {
        "today" => Some(input.today),
        "known_notes" => Some(input.known_notes),
        "known_keys" => Some(input.known_keys),
        "conversation" => Some(input.conversation),
        "items" => Some(items.as_str()),
        _ => None,
    });
    (system.trim_end().to_string(), user.trim_end().to_string())
}

/// Fills a prompt fixture's `{{name}}` placeholders in one pass, so a value
/// that happens to contain `{{items}}` is quoted, never expanded; a name
/// `value` does not know is left as it is. Shared with the episode writer's
/// prompt (design 09 §13.3).
pub(crate) fn fill_template<'v>(template: &str, value: impl Fn(&str) -> Option<&'v str>) -> String {
    let mut out = String::with_capacity(template.len() * 2);
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        match value(&after[..end]) {
            Some(v) => out.push_str(v),
            None => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// The chunk's request: the fixture's prompt, `thinking: Disabled` (M2),
/// temperature 0, no caching and no fallbacks.
pub(crate) fn build_request(
    input: &PromptInput<'_>,
    out_tokens: u32,
    model: Option<String>,
) -> RouterRequest {
    let (system, user) = render_prompt(input);
    RouterRequest {
        model,
        messages: Arc::new(vec![ChatMessage::system(&system), ChatMessage::user(&user)]),
        tools: Arc::new(vec![]),
        temperature: Some(0.0),
        max_tokens: Some(out_tokens),
        context: RequestContext {
            agent_id: Some(AGENT_LABEL.to_string()),
            task_id: None,
        },
        tool_choice: None,
        tools_token_estimate: None,
        enable_caching: false,
        // M2: an internal utility call wants no reasoning — on a local
        // thinking model the budget would otherwise go into it.
        thinking: Some(openalpaca_llm::ThinkingConfig::Disabled),
        context_management: None,
        fallback_models: Vec::new(),
        ephemeral_system_notice: None,
    }
}

/// Rule 1: the top level must be `{"items": [...]}`. `Err` names what the
/// model sent instead — a failed attempt (§1.4), never a partial read.
pub(crate) fn parse_items(content: &str) -> Result<Vec<serde_json::Value>, String> {
    let Some(value) = crate::memory::task_extraction::parse_json_response(content) else {
        return Err("the answer is not JSON".to_string());
    };
    match value.get("items").and_then(|v| v.as_array()) {
        Some(items) if value.is_object() => Ok(items.clone()),
        _ => Err("the answer's top level is not {\"items\": [...]}".to_string()),
    }
}

/// An item that passed rules 2–10.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ValidItem {
    pub title: String,
    pub content: String,
    pub kind: NoteKind,
    /// Rule 5: every cited row is the user's own typed row.
    pub user_words: bool,
    /// The cited rows, context rows removed, in transcript order.
    pub rows: Vec<CitedRow>,
    pub aliases: Vec<String>,
    pub fact_key: Option<String>,
    /// `replaces`, as an index into the listed notes (`n<k>` is `k - 1`).
    pub replaces: Option<usize>,
    pub replace_reason: Option<String>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    /// Its place in the model's answer.
    pub order: usize,
}

impl ValidItem {
    /// The newest cited row — rule 11's tie-break.
    fn newest_row(&self) -> i64 {
        self.rows.iter().map(|r| r.id).max().unwrap_or(0)
    }

    /// When the newest cited row was written, UTC to the second: the note's
    /// `observed_at` (§2.5).
    pub(crate) fn observed_at(&self) -> Option<DateTime<Utc>> {
        self.rows.iter().filter_map(CitedRow::observed_at).max()
    }
}

/// One row an item cites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CitedRow {
    pub id: i64,
    /// UTC, SQLite's `YYYY-MM-DD HH:MM:SS`.
    pub created_at: String,
}

impl CitedRow {
    fn observed_at(&self) -> Option<DateTime<Utc>> {
        NaiveDateTime::parse_from_str(&self.created_at, "%Y-%m-%d %H:%M:%S")
            .ok()
            .map(|t| t.and_utc())
    }

    fn source(&self) -> NoteSource {
        NoteSource {
            kind: SourceType::Message,
            id: self.id.to_string(),
            observed_at: self.observed_at(),
        }
    }

    fn suppression_key(&self) -> String {
        format!("message:{}", self.id)
    }
}

/// What validation kept, and what it dropped, by rule.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Validated {
    pub items: Vec<ValidItem>,
    /// Rules 2–4.
    pub invalid: usize,
    /// Rule 6.
    pub below_threshold: usize,
    /// Rule 9: a `replaces` naming no listed note; the item is kept.
    pub replaces_unknown: usize,
    /// Rule 11, first pass.
    pub duplicate_key: usize,
}

/// Rules 2–11 over the model's items. `numbered[i]` is the row `R{i+1}`;
/// `listed` is how many notes the prompt listed (`n1`…`n{listed}`).
pub(crate) fn validate(
    items: &[serde_json::Value],
    numbered: &[&LearnRow],
    threshold: f64,
    listed: usize,
) -> Validated {
    let mut out = Validated::default();
    for (order, value) in items.iter().enumerate() {
        let Some(read) = read_item(value, numbered, listed, order) else {
            out.invalid += 1;
            continue;
        };
        if read.confidence < threshold {
            out.below_threshold += 1;
            continue;
        }
        if read.replaces_unknown {
            out.replaces_unknown += 1;
        }
        out.items.push(read.item);
    }
    let (kept, dropped) = keep_latest(std::mem::take(&mut out.items), |i| i.fact_key.clone());
    let (kept, dropped_refs) = keep_latest(kept, |i| i.replaces.map(|k| k.to_string()));
    out.items = kept;
    out.duplicate_key = dropped + dropped_refs;
    out
}

/// Rule 11: among items sharing a `key`, the one whose newest cited row is
/// latest stays; a tie keeps the one the model listed first. Items with no
/// key all stay. Order is kept. Returns what stayed and how many went.
fn keep_latest<K: Eq + std::hash::Hash>(
    items: Vec<ValidItem>,
    key: impl Fn(&ValidItem) -> Option<K>,
) -> (Vec<ValidItem>, usize) {
    let mut winner: HashMap<K, (i64, usize)> = HashMap::new();
    for item in &items {
        if let Some(k) = key(item) {
            let candidate = (item.newest_row(), item.order);
            winner
                .entry(k)
                .and_modify(|best| {
                    if candidate.0 > best.0 || (candidate.0 == best.0 && candidate.1 < best.1) {
                        *best = candidate;
                    }
                })
                .or_insert(candidate);
        }
    }
    let before = items.len();
    let kept: Vec<ValidItem> = items
        .into_iter()
        .filter(|item| match key(item) {
            Some(k) => winner
                .get(&k)
                .is_some_and(|(_, order)| *order == item.order),
            None => true,
        })
        .collect();
    let dropped = before - kept.len();
    (kept, dropped)
}

/// One item, read.
struct ReadItem {
    item: ValidItem,
    confidence: f64,
    replaces_unknown: bool,
}

/// Rules 2–5 and 7–10 for one item. `None` is invalid (rules 2–4).
fn read_item(
    value: &serde_json::Value,
    numbered: &[&LearnRow],
    listed: usize,
    order: usize,
) -> Option<ReadItem> {
    // Rule 2.
    let content = value.get("content")?.as_str()?.trim();
    if content.is_empty() || content.chars().count() > MAX_CONTENT_CHARS {
        return None;
    }
    // Rule 3.
    let kind = match value.get("kind").and_then(|v| v.as_str()).map(str::trim) {
        None | Some("") => NoteKind::Fact,
        Some(k) => NoteKind::parse(k)?,
    };
    // Rule 4.
    let cited = value.get("rows")?.as_array()?;
    if cited.is_empty() || cited.len() > MAX_CITED_ROWS {
        return None;
    }
    let mut seen: HashSet<usize> = HashSet::new();
    let mut rows: Vec<&LearnRow> = Vec::new();
    for entry in cited {
        let Some(n) = row_number(entry) else {
            continue;
        };
        let Some(row) = n.checked_sub(1).and_then(|i| numbered.get(i)) else {
            continue;
        };
        if row.context || !seen.insert(n) {
            continue;
        }
        rows.push(row);
    }
    if rows.is_empty() {
        return None;
    }
    rows.sort_by_key(|r| r.id);
    // Rule 5: checked, not trusted.
    let says_user = value
        .get("stated_by")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s.trim().eq_ignore_ascii_case("user"));
    let user_words = says_user && rows.iter().all(|r| r.kind == RowKind::User);

    let confidence = value
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .map(one_line)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| one_line(content));
    let title: String = title.chars().take(MAX_TITLE_CHARS).collect();
    let title = title.trim().to_string();
    // Rule 7.
    let aliases = clean_aliases(value.get("aliases"), &title);
    // Rule 8.
    let fact_key = clean_fact_key(value.get("fact_key"));
    // Rule 9.
    let (replaces, replaces_unknown) = match value.get("replaces") {
        None | Some(serde_json::Value::Null) => (None, false),
        Some(v) => match note_ref(v).filter(|k| (1..=listed).contains(k)) {
            Some(k) => (Some(k - 1), false),
            None => (None, true),
        },
    };
    let replace_reason = value
        .get("replace_reason")
        .and_then(|v| v.as_str())
        .map(one_line)
        .filter(|r| !r.is_empty())
        .map(|r| r.chars().take(MAX_REASON_CHARS).collect::<String>());
    // Rule 10.
    let (valid_from, valid_to) = validity(value.get("valid_from"), value.get("valid_to"));

    Some(ReadItem {
        item: ValidItem {
            title,
            content: content.to_string(),
            kind,
            user_words,
            rows: rows
                .into_iter()
                .map(|r| CitedRow {
                    id: r.id,
                    created_at: r.created_at.clone(),
                })
                .collect(),
            aliases,
            fact_key,
            replaces,
            replace_reason,
            valid_from,
            valid_to,
            order,
        },
        confidence,
        replaces_unknown,
    })
}

/// Whitespace runs collapsed to one space, the ends trimmed: a title or a
/// reason is one line.
pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A row reference: `3` or `"R3"` (the transcript labels rows `R3`).
fn row_number(value: &serde_json::Value) -> Option<usize> {
    if let Some(n) = value.as_u64() {
        return usize::try_from(n).ok();
    }
    let s = value.as_str()?.trim();
    let digits = s
        .strip_prefix('R')
        .or_else(|| s.strip_prefix('r'))
        .unwrap_or(s);
    digits.parse().ok()
}

/// A note reference: `"n3"` — the number, 1-based.
fn note_ref(value: &serde_json::Value) -> Option<usize> {
    let s = value.as_str()?.trim();
    let digits = s.strip_prefix('n').or_else(|| s.strip_prefix('N'))?;
    digits.parse().ok()
}

/// Rule 7 (§2.2): each alias trimmed and cut to 60 characters, the empty
/// ones and the title dropped, duplicates dropped without regard to case,
/// at most 8. Shared with task-output extraction (§2.7).
pub(crate) fn clean_aliases(value: Option<&serde_json::Value>, title: &str) -> Vec<String> {
    let Some(list) = value.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let title_key = title.trim().to_lowercase();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for entry in list {
        let Some(text) = entry.as_str() else {
            continue;
        };
        let alias: String = one_line(text).chars().take(MAX_ALIAS_CHARS).collect();
        let alias = alias.trim().to_string();
        let folded = alias.to_lowercase();
        if alias.is_empty() || folded == title_key || !seen.insert(folded) {
            continue;
        }
        out.push(alias);
        if out.len() == MAX_ALIASES {
            break;
        }
    }
    out
}

/// Rule 8 (§2.2): the key lowercased and trimmed; one that does not match
/// `[a-z0-9][a-z0-9._-]{0,63}` (design 03 §4) is dropped, and the item is
/// kept. Shared with task-output extraction (§2.7).
pub(crate) fn clean_fact_key(value: Option<&serde_json::Value>) -> Option<String> {
    let key = value?.as_str()?.trim().to_lowercase();
    is_valid_fact_key(&key).then_some(key)
}

/// Rule 10 (§2.2): each bound a `YYYY-MM-DD` date or an RFC 3339 instant.
/// A date-only `valid_from` is that day's midnight UTC; a date-only
/// `valid_to` is the **next** day's, so the stated last day is inside the
/// half-open window (design 03 §4). An unparsable bound, or a pair whose
/// end is not after its start, drops both.
fn validity(
    from: Option<&serde_json::Value>,
    to: Option<&serde_json::Value>,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    match (bound(from, false), bound(to, true)) {
        (Ok(Some(f)), Ok(Some(t))) if t <= f => (None, None),
        (Ok(f), Ok(t)) => (f, t),
        _ => (None, None),
    }
}

fn bound(value: Option<&serde_json::Value>, end: bool) -> Result<Option<DateTime<Utc>>, ()> {
    let text = match value {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(v) => v.as_str().ok_or(())?.trim(),
    };
    if let Ok(day) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        let day = if end { day.succ_opt().ok_or(())? } else { day };
        return Ok(Some(day.and_hms_opt(0, 0, 0).ok_or(())?.and_utc()));
    }
    DateTime::parse_from_rfc3339(text)
        .map(|t| Some(t.with_timezone(&Utc)))
        .map_err(|_| ())
}

/// An item's target (§2.5): the note it would replace.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Target {
    pub note: KnownNote,
    /// Found by the item's `replaces` ref — the model compared the two.
    pub by_ref: bool,
    /// The prompt listed it: the model saw it.
    pub listed: bool,
}

/// What an item becomes (§2.5's change test and its table).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Plan {
    /// No target: a new note, active from the user's words, a candidate
    /// otherwise.
    New,
    /// It restates its target: dropped (`unchanged`).
    Unchanged,
    /// The user's words change a **learned** note: written active with
    /// `supersedes` — an automatic replacement, undoable (MD-11).
    Automatic(KnownNote),
    /// The user's words change a note the user saved, edited by hand or
    /// imported: a suggestion the user confirms (MD-11, G2, ruling LR9) — a
    /// candidate of origin `extracted_user` naming the note, which the file
    /// layer writes under `SuggestUnlessLearned` (S1, S2).
    Suggestion(KnownNote),
    /// Assistant or tool output naming a target: an inferred suggestion, a
    /// candidate that supersedes nothing until confirmed in the Memory view
    /// or the CLI (MD-2, design 09 §4.2 rule 5).
    Inferred(KnownNote),
}

/// §2.5, "Is it a change?": a `replaces` ref is a change; a target found by
/// key alone that the prompt listed is unchanged when it was saved (the
/// model saw it and did not call it replaced) and, when it was learned,
/// unless the knowledge differs after normalisation; a target the model was
/// never shown is a change — a real change in new words is exactly what the
/// lexical lookup misses, and a suggestion shows both texts.
pub(crate) fn plan(item: &ValidItem, target: Option<&Target>) -> Plan {
    let Some(target) = target else {
        return Plan::New;
    };
    let t = &target.note;
    let change = if target.by_ref || !target.listed {
        true
    } else if t.origin == Origin::ExtractedUser {
        normalize_statement(&item.content) != normalize_statement(&t.knowledge)
    } else {
        false
    };
    if !change {
        Plan::Unchanged
    } else if !item.user_words {
        Plan::Inferred(t.clone())
    } else if t.origin == Origin::ExtractedUser {
        Plan::Automatic(t.clone())
    } else {
        Plan::Suggestion(t.clone())
    }
}

/// Rule 13 (§1.4(b)): a note of this window already holds the item — a
/// candidate naming the target the item would, or a note with the item's
/// identity (equal `fact_key` when both carry one, else equal kind and
/// case-folded title) whose knowledge equals the item's content after
/// normalisation. A same-key item whose value differs is not held: the
/// change test lets it correct the earlier copy (guard (c)).
pub(crate) fn already_learned(
    item: &ValidItem,
    target: Option<&Target>,
    window: &[KnownNote],
) -> bool {
    let content = normalize_statement(&item.content);
    window.iter().any(|w| {
        if w.effective_status == openalpaca_storage::memory_files::EffectiveStatus::Candidate
            && target.is_some_and(|t| w.supersedes.contains(&t.note.id))
        {
            return true;
        }
        let same_identity = match (&w.fact_key, &item.fact_key) {
            (Some(a), Some(b)) => a == b,
            _ => w.kind == item.kind && w.title.trim().to_lowercase() == item.title.to_lowercase(),
        };
        same_identity && normalize_statement(&w.knowledge) == content
    })
}

/// Whether a key's subject is the user (`user.*`, ruling LR17).
fn is_user_key(key: Option<&str>) -> bool {
    key.and_then(|k| k.split('.').next()) == Some("user")
}

/// How a replacement's evidence names its target's origin.
fn target_label(origin: Origin) -> &'static str {
    match origin {
        Origin::User => "saved by you",
        Origin::ExtractedUser => "learned",
        Origin::External => "edited by hand",
        Origin::Imported => "imported",
        Origin::ExtractedOther => "inferred",
    }
}

/// The note's body (§2.6): design 03 §4's sections. `## Knowledge` holds
/// exactly the item's content — what rule 13 and the change test compare —
/// and `## Evidence and limits` says whose words it was and when, and what
/// it replaces and why.
pub(crate) fn note_body(item: &ValidItem, target: Option<&KnownNote>, date: &str) -> String {
    let mut evidence = if item.user_words {
        format!("From what you said on {date}.")
    } else {
        format!("Inferred from the conversation on {date}; not said by you.")
    };
    if let Some(t) = target {
        let reason = item
            .replace_reason
            .as_deref()
            .unwrap_or(DEFAULT_REPLACE_REASON);
        evidence.push_str(&format!(
            " Replaces “{}” ({}): {reason}",
            t.title,
            target_label(t.origin)
        ));
    }
    format!(
        "# {}\n\n## Knowledge\n{}\n\n## Evidence and limits\n{evidence}\n",
        item.title, item.content
    )
}

/// What one chunk's store loop wrote and dropped, counted without note
/// bodies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StoreCounts {
    pub stored: usize,
    pub active: usize,
    pub candidates: usize,
    /// Automatic replacements of a learned note, among `active`.
    pub replaced: usize,
    pub deduplicated: usize,
    /// Rule 14.
    pub dropped_over_cap: usize,
    /// Rule 12.
    pub dropped_suppressed: usize,
    /// Rule 11, second pass: two items reaching one target.
    pub dropped_duplicate_key: usize,
    /// Rule 13.
    pub dropped_already_learned: usize,
    /// §2.5: restatements of their target.
    pub dropped_unchanged: usize,
    /// §2.5, *The race*: the target moved between the lookup and the write.
    pub dropped_conflict: usize,
    /// Among `candidates`: suggestions (MD-11) — a change in the user's
    /// words to a note they saved, edited or imported, and an inferred
    /// change to any note — each naming the note it would replace.
    pub suggestions: usize,
    pub failed: usize,
}

/// How the store-and-advance closure ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoreEnd {
    /// The cursor moved past the chunk.
    Advanced,
    /// Another writer moved the cursor first; this chunk's advance did
    /// nothing (the compare-and-swap refused it).
    CursorMoved,
    /// The session was deleted (LQ4): nothing was written.
    SessionGone,
    /// A store failed on I/O or a quarantine: the cursor holds. The notes
    /// already written answer `dedup` or `already_learned` on the retry.
    Held(String),
}

/// Everything the closure needs, owned so it can run on the blocking pool.
pub(crate) struct StoreJob {
    pub db: Database,
    pub service: Arc<MemoryService>,
    pub ctx: MemoryAccessContext,
    pub session_id: String,
    pub items: Vec<ValidItem>,
    /// The notes the prompt listed: `replaces: n<k>` names `listed[k - 1]`.
    pub listed: Vec<KnownNote>,
    /// This window's learned notes (rule 13).
    pub from_this_window: Vec<KnownNote>,
    /// The chunk's date, for a note whose rows carry no time.
    pub today: String,
    pub max_items: usize,
    pub through: i64,
    pub expected: i64,
    pub hooks: super::queue::StoreHooks,
}

/// Steps 3 and 4 of §1.4, as **one** blocking task: the chunk's store loop
/// and its cursor advance. A cancellation or a shutdown that lands after the
/// model answered can drop the async half that awaits this, but not this —
/// only a process exit mid-closure can leave the cursor behind the writes,
/// and the four guards of §1.4 make that repeat harmless.
pub(crate) fn store_and_advance(job: StoreJob) -> (StoreCounts, StoreEnd) {
    let mut counts = StoreCounts::default();
    // LQ4: a deleted conversation's unlearned turns are dropped. A flush
    // that finds its session gone after the model call writes nothing.
    match ConversationRepository::new(&job.db).get_session(&job.session_id) {
        Ok(Some(_)) => {}
        Ok(None) => return (counts, StoreEnd::SessionGone),
        Err(e) => {
            return (
                counts,
                StoreEnd::Held(format!("session lookup failed: {e:#}")),
            );
        }
    }

    // Rule 12 (§1.4(d)): a cited row a forget suppressed never becomes a
    // source again, so a replay cannot resurrect a forgotten note.
    let keys: Vec<String> = job
        .items
        .iter()
        .flat_map(|i| i.rows.iter().map(CitedRow::suppression_key))
        .collect();
    let suppressed = match job.service.suppressed_sources(&job.ctx, &keys) {
        Ok(s) => s,
        Err(e) => return (counts, StoreEnd::Held(e.to_string())),
    };
    let mut survivors: Vec<ValidItem> = Vec::new();
    for mut item in job.items {
        item.rows
            .retain(|r| !suppressed.contains(&r.suppression_key()));
        if item.rows.is_empty() {
            counts.dropped_suppressed += 1;
        } else {
            survivors.push(item);
        }
    }

    // §2.5: each item's target — its `replaces` ref, else its key's R9
    // winner in the chunk's project partition, then the home one.
    let wanted: Vec<String> = survivors
        .iter()
        .filter(|i| i.replaces.is_none())
        .filter_map(|i| i.fact_key.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let by_key = match job.service.learning_key_targets(&job.ctx, &wanted) {
        Ok(found) => found,
        Err(e) => return (counts, StoreEnd::Held(e.to_string())),
    };
    let listed_ids: HashSet<_> = job.listed.iter().map(|n| n.id).collect();
    let mut targeted: Vec<(ValidItem, Option<Target>)> = survivors
        .into_iter()
        .map(|item| {
            let target = match item.replaces {
                Some(k) => job.listed.get(k).map(|note| Target {
                    note: note.clone(),
                    by_ref: true,
                    listed: true,
                }),
                None => item
                    .fact_key
                    .as_ref()
                    .and_then(|key| by_key.get(key))
                    .map(|note| Target {
                        note: note.clone(),
                        by_ref: false,
                        listed: listed_ids.contains(&note.id),
                    }),
            };
            (item, target)
        })
        .collect();

    // Rule 11, again: two items that reach one target by different routes.
    let target_of: HashMap<usize, String> = targeted
        .iter()
        .filter_map(|(i, t)| t.as_ref().map(|t| (i.order, t.note.id.to_string())))
        .collect();
    let items: Vec<ValidItem> = targeted.iter().map(|(i, _)| i.clone()).collect();
    let (kept, dropped) = keep_latest(items, |i| target_of.get(&i.order).cloned());
    counts.dropped_duplicate_key += dropped;
    let kept_orders: HashSet<usize> = kept.iter().map(|i| i.order).collect();
    targeted.retain(|(i, _)| kept_orders.contains(&i.order));

    // Rule 13, then the change test.
    let mut planned: Vec<(ValidItem, Plan)> = Vec::new();
    for (item, target) in targeted {
        if already_learned(&item, target.as_ref(), &job.from_this_window) {
            counts.dropped_already_learned += 1;
            continue;
        }
        match plan(&item, target.as_ref()) {
            Plan::Unchanged => counts.dropped_unchanged += 1,
            p => planned.push((item, p)),
        }
    }

    // Rule 14: at most `max_items` survive, in the model's order.
    if planned.len() > job.max_items {
        counts.dropped_over_cap = planned.len() - job.max_items;
        planned.truncate(job.max_items);
    }

    for (item, plan) in planned {
        // §2.5's table: the file layer applies it (S1, S2) — an automatic
        // replacement of a learned note, a suggestion for one the user
        // saved, edited by hand or imported, and an inferred candidate that
        // retires nothing whatever it names.
        let target = match &plan {
            Plan::New => None,
            Plan::Automatic(t) | Plan::Suggestion(t) | Plan::Inferred(t) => Some(t),
            Plan::Unchanged => continue,
        };
        let provenance = if item.user_words {
            ExtractionProvenance::UserWords
        } else {
            ExtractionProvenance::OtherOutput
        };
        let observed_at = item.observed_at();
        let date = observed_at
            .map(|t| t.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| job.today.clone());
        // Ruling LR10: a replacement goes to its target's partition;
        // LR17: a learned `user.*` fact that replaces nothing is global.
        let scope = match target {
            Some(t) => Some(t.scope),
            None if is_user_key(item.fact_key.as_deref()) => Some(NoteScope::Global),
            None => None,
        };
        let input = StoreInput {
            body: note_body(&item, target, &date),
            title: item.title.clone(),
            kind: Some(item.kind),
            scope,
            aliases: item.aliases.clone(),
            fact_key: item.fact_key.clone(),
            sources: item.rows.iter().map(CitedRow::source).collect(),
            observed_at,
            valid_from: item.valid_from,
            valid_to: item.valid_to,
            supersedes: target.map(|t| (t.id, t.revision.clone())),
            replace_policy: if target.is_some() {
                ReplacePolicy::SuggestUnlessLearned
            } else {
                ReplacePolicy::Direct
            },
            provenance: Some(provenance),
            ..Default::default()
        };
        match job.service.store(&job.ctx, input) {
            Ok(out) if out.dedup == Dedup::Existing => counts.deduplicated += 1,
            Ok(out) => {
                counts.stored += 1;
                if out.suggestion {
                    counts.candidates += 1;
                    counts.suggestions += 1;
                } else if item.user_words {
                    counts.active += 1;
                    if matches!(plan, Plan::Automatic(_)) {
                        counts.replaced += 1;
                    }
                } else {
                    counts.candidates += 1;
                }
            }
            // §2.5, *The race*: the target moved or went since the lookup.
            // The item is dropped and the chunk still advances — the change
            // is stated again the next time the user mentions it.
            Err(MemoryError::RevisionConflict { .. } | MemoryError::NotFound(_))
                if target.is_some() =>
            {
                counts.dropped_conflict += 1;
            }
            Err(e @ (MemoryError::Internal(_) | MemoryError::Quarantined { .. })) => {
                return (counts, StoreEnd::Held(e.to_string()));
            }
            Err(e) => {
                tracing::warn!(session_id = %job.session_id, "memory learning: the service refused a note: {e}");
                counts.failed += 1;
            }
        }
    }

    (job.hooks.after_store_loop)();
    if job.hooks.skip_advance {
        // A test's crash between steps 3 and 4.
        return (counts, StoreEnd::Held("injected fault".to_string()));
    }
    match LearningCursorRepository::new(&job.db).advance_cas(
        &job.session_id,
        job.through,
        job.expected,
    ) {
        Ok(true) => (counts, StoreEnd::Advanced),
        Ok(false) => match ConversationRepository::new(&job.db).get_session(&job.session_id) {
            Ok(None) => (counts, StoreEnd::SessionGone),
            _ => (counts, StoreEnd::CursorMoved),
        },
        Err(e) => (
            counts,
            StoreEnd::Held(format!("cursor advance failed: {e:#}")),
        ),
    }
}
