//! The service's inputs and outputs — the §7a contracts, minus the JSON
//! (the tools and the routes render them).

use chrono::{DateTime, Utc};
use openalpaca_storage::memory_files::{
    Dedup, EffectiveStatus, NoteId, NoteKind, NoteScope, NoteSource, NoteStatus, Origin,
};
use openalpaca_storage::memory_index::{IndexState, MatchReason};

use super::context::Surface;

/// What an extraction write is made of (design §7a `Extraction`): the user's
/// own words become an active `extracted_user` note; assistant or tool
/// output becomes an `extracted_other` candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionProvenance {
    UserWords,
    OtherOutput,
}

/// `memory_store` (design §7a).
#[derive(Debug, Clone, Default)]
pub struct StoreInput {
    pub title: String,
    pub body: String,
    pub kind: Option<NoteKind>,
    /// Absent: by kind (design §3 rule 2).
    pub scope: Option<NoteScope>,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub fact_key: Option<String>,
    /// Verified against the database; an unknown id is refused.
    pub sources: Vec<NoteSource>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    /// `{id, expected_revision}` of the note this one replaces (design §6).
    pub supersedes: Option<(NoteId, String)>,
    /// Required on the `Extraction` surface, refused on every other.
    pub provenance: Option<ExtractionProvenance>,
}

/// One note background learning may show the extractor, or check an item
/// against (design 09 §2.3): read from the index and verified against its
/// file. Built by [`super::MemoryService::learning_view`] and
/// [`super::MemoryService::learning_key_target`] only.
#[derive(Debug, Clone, PartialEq)]
pub struct KnownNote {
    pub id: NoteId,
    /// The file's revision as the verification read it — what a
    /// replacement's `expected_revision` binds.
    pub revision: String,
    /// `home-<owner-key>` or `proj-<owner-key>-<dir-key>`.
    pub partition_key: String,
    pub scope: NoteScope,
    pub title: String,
    pub kind: NoteKind,
    /// The derived origin (design §4): what tells a learned note from one
    /// the user saved or edited.
    pub origin: Origin,
    /// `Active` or `Candidate`; nothing else is ever known.
    pub effective_status: EffectiveStatus,
    pub fact_key: Option<String>,
    /// The note's `## Knowledge` section, or — for a note learned before L2 —
    /// its body below the title ([`openalpaca_storage::memory_files::knowledge_text`]).
    pub knowledge: String,
    /// The ids its own frontmatter lists in `supersedes`.
    pub supersedes: Vec<NoteId>,
    /// A learned note (`extracted_user` or `extracted_other`) that cites a
    /// row of the chunk being learned (design 09 §2.3 step 2).
    pub from_this_window: bool,
}

/// What learning's known-notes lookup found for one chunk (design 09 §2.3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LearningView {
    /// At most 20 effectively active notes the extractor may be shown: this
    /// window's own first, then the search's by best score. The prompt
    /// lists as many as its byte budget holds.
    pub notes: Vec<KnownNote>,
    /// This window's learned notes, **active and candidate** — the re-run
    /// guard's input (design 09 §1.4(b)). A candidate is never shown to the
    /// model: it is never a replacement target.
    pub from_this_window: Vec<KnownNote>,
    /// At most 150 distinct `fact_key`s of effectively active notes, the
    /// most recently updated first.
    pub keys: Vec<String>,
}

/// One `memory_changed` a mutation produced (design §7a) — the routes
/// publish it; the service records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryChange {
    pub partition_key: String,
    pub note_id: NoteId,
    /// `created | updated | archived | confirmed | forgotten | external`.
    pub change: &'static str,
    pub revision: Option<String>,
    pub origin: Origin,
    /// `None` for a change a reconciliation scan discovered (§7b P13).
    pub surface: Option<Surface>,
}

impl MemoryChange {
    /// The bus event this change is published as (design §7a
    /// `memory_changed`): the service's own words, stamped now.
    pub fn to_event(&self) -> crate::events::SystemEvent {
        crate::events::SystemEvent::MemoryChanged {
            partition_key: self.partition_key.clone(),
            note_id: self.note_id.to_string(),
            change: self.change.to_string(),
            revision: self.revision.clone(),
            origin: self.origin.as_str().to_string(),
            surface: self.surface.map(|s| s.as_str().to_string()),
            timestamp: Utc::now(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StoreOutput {
    pub id: NoteId,
    pub revision: String,
    pub scope: NoteScope,
    pub origin: Origin,
    pub superseded: Vec<NoteId>,
    pub dedup: Dedup,
    pub change: Option<MemoryChange>,
}

/// `memory_update.action` (design §7a).
#[derive(Debug, Clone)]
pub enum UpdateAction {
    Edit(Patch),
    Archive,
    Confirm,
}

/// `memory_update.patch`: an absent field is untouched; `Some(None)` clears
/// a nullable one.
#[derive(Debug, Clone, Default)]
pub struct Patch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub fact_key: Option<Option<String>>,
    pub valid_from: Option<Option<DateTime<Utc>>>,
    pub valid_to: Option<Option<DateTime<Utc>>>,
    pub review_after: Option<Option<DateTime<Utc>>>,
}

#[derive(Debug, Clone)]
pub struct UpdateInput {
    pub id: NoteId,
    pub expected_revision: String,
    pub action: UpdateAction,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UpdateOutput {
    pub id: NoteId,
    pub revision: String,
    pub status: NoteStatus,
    pub change: Option<MemoryChange>,
}

#[derive(Debug, Clone)]
pub struct ForgetInput {
    pub targets: Vec<(NoteId, String)>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgetItemStatus {
    Forgotten,
    RevisionConflict { current_revision: String },
    NotFound,
}

#[derive(Debug, Clone)]
pub struct ForgetItem {
    pub id: NoteId,
    pub status: ForgetItemStatus,
}

/// What a forget does not erase (design §6, *Forget contract*).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retained {
    pub transcript: bool,
    pub session_logs: bool,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct ForgetOutput {
    pub results: Vec<ForgetItem>,
    pub retained: Retained,
    pub changes: Vec<MemoryChange>,
}

/// `memory_read` (design §7a).
#[derive(Debug, Clone, Default)]
pub struct ReadInput {
    pub id: NoteId,
    pub expected_revision: Option<String>,
    pub section: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReadOutput {
    pub id: NoteId,
    pub revision: String,
    pub title: String,
    pub kind: NoteKind,
    pub scope: NoteScope,
    pub status: NoteStatus,
    pub effective_status: EffectiveStatus,
    pub origin: Origin,
    pub frontmatter: serde_json::Value,
    pub body: String,
    pub truncated: bool,
    /// Partition-relative: `notes/<uuid>--<slug>.md`.
    pub path: String,
    pub sources: Vec<NoteSource>,
    pub supersedes: Vec<NoteId>,
    pub superseded_by: Vec<NoteId>,
    pub freshness: Freshness,
    pub conflict: bool,
    pub source_forgotten: bool,
}

/// One kept revision of a note (`GET /v1/memories/{id}/history`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub revision: String,
    /// When the copy was written, as the file system remembers it.
    pub written_at: Option<DateTime<Utc>>,
}

/// `memory_list` (design §7a).
#[derive(Debug, Clone, Default)]
pub struct ListInput {
    pub kind: Option<NoteKind>,
    /// Default `active`.
    pub status: Option<NoteStatus>,
    pub scope: Option<NoteScope>,
    pub cursor: Option<String>,
    /// 1–100, default 50.
    pub limit: Option<usize>,
    pub include_history: bool,
    pub include_candidates: bool,
}

#[derive(Debug, Clone)]
pub struct ListItem {
    pub id: NoteId,
    pub revision: String,
    pub title: String,
    pub kind: NoteKind,
    pub scope: NoteScope,
    pub status: NoteStatus,
    pub effective_status: EffectiveStatus,
    pub origin: Origin,
    pub updated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub pinned: bool,
}

#[derive(Debug, Clone)]
pub struct ListOutput {
    pub items: Vec<ListItem>,
    pub next_cursor: Option<String>,
}

/// `Freshness` (design §7a): `verified` when the file was re-read and its
/// hash matched the index; `stale` when it had changed behind the service's
/// back and the fresh bytes were served; `unverified` in a quarantined
/// partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Verified,
    Stale,
    Unverified,
}

impl Freshness {
    pub fn as_str(self) -> &'static str {
        match self {
            Freshness::Verified => "verified",
            Freshness::Stale => "stale",
            Freshness::Unverified => "unverified",
        }
    }
}

/// `memory_search` (design §7a).
#[derive(Debug, Clone, Default)]
pub struct SearchInput {
    pub query: String,
    pub kind: Option<NoteKind>,
    pub scope: Option<NoteScope>,
    /// 1–20, default 8.
    pub limit: Option<usize>,
    pub historical: bool,
    pub as_of: Option<DateTime<Utc>>,
}

/// One search result, verified against the file it cites (design §5 step 7).
#[derive(Debug, Clone)]
pub struct SearchResultItem {
    pub id: NoteId,
    pub revision: String,
    pub title: String,
    pub kind: NoteKind,
    pub scope: NoteScope,
    pub status: NoteStatus,
    pub effective_status: EffectiveStatus,
    pub origin: Origin,
    pub excerpt: String,
    pub heading: Option<String>,
    /// 1-based file lines, inclusive.
    pub line_range: (u32, u32),
    pub matches: Vec<MatchReason>,
    pub freshness: Freshness,
    /// §7b P3: carries an un-honored `supersedes` link, or is a same-key
    /// loser.
    pub conflict: bool,
    pub same_key_loser: bool,
    pub sources: Vec<NoteSource>,
    /// §7b P6: `intact` or `source_forgotten`.
    pub source_forgotten: bool,
    pub superseded_by: Vec<NoteId>,
    /// Partition-relative path.
    pub path: String,
    pub pinned: bool,
}

#[derive(Debug, Clone)]
pub struct SearchOutput {
    pub results: Vec<SearchResultItem>,
    pub partial: bool,
    pub index_state: IndexState,
}

/// `memory_used[].reason` (design §7a): `pinned` and `recall` are the
/// automatic path, `tool` a note the model fetched through `memory_search`
/// or `memory_read` in that turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsedReason {
    Pinned,
    Recall,
    Tool,
}

impl UsedReason {
    pub fn as_str(self) -> &'static str {
        match self {
            UsedReason::Pinned => "pinned",
            UsedReason::Recall => "recall",
            UsedReason::Tool => "tool",
        }
    }
}

/// One `memory_used` entry (design §5 step 8, §7a): the wire form on
/// `HandleResult`, `GatewayResponse` and the SSE `done` frame, and the key
/// of a memory section in a context bundle (`prompt_ctx::ContextKey`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MemoryUsed {
    pub id: NoteId,
    pub revision: String,
    pub reason: UsedReason,
}

/// One row of the recently-used ring (`GET /v1/memories/recently-used`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentlyUsedItem {
    pub id: NoteId,
    pub revision: String,
    pub reason: UsedReason,
    pub delivered_at: DateTime<Utc>,
    pub lane_key: Option<String>,
    pub task_id: Option<String>,
}

/// The byte budgets of design §4 (*Initial limits*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecallBudget {
    /// Combined automatic memory context, pinned notes and wrappers included.
    pub total_bytes: usize,
    /// The pinned sub-budget (design §5 step 2).
    pub pinned_bytes: usize,
    /// At most this many delivered items, pinned included.
    pub max_items: usize,
}

impl Default for RecallBudget {
    fn default() -> Self {
        Self {
            total_bytes: super::RECALL_TOTAL_BYTES,
            pinned_bytes: super::RECALL_PINNED_BYTES,
            max_items: super::RECALL_MAX_ITEMS,
        }
    }
}

/// One delivered note (design §5 step 7: id, path, heading, line range,
/// revision, sources, match reason, freshness).
#[derive(Debug, Clone)]
pub struct RecalledNote {
    pub id: NoteId,
    pub revision: String,
    pub title: String,
    pub kind: NoteKind,
    pub scope: NoteScope,
    pub path: String,
    pub heading: Option<String>,
    pub line_range: (u32, u32),
    pub matches: Vec<MatchReason>,
    pub freshness: Freshness,
    pub sources: Vec<NoteSource>,
    pub reason: UsedReason,
    /// The text delivered: the matched chunk, or the whole body of a pinned
    /// note.
    pub text: String,
}

impl RecalledNote {
    /// The quoted-data block for one note.
    pub fn render(&self) -> String {
        let sources: Vec<String> = self.sources.iter().map(|s| s.suppression_key()).collect();
        let matches: Vec<&str> = self.matches.iter().map(|m| m.as_str()).collect();
        let heading = self
            .heading
            .as_deref()
            .map(|h| h.replace('"', "'"))
            .unwrap_or_default();
        format!(
            "[memory id={} revision={} path={} kind={} scope={} heading=\"{}\" lines={}-{} match={} freshness={} sources={} reason={}]\n{}\n[/memory]\n",
            self.id,
            self.revision,
            self.path,
            self.kind,
            self.scope,
            heading,
            self.line_range.0,
            self.line_range.1,
            if matches.is_empty() {
                "-".to_string()
            } else {
                matches.join(",")
            },
            self.freshness.as_str(),
            if sources.is_empty() {
                "-".to_string()
            } else {
                sources.join(",")
            },
            self.reason.as_str(),
            self.text.trim_end_matches('\n'),
        )
    }
}

/// What a recall delivered (design §5 step 8).
#[derive(Debug, Clone)]
pub struct RecallResult {
    pub notes: Vec<RecalledNote>,
    /// Something matched but did not fit the budget or the item cap.
    pub truncated: bool,
    pub partial: bool,
    pub index_state: IndexState,
    /// The rendered block's size in bytes, wrappers included.
    pub bytes: usize,
    /// The pinned notes' text bytes, against the pinned sub-budget (their
    /// wrappers count against the total).
    pub pinned_bytes: usize,
}

impl RecallResult {
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// `memory_used` for the turn's `done` frame (design §7a).
    pub fn memory_used(&self) -> Vec<MemoryUsed> {
        self.notes
            .iter()
            .map(|n| MemoryUsed {
                id: n.id,
                revision: n.revision.clone(),
                reason: n.reason,
            })
            .collect()
    }

    /// The whole delivery as quoted data with its trust label (design §5
    /// step 8): reference data in user or tool context, never a system
    /// instruction. Empty when nothing was delivered.
    pub fn render(&self) -> String {
        super::recall::render_block(&self.notes)
    }
}
