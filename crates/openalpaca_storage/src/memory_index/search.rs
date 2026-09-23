//! Search: candidate generation from both FTS tables, the substring pass and
//! the exact lookups; the filters, applied before the limit; ranking.

use chrono::{DateTime, Utc};

use super::{EffectiveStatus, IndexState};

/// Why a hit matched (design §7a `MatchReason`). The substring pass for one-
/// and two-character queries adds no reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MatchReason {
    /// The whole normalized query equals the note's title.
    Title,
    /// The whole normalized query equals one of the note's aliases.
    Alias,
    /// The query equals the note's `fact_key`.
    FactKey,
    /// A term matched a whole token (the `unicode61` table).
    Term,
    /// A term matched only as a substring (the trigram table).
    Trigram,
    /// An identifier or path literal matched as a phrase.
    Phrase,
}

impl MatchReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Alias => "alias",
            Self::FactKey => "fact_key",
            Self::Term => "term",
            Self::Trigram => "trigram",
            Self::Phrase => "phrase",
        }
    }
}

/// Which filter set applies (design §5 step 5; fixture README, *Slice 2 has
/// no service*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchMode {
    /// `memory_search`: a same-`fact_key` loser and a `source_forgotten` note
    /// are returned, flagged.
    #[default]
    Explicit,
    /// The automatic per-turn recall: both are excluded.
    Automatic,
}

/// One search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// 1–512 characters.
    pub query: String,
    /// The partitions the caller resolved and authorized (design §5 step 1).
    /// Empty means nothing is searched.
    pub partition_keys: Vec<String>,
    /// A `NoteKind` string.
    pub kind: Option<String>,
    /// 1–20.
    pub limit: usize,
    /// `true` includes superseded and archived notes. Candidates are never
    /// returned.
    pub historical: bool,
    /// Evaluate effective validity at this instant. Absent: at now, unless
    /// `historical`, which then applies no validity filter.
    pub as_of: Option<DateTime<Utc>>,
    pub mode: SearchMode,
}

impl SearchQuery {
    pub fn new(query: impl Into<String>, partition_keys: Vec<String>) -> Self {
        Self {
            query: query.into(),
            partition_keys,
            kind: None,
            limit: super::DEFAULT_LIMIT,
            historical: false,
            as_of: None,
            mode: SearchMode::Explicit,
        }
    }
}

/// One result.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub id: String,
    pub revision: String,
    pub partition_key: String,
    pub scope: String,
    pub relative_path: String,
    pub title: String,
    pub kind: String,
    /// The stored `NoteStatus`.
    pub status: String,
    pub effective_status: EffectiveStatus,
    pub origin: String,
    pub fact_key: Option<String>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub pinned: bool,
    pub conflict: bool,
    pub same_key_loser: bool,
    pub source_forgotten: bool,
    pub superseded_by: Vec<String>,
    /// The best chunk's text — the lines `line_range` names.
    pub excerpt: String,
    pub heading: Option<String>,
    /// 1-based file lines, inclusive.
    pub line_range: (u32, u32),
    pub chunk_ordinal: u32,
    /// Sorted, deduplicated.
    pub matches: Vec<MatchReason>,
    pub score: f64,
}

/// A search's results.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    /// `hits.len()`.
    pub count: usize,
    /// A scan cap was hit: the substring pass or a candidate cap.
    pub partial: bool,
    pub index_state: IndexState,
}
