//! One chunk's extraction (design 09 §2, the L1 subset).
//!
//! The model is asked what durable knowledge a chunk of conversation carries
//! about the user, and each item it names is validated deterministically
//! before anything is written. L1 keeps today's item fields — `title`,
//! `content`, `kind`, `stated_by`, `confidence` — and adds `rows`, the
//! transcript rows that state the item, so provenance is checked rather than
//! trusted (ruling LR3) and every note cites its evidence. L2 widens the
//! schema (aliases, `fact_key`, `replaces`, validity) and the prompt.
//!
//! Validation, in order (§2.4, rules 1–6, 12 and 14):
//! 1. the top level is `{"items": [...]}`, else the whole answer failed;
//! 2. `content` is present and at most 1,000 characters;
//! 3. `kind` is one of the six when present (missing = `fact`);
//! 4. `rows` names 1–8 rows of the chunk, at least one not a context row —
//!    context rows are removed and never become sources;
//! 5. `stated_by: "user"` stands only when **every** remaining cited row is
//!    the user's own typed row; anything else is inferred (a candidate);
//! 6. an item under `memory_confidence_threshold` is dropped;
//! 12. a cited row a forget suppressed is removed, and an item left citing
//!     nothing is dropped (§1.4(d));
//! 14. at most `extract_max_items_per_call` items survive, in model order.

use super::window::{LearnRow, RowKind};
use crate::memory::service::{
    ExtractionProvenance, MemoryAccessContext, MemoryError, MemoryService, StoreInput,
};
use openalpaca_llm::{ChatMessage, RequestContext, RouterRequest};
use openalpaca_storage::memory_files::{Dedup, NoteKind, NoteSource, SourceType};
use openalpaca_storage::{ConversationRepository, Database, LearningCursorRepository};
use std::collections::HashSet;
use std::sync::Arc;

/// The usage label chat learning's calls are billed and gated under.
pub(crate) const AGENT_LABEL: &str = "orchestrator_user_extract";

/// The most characters an item's `content` may have (§2.2).
const MAX_CONTENT_CHARS: usize = 1000;
/// The most rows an item may cite (§2.2).
const MAX_CITED_ROWS: usize = 8;

/// The system message: who the model is and that it answers in JSON only.
const SYSTEM: &str = "<role>You are the memory extractor for OpenAlpaca.</role>\n\
     <output_format>Output ONLY valid JSON matching the schema. No markdown fences, no commentary.</output_format>";

/// The chunk's request: today's prompt wording over a numbered transcript,
/// with `rows` added to the schema. `thinking: Disabled` (M2), temperature
/// 0, no caching and no fallbacks, as the per-turn extractor asked.
pub(crate) fn build_request(
    transcript: &str,
    max_items: usize,
    out_tokens: u32,
    model: Option<String>,
) -> RouterRequest {
    let user_prompt = format!(
        "<conversation>\n{transcript}</conversation>\n\n\
         Extract the durable knowledge this conversation carries about the user: their \
         preferences, facts about them or their work, decisions they made, procedures \
         they follow, references they gave, or an episode worth remembering.\n\n\
         <output_schema>\n\
         {{\n\
           \"items\": [\n\
             {{\n\
               \"title\": \"a short title, at most 80 characters\",\n\
               \"content\": \"one concise statement\",\n\
               \"kind\": \"preference\" | \"fact\" | \"decision\" | \"procedure\" | \"reference\" | \"episode\",\n\
               \"stated_by\": \"user\" | \"assistant\",\n\
               \"confidence\": 0.0 to 1.0,\n\
               \"rows\": [the numbers of the rows that state it, e.g. 3]\n\
             }}\n\
           ]\n\
         }}\n\
         </output_schema>\n\n\
         <extraction_rules>\n\
         - \"stated_by\": \"user\" ONLY when a `user` row says it in the user's own words, and then \
         cite only `user` rows.\n\
         - \"stated_by\": \"assistant\" when it is inferred from an `assistant` row, a `queued-task` \
         row, a long `user` row that may quote other text, or anything the user did not say \
         themselves.\n\
         - Cite in \"rows\" the numbers of the rows that state it. A `context` row is never enough \
         on its own.\n\
         - Only extract what is clearly stated or strongly implied. Do not invent.\n\
         - Skip pleasantries, the questions themselves, and anything true only for one reply.\n\
         - Be conservative: prefer fewer high-confidence items over many low-confidence ones.\n\
         - At most {max_items} items, most important first.\n\
         - If nothing can be extracted, return {{\"items\": []}}.\n\
         </extraction_rules>"
    );
    RouterRequest {
        model,
        messages: Arc::new(vec![
            ChatMessage::system(SYSTEM),
            ChatMessage::user(&user_prompt),
        ]),
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

/// An item that passed rules 2–6.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ValidItem {
    pub title: String,
    pub content: String,
    pub kind: NoteKind,
    /// Rule 5: every cited row is the user's own typed row.
    pub user_words: bool,
    /// The cited rows, context rows removed, in transcript order.
    pub rows: Vec<CitedRow>,
}

/// One row an item cites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CitedRow {
    pub id: i64,
    pub created_at: String,
}

impl CitedRow {
    fn source(&self) -> NoteSource {
        NoteSource {
            kind: SourceType::Message,
            id: self.id.to_string(),
            observed_at: chrono::NaiveDateTime::parse_from_str(
                &self.created_at,
                "%Y-%m-%d %H:%M:%S",
            )
            .ok()
            .map(|t| t.and_utc()),
        }
    }

    fn suppression_key(&self) -> String {
        format!("message:{}", self.id)
    }
}

/// What validation dropped, by rule.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Validated {
    pub items: Vec<ValidItem>,
    /// Rules 2–4.
    pub invalid: usize,
    /// Rule 6.
    pub below_threshold: usize,
}

/// Rules 2–6 over the model's items. `numbered[i]` is the row `R{i+1}`.
pub(crate) fn validate(
    items: &[serde_json::Value],
    numbered: &[&LearnRow],
    threshold: f64,
) -> Validated {
    let mut out = Validated::default();
    for value in items {
        let Some(item) = read_item(value, numbered) else {
            out.invalid += 1;
            continue;
        };
        if item.1 < threshold {
            out.below_threshold += 1;
            continue;
        }
        out.items.push(item.0);
    }
    out
}

/// Rules 2–5 for one item, with its confidence beside it.
fn read_item(value: &serde_json::Value, numbered: &[&LearnRow]) -> Option<(ValidItem, f64)> {
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
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().take(80).collect::<String>())
        .unwrap_or_else(|| content.chars().take(80).collect());
    Some((
        ValidItem {
            title: title.trim().to_string(),
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
        },
        confidence,
    ))
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

/// What one chunk's store loop wrote, counted without note bodies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StoreCounts {
    pub stored: usize,
    pub active: usize,
    pub candidates: usize,
    pub deduplicated: usize,
    pub dropped_over_cap: usize,
    pub dropped_suppressed: usize,
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
    /// already written answer `dedup` on the retry.
    Held(String),
}

/// Everything the closure needs, owned so it can run on the blocking pool.
pub(crate) struct StoreJob {
    pub db: Database,
    pub service: Arc<MemoryService>,
    pub ctx: MemoryAccessContext,
    pub session_id: String,
    pub items: Vec<ValidItem>,
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
    // Rule 14: at most `max_items` survive, in the model's order.
    if survivors.len() > job.max_items {
        counts.dropped_over_cap = survivors.len() - job.max_items;
        survivors.truncate(job.max_items);
    }

    for item in survivors {
        let provenance = if item.user_words {
            ExtractionProvenance::UserWords
        } else {
            ExtractionProvenance::OtherOutput
        };
        let input = StoreInput {
            body: format!("# {}\n\n{}\n", item.title, item.content),
            title: item.title,
            kind: Some(item.kind),
            sources: item.rows.iter().map(CitedRow::source).collect(),
            provenance: Some(provenance),
            ..Default::default()
        };
        match job.service.store(&job.ctx, input) {
            Ok(out) if out.dedup == Dedup::Existing => counts.deduplicated += 1,
            Ok(_) => {
                counts.stored += 1;
                if item.user_words {
                    counts.active += 1;
                } else {
                    counts.candidates += 1;
                }
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
