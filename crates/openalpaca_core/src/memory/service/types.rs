//! The service's inputs and outputs — the §7a contracts, minus the JSON
//! (the tools and the routes render them).

use chrono::{DateTime, Utc};
use openalpaca_storage::memory_files::{
    Dedup, EffectiveStatus, NoteId, NoteKind, NoteScope, NoteSource, NoteStatus, Origin,
    ReplacePolicy,
};
use openalpaca_storage::memory_index::{IndexState, MatchReason, SearchOrder};

use super::context::Surface;
use crate::daemon_config::RecallMode;

/// What an extraction write is made of (design §7a `Extraction`): the user's
/// own words become an active `extracted_user` note; assistant or tool
/// output becomes an `extracted_other` candidate; a conversation summary
/// becomes an **active** `extracted_other` `episode` (design 09 §13.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionProvenance {
    UserWords,
    OtherOutput,
    /// A conversation summary (design 09 §13.3, MD-16): the model's own
    /// wording, so origin `extracted_other`, but `active` — a record of a
    /// conversation, not a claim about the user — so search, the timeline,
    /// the list and the Memory view find it, while automatic recall never
    /// delivers it ([`openalpaca_storage::memory_files::is_conversation_summary`]).
    /// Only a plain `episode` may carry it: no `fact_key`, no `supersedes`,
    /// no replace policy.
    Summary,
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
    /// When the evidence was observed — for a learned note, the time of its
    /// newest cited row (design 09 §2.5). Accepted on the `Extraction`
    /// surface only.
    pub observed_at: Option<DateTime<Utc>>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    /// `{id, expected_revision}` of the note this one replaces (design §6).
    pub supersedes: Option<(NoteId, String)>,
    /// How `supersedes` is applied (design 09 §3.2, S2). Anything but the
    /// default is accepted on the `Extraction` surface only.
    pub replace_policy: ReplacePolicy,
    /// Required on the `Extraction` surface, refused on every other.
    pub provenance: Option<ExtractionProvenance>,
}

/// One note background learning may show the extractor, or check an item
/// against (design 09 §2.3): read from the index and verified against its
/// file. Built by [`super::MemoryService::learning_view`] and
/// [`super::MemoryService::learning_key_targets`] only.
#[derive(Debug, Clone, PartialEq)]
pub struct KnownNote {
    pub id: NoteId,
    /// The file's revision as the verification read it — what a
    /// replacement's `expected_revision` binds.
    pub revision: String,
    /// `home-<owner-key>` or `proj-<owner-key>-<project-key>`.
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
    /// Only the chat model wrote it: origin `extracted_user` with no
    /// `observed_at`, which the extractor alone sets (owner decisions LQ14,
    /// LQ19).
    pub by_chat_model: bool,
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
    /// The note is a suggestion (design 09 §3.1): a candidate naming the
    /// note it would replace, which retires nothing until the user confirms
    /// it.
    pub suggestion: bool,
    pub change: Option<MemoryChange>,
}

/// `memory_update.action` (design §7a).
#[derive(Debug, Clone)]
pub enum UpdateAction {
    Edit(Patch),
    Archive,
    /// `target_revision`: the revision of the note a suggestion replaces, as
    /// the confirming user last saw it (design 09 §3.2, S5). Required for a
    /// candidate that carries `supersedes`; ignored for one that does not.
    Confirm {
        target_revision: Option<String>,
    },
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
    /// A confirmed suggestion: the note it now retires (design 09 §3.5).
    /// Empty for every other update.
    pub superseded: Vec<NoteId>,
    /// A confirmed suggestion: the other pending suggestions for the same
    /// note, each withdrawn after the confirm was written (S6).
    pub dismissed: Vec<NoteId>,
    /// Owner decision LQ11: the chat model's edit of a note the user saved,
    /// edited by hand or imported was written as a suggestion naming that
    /// note — this one — and `id` is the suggestion's, not the note's.
    /// `None` for every other update.
    pub suggested_for: Option<NoteId>,
}

/// What a withdrawal did (design 09 §3.3, ruling LR12): the one note is
/// gone — its tombstone keeps no link and suppresses no source — and the
/// notes it had retired are effective again.
#[derive(Debug, Clone)]
pub struct WithdrawOutput {
    pub id: NoteId,
    /// The notes this one had superseded that the withdrawal brought back.
    /// Empty for a suggestion, which retired nothing.
    pub restored: Vec<NoteId>,
    pub change: MemoryChange,
}

/// The note a suggestion would replace (design 09 §3.5), as verified
/// against its file.
#[derive(Debug, Clone)]
pub struct SuggestionTarget {
    pub id: NoteId,
    pub revision: String,
    pub title: String,
    /// Its `## Knowledge` section (or its body below the title), cut to
    /// [`super::SUGGESTION_EXCERPT_BYTES`].
    pub excerpt: String,
    /// How many characters of that section the cut left out: 0 when the
    /// excerpt is the whole of it.
    pub excerpt_left_out: usize,
    pub origin: Origin,
    pub scope: NoteScope,
    /// Its metadata, which the suggestion's `changes` are read against (not
    /// served on its own).
    pub metadata: NoteMetadata,
}

/// A note's metadata a suggestion can change besides its title and text,
/// field by field in a fixed order — `fact_key`, `aliases`, `tags`,
/// `valid_from`, `valid_to` — each as the values a card shows: a list's
/// items, the key or the bound; none is empty (the L8 close-out's A4).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NoteMetadata(pub Vec<(&'static str, Vec<String>)>);

/// One field of that metadata a suggestion changes: the replaced note's
/// values and the suggestion's (`GET /v1/memories/suggestions`'
/// `changes`; the L8 close-out's A4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldChange {
    /// `fact_key`, `aliases`, `tags`, `valid_from` or `valid_to`.
    pub field: &'static str,
    pub from: Vec<String>,
    pub to: Vec<String>,
}

impl NoteMetadata {
    /// The fields whose values differ from `other`'s, in order: what a
    /// suggestion with metadata `other` changes of the note with this.
    pub fn changes_to(&self, other: &NoteMetadata) -> Vec<FieldChange> {
        self.0
            .iter()
            .zip(&other.0)
            .filter(|((_, from), (_, to))| from != to)
            .map(|((field, from), (_, to))| FieldChange {
                field,
                from: from.clone(),
                to: to.clone(),
            })
            .collect()
    }
}

/// One pending suggestion (`GET /v1/memories/suggestions`, design 09
/// §3.5): a candidate that names the note it would replace, the newest of
/// those naming that note.
#[derive(Debug, Clone)]
pub struct Suggestion {
    pub id: NoteId,
    pub revision: String,
    pub title: String,
    /// The suggested text: its `## Knowledge` section — shown beside the
    /// target's, because a suggestion can merely restate it (§2.5 case 3) —
    /// cut to 4 KiB.
    pub knowledge: String,
    /// How many characters of that section the cut left out: 0 when
    /// `knowledge` is the whole of it.
    pub knowledge_left_out: usize,
    /// Why it replaces the target, from its `Replaces …` evidence sentence
    /// (§2.6); `None` when the note carries none, and for the chat model's
    /// own suggestion, whose body may copy its target's with that note's
    /// old sentence (the L8 close-out's A4).
    pub reason: Option<String>,
    /// What else it changes of the target: each metadata field whose values
    /// differ, empty when only the title or the text do.
    pub changes: Vec<FieldChange>,
    pub created_at: DateTime<Utc>,
    /// When the evidence was observed: the newest cited row's time.
    pub observed_at: Option<DateTime<Utc>>,
    /// `extracted_user` — from the user's words — or `extracted_other`,
    /// inferred from the conversation.
    pub origin: Origin,
    pub scope: NoteScope,
    pub sources: Vec<NoteSource>,
    /// How many older pending suggestions name the same target and are not
    /// listed.
    pub older_count: usize,
    pub target: SuggestionTarget,
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

/// `memory_read` with its ids (design 09 §13.1, MD-14): 1–10 notes in one
/// call, inside the one 32 KiB read budget. `expected_revision` and
/// `section` go with a single id.
#[derive(Debug, Clone, Default)]
pub struct ReadManyInput {
    pub ids: Vec<NoteId>,
    pub expected_revision: Option<String>,
    pub section: Option<String>,
}

/// One id of a batch read, in the order asked.
#[derive(Debug, Clone)]
pub enum ReadManyItem {
    Read(Box<ReadOutput>),
    /// No note of this owner's partitions has the id — another owner's note
    /// reads exactly like one that never existed.
    NotFound(NoteId),
    /// `expected_revision` named another revision (a single-id read).
    RevisionConflict {
        id: NoteId,
        current_revision: String,
    },
}

#[derive(Debug, Clone)]
pub struct ReadManyOutput {
    pub items: Vec<ReadManyItem>,
    /// The bytes the bodies shared ([`super::READ_MAX_BYTES`]).
    pub budget_bytes: usize,
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
    /// The ids its own frontmatter names in `supersedes` (design 09 §3.2,
    /// S8): what a replacement replaced, or what a suggestion would.
    pub supersedes: Vec<NoteId>,
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

/// `memory_search.detail` (design 09 §13.1): what one result carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchDetail {
    /// One index row per note — `{id, date, kind, scope, status, title,
    /// reason, size}` and no body — the tool's default.
    #[default]
    Index,
    /// The cited excerpt of design §7a, as before.
    Snippets,
}

impl SearchDetail {
    pub fn as_str(self) -> &'static str {
        match self {
            SearchDetail::Index => "index",
            SearchDetail::Snippets => "snippets",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "index" => Some(SearchDetail::Index),
            "snippets" => Some(SearchDetail::Snippets),
            _ => None,
        }
    }
}

/// `memory_search` (design §7a; design 09 §13.1).
#[derive(Debug, Clone, Default)]
pub struct SearchInput {
    pub query: String,
    pub kind: Option<NoteKind>,
    pub scope: Option<NoteScope>,
    /// 1–50, default 10 (design 09 §13.1).
    pub limit: Option<usize>,
    pub historical: bool,
    pub as_of: Option<DateTime<Utc>>,
    /// Only notes dated at or after this instant — the date is `observed_at`,
    /// else `created_at` (design 09 §13.1).
    pub date_from: Option<DateTime<Utc>>,
    /// Only notes dated at or before this instant.
    pub date_to: Option<DateTime<Utc>>,
    pub order: SearchOrder,
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
    /// The note's effective window, from the re-read file (design 09 §6.1).
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    /// `observed_at`, else `created_at`, from the re-read file (design 09
    /// §13.1).
    pub date: DateTime<Utc>,
    /// The note's approximate size in tokens — its body's bytes ÷ 4 — so a
    /// model can budget a read.
    pub size: usize,
    /// The window has ended at the instant the search evaluated validity.
    pub ended: bool,
}

impl SearchResultItem {
    /// The result as an index row (design 09 §13.1): the reason is the match
    /// reasons, joined — `-` for a short query's substring hit (§7b P5).
    pub fn index_row(&self) -> IndexRow {
        IndexRow {
            id: self.id,
            date: self.date,
            kind: self.kind,
            scope: self.scope,
            status: RowStatus::of(self.effective_status, self.ended),
            title: self.title.clone(),
            reason: match_reasons(&self.matches),
            size: self.size,
            conflict: self.conflict,
        }
    }
}

/// The match reasons of one hit as the recall header writes them: joined by
/// `,`, or `-` when there is none.
pub fn match_reasons(matches: &[MatchReason]) -> String {
    if matches.is_empty() {
        "-".to_string()
    } else {
        matches
            .iter()
            .map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Where a listed note stands (design 09 §13.1): in force, or out of recall
/// for one of the reasons a timeline marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowStatus {
    Active,
    Superseded,
    Archived,
    /// Active, but its validity window has ended.
    Ended,
}

impl RowStatus {
    pub fn of(effective: EffectiveStatus, ended: bool) -> Self {
        match effective {
            EffectiveStatus::Superseded => RowStatus::Superseded,
            EffectiveStatus::Archived => RowStatus::Archived,
            // A candidate is never listed; were one handed here, it reads as
            // what it is not yet — in force.
            EffectiveStatus::Active | EffectiveStatus::Candidate if ended => RowStatus::Ended,
            EffectiveStatus::Active | EffectiveStatus::Candidate => RowStatus::Active,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RowStatus::Active => "active",
            RowStatus::Superseded => "superseded",
            RowStatus::Archived => "archived",
            RowStatus::Ended => "ended",
        }
    }
}

/// One index row (design 09 §13.1): what `memory_search` answers by default
/// and `memory_timeline` lists — about 50–100 tokens, and no body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRow {
    pub id: NoteId,
    /// `observed_at`, else `created_at`.
    pub date: DateTime<Utc>,
    pub kind: NoteKind,
    pub scope: NoteScope,
    pub status: RowStatus,
    pub title: String,
    /// Why it is listed: the match reasons of a search hit; `before`,
    /// `anchor` or `after` on a timeline.
    pub reason: String,
    /// Approximate tokens: the body's bytes ÷ 4, rounded up.
    pub size: usize,
    /// §7b P3/P4: another note outranks it, or it carries a link nothing
    /// honours — the flag a search result has always carried.
    pub conflict: bool,
}

/// What a timeline is drawn around (design 09 §13.1): exactly one of a note,
/// the first hit of a search, or an instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineAnchor {
    Note(NoteId),
    Query(String),
    Date(DateTime<Utc>),
}

/// `memory_timeline` (design 09 §13.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineInput {
    pub anchor: TimelineAnchor,
    /// Notes before the anchor: 0–20, default 5.
    pub before: Option<usize>,
    /// Notes after it: 0–20, default 5.
    pub after: Option<usize>,
}

/// A timeline: index rows in time order — `before`, the anchor note (when
/// the anchor is a note), `after`.
#[derive(Debug, Clone)]
pub struct TimelineOutput {
    /// The note it was drawn around; `None` for an instant, and for a query
    /// nothing matched (then nothing is listed).
    pub anchor_note: Option<NoteId>,
    /// The instant it was drawn around, for a date anchor.
    pub anchor_date: Option<DateTime<Utc>>,
    pub rows: Vec<IndexRow>,
    pub index_state: IndexState,
}

#[derive(Debug, Clone)]
pub struct SearchOutput {
    pub results: Vec<SearchResultItem>,
    pub partial: bool,
    pub index_state: IndexState,
}

/// `memory_used[].reason` (design §7a): `pinned`, `recall` and `index` are
/// the automatic path, `tool` a note the model fetched through
/// `memory_search`, `memory_timeline` or `memory_read` in that turn — an
/// index row counts, its title reached the model (design 09 §13.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsedReason {
    Pinned,
    Recall,
    /// Listed, not delivered: one line of the automatic recall's index block
    /// (design 09 §13.2) — its id, date, kind, title and match reason reached
    /// the model, its body did not.
    Index,
    Tool,
}

impl UsedReason {
    pub fn as_str(self) -> &'static str {
        match self {
            UsedReason::Pinned => "pinned",
            UsedReason::Recall => "recall",
            UsedReason::Index => "index",
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

/// The budgets of design §4 (*Initial limits*), and how the search's notes
/// are delivered within them (design 09 §13.2): as excerpts, or the first
/// few as excerpts and the next ones listed one line each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecallBudget {
    /// Combined automatic memory context, pinned notes, wrappers and the
    /// index block included.
    pub total_bytes: usize,
    /// The pinned sub-budget (design §5 step 2).
    pub pinned_bytes: usize,
    /// At most this many delivered items — excerpts, pinned included.
    pub max_items: usize,
    /// At most this many of the search's notes are delivered as excerpts.
    pub excerpts: usize,
    /// At most this many further notes are listed in the index block; `0`,
    /// and there is no index block.
    pub listed: usize,
}

impl RecallBudget {
    /// The budget `[orchestrator.memory] recall_mode` names (design 09
    /// §13.2): `excerpts` is design §4's — every note that fits the 8 KiB
    /// and the 8 items an excerpt, nothing listed; `index` excerpts the top
    /// 2 and lists up to 8 more, inside the same 8 KiB.
    pub fn for_mode(mode: RecallMode) -> Self {
        let excerpts = Self {
            total_bytes: super::RECALL_TOTAL_BYTES,
            pinned_bytes: super::RECALL_PINNED_BYTES,
            max_items: super::RECALL_MAX_ITEMS,
            excerpts: super::RECALL_MAX_ITEMS,
            listed: 0,
        };
        match mode {
            RecallMode::Excerpts => excerpts,
            RecallMode::Index => Self {
                excerpts: super::RECALL_INDEX_EXCERPTS,
                listed: super::RECALL_INDEX_LINES,
                ..excerpts
            },
        }
    }
}

/// Design §4's budgets: the excerpts delivery. A per-turn caller builds its
/// budget from the configured mode with [`RecallBudget::for_mode`].
impl Default for RecallBudget {
    fn default() -> Self {
        Self::for_mode(RecallMode::Excerpts)
    }
}

/// Where a recalled note's window stands, on the index's clock (design 09
/// §6.1). Only a window that has not begun is named: one that has ended
/// never reaches automatic recall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timing {
    /// `valid_from` is later than "now": the note describes a plan, not the
    /// present.
    Upcoming,
}

impl Timing {
    pub fn as_str(self) -> &'static str {
        match self {
            Timing::Upcoming => "upcoming",
        }
    }
}

/// One delivered note (design §5 step 7: id, path, heading, line range,
/// revision, sources, match reason, freshness; design 09 §6.1: the window).
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
    /// The note's effective window, from the re-read file.
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    /// `Some(Upcoming)` when the index, on its own clock, says the window
    /// has not begun.
    pub timing: Option<Timing>,
}

impl RecalledNote {
    /// The quoted-data block for one note. A note with a window says so at
    /// the end of its header — both bounds, `-` for an open one, then
    /// `timing=upcoming` when the window has not begun (design 09 §6.1); a
    /// note with none renders exactly as it did before windows were shown.
    pub fn render(&self) -> String {
        let sources: Vec<String> = self.sources.iter().map(|s| s.suppression_key()).collect();
        let matches: Vec<&str> = self.matches.iter().map(|m| m.as_str()).collect();
        let heading = self
            .heading
            .as_deref()
            .map(|h| h.replace('"', "'"))
            .unwrap_or_default();
        let mut window = String::new();
        if self.valid_from.is_some() || self.valid_to.is_some() {
            let bound =
                |t: Option<DateTime<Utc>>| t.map_or_else(|| "-".to_string(), super::format_ts);
            window = format!(
                " valid_from={} valid_to={}",
                bound(self.valid_from),
                bound(self.valid_to)
            );
            if let Some(timing) = self.timing {
                window.push_str(" timing=");
                window.push_str(timing.as_str());
            }
        }
        format!(
            "[memory id={} revision={} path={} kind={} scope={} heading=\"{}\" lines={}-{} match={} freshness={} sources={} reason={}{}]\n{}\n[/memory]\n",
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
            window,
            self.text.trim_end_matches('\n'),
        )
    }
}

/// One note the automatic recall lists rather than delivers (design 09
/// §13.2): a line of the index block, read from the verified file — the
/// model sees that it exists and why it matched, and opens it with
/// `memory_read` if it needs it.
#[derive(Debug, Clone)]
pub struct ListedNote {
    pub id: NoteId,
    pub revision: String,
    /// `observed_at`, else `created_at` — an index row's date (design 09
    /// §13.1).
    pub date: DateTime<Utc>,
    pub kind: NoteKind,
    pub title: String,
    pub matches: Vec<MatchReason>,
}

impl ListedNote {
    /// `<id> <date> <kind> “<title>” — <reason>`, one line: the date and the
    /// reason as an index row writes them (design 09 §13.1), the full id —
    /// what `memory_read` takes.
    pub fn render(&self) -> String {
        format!(
            "{} {} {} “{}” — {}\n",
            self.id,
            super::format_ts(self.date),
            self.kind,
            self.title.replace(['\r', '\n'], " "),
            match_reasons(&self.matches),
        )
    }
}

/// What a recall delivered (design §5 step 8).
#[derive(Debug, Clone)]
pub struct RecallResult {
    /// The delivered notes: pinned first, then the recalled excerpts.
    pub notes: Vec<RecalledNote>,
    /// The notes listed after them in the index block, in rank order
    /// (design 09 §13.2); empty in the excerpts delivery.
    pub listed: Vec<ListedNote>,
    /// Something matched but did not fit the budget, the item cap or the
    /// index block.
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
        self.notes.is_empty() && self.listed.is_empty()
    }

    /// `memory_used` for the turn's `done` frame (design §7a): the delivered
    /// notes, then the listed ones with reason `index` (design 09 §13.2).
    pub fn memory_used(&self) -> Vec<MemoryUsed> {
        self.notes
            .iter()
            .map(|n| MemoryUsed {
                id: n.id,
                revision: n.revision.clone(),
                reason: n.reason,
            })
            .chain(self.listed.iter().map(|l| MemoryUsed {
                id: l.id,
                revision: l.revision.clone(),
                reason: UsedReason::Index,
            }))
            .collect()
    }

    /// The whole delivery as quoted data with its trust label (design §5
    /// step 8): reference data in user or tool context, never a system
    /// instruction — the index block included. Empty when nothing was
    /// delivered.
    pub fn render(&self) -> String {
        super::recall::render_block(&self.notes, &self.listed)
    }
}
