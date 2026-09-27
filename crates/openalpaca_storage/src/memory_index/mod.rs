//! `memory_index` — the disposable lexical catalog over Markdown memory notes
//! (design `tasks/memory-redesign/03-design.md` §5, §8; plan slice 2).
//!
//! One SQLite file at `state/cache/memory-index-v1.sqlite` holds, per note, a
//! catalog row (identity, revision, relative path, the pre-computed filter
//! fields, the normalized title, aliases and tags), its chunks with line spans
//! and their normalized text — the one copy of the text in the cache — and
//! three contentless FTS5 indexes over the chunks (index schema 4, design 09
//! §6.5, LQ17; schema 5 widens the unsegmented scripts):
//!
//! - `fts_word`, `unicode61` over title, aliases, tags and body, the text
//!   split at script boundaries ([`query::split_scripts`]): whole tokens, and
//!   a word's beginning through a prefix query (`prefer` finds `prefers`);
//! - `fts_tri_head`, `trigram` over title, aliases and tags: substrings of the
//!   header;
//! - `fts_tri`, `trigram` over the body of a chunk that carries a character of
//!   a script `unicode61` cannot split into words ([`query::is_unsegmented`]:
//!   CJK, Thai and the other scripts written without spaces; Devanagari, Tamil
//!   and the other scripts built with vowel signs): substrings of that text.
//!
//! [`query`] says which term goes where. A word-beginning or substring hit is
//! weighted below a whole-token hit; one- and two-character queries go
//! through a bounded substring pass that reports `partial` when its cap is
//! hit; no stemmer. Nothing finds a term inside a word of a body written in a
//! space-separated script; a title, an alias or a tag still does.
//!
//! **What this module is not.** It reads no note file, computes no revision,
//! derives no supersession and holds no snapshot: those are `memory_files`.
//! Its input is [`IndexRecord`], a plain struct whose filter fields —
//! `effective_status`, `conflict`, `same_key_loser`, `source_forgotten`,
//! `superseded_by` — arrive **already derived**; the index applies them, before
//! the limit, and reports them. The file is a cache: delete it and the next
//! open reports [`IndexState::Rebuilding`] until [`MemoryIndex::rebuild`] has
//! run. Nothing in it is authoritative.
//!
//! Byte limits are design §4's: a note body over [`MAX_NOTE_BYTES`] is refused,
//! a chunk is at most [`chunker::CHUNK_MAX_BYTES`], a query is 1–512 characters.

pub mod chunker;
pub mod query;
mod scan;
mod search;
mod timeline;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, params};

pub use search::{MatchReason, SearchHit, SearchMode, SearchOrder, SearchQuery, SearchResults};
pub use timeline::{Timeline, TimelinePoint, TimelineQuery, TimelineRow};

/// The cache file's name under `state/cache/`.
pub const INDEX_FILE_NAME: &str = "memory-index-v1.sqlite";
/// The on-disk schema this build writes. A file carrying another value is
/// recreated on open (and reports `rebuilding`), never migrated: the index is
/// disposable. Version 2 (design 09 §2.3, ruling LR18) added
/// `note.supersedes` and the `note_source` table, so learning's per-chunk
/// lookup reads a note's links and citations without parsing a partition.
/// Version 3 (design 09 §13.1, MD-14) added the note's dates — `created_at`,
/// `observed_at` and `date`, the one a search filters and orders by and a
/// timeline walks. Version 4 (design 09 §6.5, LQ17) is the layout revision:
/// `note.norm_aliases` and `norm_tags`, `chunk` holding only its normalized
/// text, the header trigram table `fts_tri_head`, `fts_tri` only over
/// unsegmented-script chunks, and all three FTS tables contentless. Version
/// 5 (LQ17's repair round 1) widens [`query::is_unsegmented`] to the
/// scripts whose words `unicode61` breaks at their vowel signs (Devanagari
/// and the rest) and to Tibetan, Yi and the other scripts written without
/// spaces, so their bodies are in `fts_tri` again.
///
/// **It versions what a row holds, not only the tables.** Since scale slice
/// S1 the service re-indexes a note only when its bytes, its file's stat or
/// its derived facts change — also across a restart, reading what it
/// indexed back from the rows — so rows written by an older build are kept
/// until their note changes. Any change to what a row holds for the same
/// note bytes — the chunker, [`query::normalize`], `authority_rank`, the
/// service's `record_for`, how `date` or `norm_title` is derived, the FTS
/// tokenizers, the script-boundary split ([`query::split_scripts`]) and the
/// ranges of [`query::is_unsegmented`], which decide what `fts_word` and
/// `fts_tri` hold — must bump this too, or the old rows are served for
/// good; the bump is what re-derives every row, by a rebuild off the
/// request path.
pub const SCHEMA_VERSION: i64 = 5;
/// Design §4: a note is at most 32 KiB. A larger body is refused by
/// [`MemoryIndex::upsert`] so the cache stays bounded by what `memory_files`
/// would have written.
pub const MAX_NOTE_BYTES: usize = 32 * 1024;
/// Design §7a: `memory_search.query` is 1–512 characters.
pub const MAX_QUERY_CHARS: usize = 512;
/// A [`SearchQuery::new`]'s own limit (design §7a). The tool's and the
/// route's default is the service's (design 09 §13.1: 10), which always
/// names its limit.
pub const DEFAULT_LIMIT: usize = 8;
/// The largest page a search returns (design 09 §13.1: `limit` 1–50).
pub const MAX_LIMIT: usize = 50;
/// How many notes one transaction of a whole rebuild indexes: the index is
/// released between batches, and the WAL checkpoints between them.
pub const REBUILD_BATCH: usize = 256;
/// How many best-ranked chunks each FTS table contributes to the candidate
/// set. The filters run inside the query, so these are *eligible* chunks; the
/// cap bounds the merge, and hitting it sets `partial`.
pub const CANDIDATE_CAP: usize = 256;
/// The bounded substring pass for one- and two-character queries scans at
/// most this many eligible chunks, in index order, and sets `partial` when
/// more exist (design §5 step 4).
pub const SUBSTRING_SCAN_CAP: usize = 4096;

// ============================================================================
// Errors
// ============================================================================

/// What the index can refuse or fail with.
#[derive(Debug)]
pub enum IndexError {
    /// A caller-supplied value the index will not accept: an empty or
    /// over-long query, a limit outside 1–20, an over-size body, a timestamp
    /// that is not RFC 3339. The message names the field.
    InvalidInput(String),
    /// The cache database itself.
    Sqlite(rusqlite::Error),
    /// The scanner callback of [`MemoryIndex::reconcile`], or the cache file's
    /// directory.
    Io(std::io::Error),
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
            Self::Sqlite(e) => write!(f, "memory index database: {e}"),
            Self::Io(e) => write!(f, "memory index io: {e}"),
        }
    }
}

impl std::error::Error for IndexError {}

impl IndexError {
    /// Whether the cache file itself is damaged — SQLite's `SQLITE_CORRUPT`
    /// (FTS5's `SQLITE_CORRUPT_VTAB` included) or `SQLITE_NOTADB`. Open no
    /// longer runs `PRAGMA quick_check` (scale slice S6: 2.4 s at every boot
    /// at 100K notes), so damage below the catalog surfaces on use, and a
    /// caller that meets this rebuilds the cache.
    pub fn is_corruption(&self) -> bool {
        matches!(
            self,
            Self::Sqlite(rusqlite::Error::SqliteFailure(e, _))
                if matches!(
                    e.code,
                    rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
                )
        )
    }
}

impl From<rusqlite::Error> for IndexError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<std::io::Error> for IndexError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, IndexError>;

// ============================================================================
// Input and catalog types
// ============================================================================

/// The stat triple the reconciliation scan compares (design §5, *Freshness
/// without a watcher*): mtime + size + inode. A rewrite with identical bytes
/// that keeps all three is invisible to the scan by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileStat {
    /// Modification time in nanoseconds since the Unix epoch.
    pub mtime_ns: i64,
    pub size: u64,
    /// The inode (or the platform's file id); `0` where none is available.
    pub inode: u64,
}

/// One file the partition scanner saw under `notes/`, by its path relative to
/// the partition directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanEntry {
    pub relative_path: String,
    pub stat: FileStat,
}

/// A note's status after `memory_files` has applied design §6's derived
/// supersession. `Superseded` is derived, never stored (§4); the stored
/// `NoteStatus` travels beside it in [`IndexRecord::status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectiveStatus {
    Active,
    Superseded,
    Archived,
    Candidate,
}

impl EffectiveStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Archived => "archived",
            Self::Candidate => "candidate",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "superseded" => Some(Self::Superseded),
            "archived" => Some(Self::Archived),
            "candidate" => Some(Self::Candidate),
            _ => None,
        }
    }
}

/// One note as the slice-3 service hands it to the index — a plain struct
/// with **no** dependency on `memory_files`. Every filter field is
/// pre-computed by the caller; the index applies and reports them.
///
/// Timestamps are RFC 3339; the index re-formats them to the canon (`Z`, whole
/// seconds) so that string comparison is chronological, and refuses one it
/// cannot parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRecord {
    /// The note's UUID (hyphenated lowercase).
    pub id: String,
    /// The revision the caller read the file at — 64 lowercase hex.
    pub revision: String,
    /// `home-<owner-key>` or `proj-<owner-key>-<dir-key>` (design §2).
    pub partition_key: String,
    /// `global` or `project` — reported on every hit.
    pub scope: String,
    /// The note file's path relative to the partition directory
    /// (`notes/<uuid>--<slug>.md`).
    pub relative_path: String,
    pub title: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    /// `NoteKind` as a string.
    pub kind: String,
    /// The stored `NoteStatus`: `active`, `candidate` or `archived`.
    pub status: String,
    pub effective_status: EffectiveStatus,
    /// `Origin` as a string; authority ranking reads it (design §4).
    pub origin: String,
    pub fact_key: Option<String>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub updated_at: String,
    /// When the note was first written; no edit moves it.
    pub created_at: String,
    /// When the evidence was observed, if the note says (design §4). The
    /// note's date is this, else `created_at` (design 09 §13.1).
    pub observed_at: Option<String>,
    pub pinned: bool,
    /// Derived at read time by `memory_files`: a source it cites was forgotten.
    /// Excluded from the automatic path (design §6, *Forget contract*).
    pub source_forgotten: bool,
    /// Reported on the hit: the note carries an un-honored `supersedes` link,
    /// or is a same-`fact_key` loser (design §3 rule 3, §6).
    pub conflict: bool,
    /// The same-`fact_key` loser: returned by an explicit search, excluded
    /// from the automatic path (design §3 rule 3, §5 step 5).
    pub same_key_loser: bool,
    /// Ids of the notes and tombstones whose honored link supersedes this one.
    pub superseded_by: Vec<String>,
    /// The ids the note's own frontmatter lists in `supersedes` (design §4),
    /// whether or not derivation honours the link — a candidate's link
    /// counts for nothing until it is confirmed, and is still listed here.
    pub supersedes: Vec<String>,
    /// The note's `sources` as suppression keys, `message:<id>` or
    /// `task:<id>` (design §4) — the `note_source` rows learning's lookup
    /// finds this window's notes through (design 09 §2.3).
    pub sources: Vec<String>,
    /// The Markdown body below the frontmatter, as read.
    pub body: String,
    /// How many file lines precede the body's first line, so hit line ranges
    /// are file lines. `0` when the body is the whole text.
    pub body_line_offset: u32,
    /// The file's stat at the time the caller read it.
    pub file: FileStat,
}

/// A catalog row as the index holds it — what a caller needs to decide whether
/// it must re-read a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedNote {
    pub id: String,
    pub partition_key: String,
    pub revision: String,
    pub relative_path: String,
    pub effective_status: EffectiveStatus,
    pub pinned: bool,
    pub file: FileStat,
    pub chunks: u32,
    /// The note's window has not begun by the index clock's "now" (design 09
    /// §6.1) — the clock `list_pinned`'s validity filter reads.
    pub upcoming: bool,
}

/// Whether the index can be served from (design §5, *Freshness without a
/// watcher*): a fresh, recreated or mid-rebuild index is `Rebuilding` until a
/// [`MemoryIndex::rebuild`] completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexState {
    Ready,
    Rebuilding,
}

impl IndexState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Rebuilding => "rebuilding",
        }
    }
}

/// What [`MemoryIndex::index_state`] reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexStateReport {
    pub state: IndexState,
    pub schema_version: i64,
    pub notes: u64,
    pub chunks: u64,
    pub partitions: u64,
    /// `None` for an in-memory index.
    pub path: Option<PathBuf>,
}

/// What a reconciliation scan found (design §5): the caller re-indexes
/// `added` and `changed` through [`MemoryIndex::upsert`] and drops `removed`
/// through [`MemoryIndex::remove_path`]. The index changes nothing itself,
/// because only `memory_files` can re-hash and re-parse a file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconcileReport {
    pub partition_key: String,
    /// Files the scanner listed.
    pub scanned: usize,
    /// Paths on disk the index has no row for.
    pub added: Vec<String>,
    /// `(id, relative_path)` of rows whose stat differs from the file's.
    pub changed: Vec<(String, String)>,
    /// `(id, relative_path)` of rows whose file the scanner did not list.
    pub removed: Vec<(String, String)>,
}

impl ReconcileReport {
    pub fn is_clean(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

// ============================================================================
// The index
// ============================================================================

/// What "now" is to the index: the instant a search without `as_of`
/// evaluates validity at, and the time a stat scan is recorded at.
type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// The cache database. One connection behind a mutex, like [`crate::Database`].
pub struct MemoryIndex {
    conn: Mutex<Connection>,
    path: Option<PathBuf>,
    clock: Clock,
    /// Row writes since open — what tells an idle index from a busy one
    /// (scale slice S5).
    writes: AtomicU64,
}

/// What one maintenance pass found and did (scale slice S5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Maintenance {
    /// FTS merge steps that did work.
    pub merge_steps: usize,
    /// The file's size now (pages × page size), the WAL folded in.
    pub bytes: u64,
    /// Its size right after the last whole rebuild — the yardstick a
    /// caller compares `bytes` with. A cache written before this was
    /// recorded takes its size now.
    pub rebuilt_bytes: u64,
}

impl fmt::Debug for MemoryIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryIndex")
            .field("path", &self.path)
            .finish()
    }
}

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS partition (
    partition_key   TEXT PRIMARY KEY,
    last_scan_at    TEXT,
    last_scan_files INTEGER
);
CREATE TABLE IF NOT EXISTS note (
    rowid            INTEGER PRIMARY KEY,
    partition_key    TEXT NOT NULL,
    id               TEXT NOT NULL,
    scope            TEXT NOT NULL,
    revision         TEXT NOT NULL,
    relative_path    TEXT NOT NULL,
    title            TEXT NOT NULL,
    norm_title       TEXT NOT NULL,
    aliases          TEXT NOT NULL,
    tags             TEXT NOT NULL,
    norm_aliases     TEXT NOT NULL,
    norm_tags        TEXT NOT NULL,
    kind             TEXT NOT NULL,
    status           TEXT NOT NULL,
    effective_status TEXT NOT NULL,
    origin           TEXT NOT NULL,
    authority        INTEGER NOT NULL,
    fact_key         TEXT,
    valid_from       TEXT,
    valid_to         TEXT,
    updated_at       TEXT NOT NULL,
    created_at       TEXT NOT NULL,
    observed_at      TEXT,
    date             TEXT NOT NULL,
    pinned           INTEGER NOT NULL,
    source_forgotten INTEGER NOT NULL,
    conflict         INTEGER NOT NULL,
    same_key_loser   INTEGER NOT NULL,
    superseded_by    TEXT NOT NULL,
    supersedes       TEXT NOT NULL,
    body_line_offset INTEGER NOT NULL,
    mtime_ns         INTEGER NOT NULL,
    size             INTEGER NOT NULL,
    inode            INTEGER NOT NULL,
    UNIQUE (partition_key, id),
    UNIQUE (partition_key, relative_path)
);
CREATE INDEX IF NOT EXISTS note_partition_title ON note(partition_key, norm_title);
CREATE INDEX IF NOT EXISTS note_partition_fact_key ON note(partition_key, fact_key);
CREATE INDEX IF NOT EXISTS note_partition_pinned ON note(partition_key, pinned);
CREATE INDEX IF NOT EXISTS note_partition_date ON note(partition_key, date, id);
CREATE TABLE IF NOT EXISTS note_alias (
    note_rowid INTEGER NOT NULL REFERENCES note(rowid) ON DELETE CASCADE,
    norm_alias TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS note_alias_norm ON note_alias(norm_alias, note_rowid);
CREATE INDEX IF NOT EXISTS note_alias_note ON note_alias(note_rowid);
CREATE TABLE IF NOT EXISTS note_source (
    note_rowid INTEGER NOT NULL REFERENCES note(rowid) ON DELETE CASCADE,
    source_key TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS note_source_key ON note_source(source_key, note_rowid);
CREATE INDEX IF NOT EXISTS note_source_note ON note_source(note_rowid);
CREATE TABLE IF NOT EXISTS chunk (
    id            INTEGER PRIMARY KEY,
    note_rowid    INTEGER NOT NULL REFERENCES note(rowid) ON DELETE CASCADE,
    partition_key TEXT NOT NULL,
    ordinal       INTEGER NOT NULL,
    heading       TEXT,
    line_from     INTEGER NOT NULL,
    line_to       INTEGER NOT NULL,
    body          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS chunk_note ON chunk(note_rowid, ordinal);
CREATE INDEX IF NOT EXISTS chunk_partition ON chunk(partition_key, id);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_word USING fts5(
    title, aliases, tags, body,
    content='', contentless_delete=1,
    tokenize='unicode61 remove_diacritics 2'
);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_tri_head USING fts5(
    title, aliases, tags,
    content='', contentless_delete=1,
    tokenize='trigram'
);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_tri USING fts5(
    body,
    content='', contentless_delete=1,
    tokenize='trigram'
);
CREATE TRIGGER IF NOT EXISTS chunk_ad AFTER DELETE ON chunk BEGIN
    DELETE FROM fts_word WHERE rowid = old.id;
    DELETE FROM fts_tri_head WHERE rowid = old.id;
    DELETE FROM fts_tri WHERE rowid = old.id;
END;
"#;

/// BM25 column weights: title, aliases, tags, body. Pinned by
/// `search::tests`; not claimed optimal (design §5 step 6).
const RANK_WEIGHTS: &str = "bm25(4.0, 3.0, 1.5, 1.0)";
/// The same weights for the header trigram table: title, aliases, tags.
/// `fts_tri`, one column, ranks with the default `bm25()`.
const HEAD_RANK_WEIGHTS: &str = "bm25(4.0, 3.0, 1.5)";
/// The full-text tables, for the passes that treat them alike: `optimize`
/// after a rebuild or a replace, and the maintenance `merge`.
const FTS_TABLES: [&str; 3] = ["fts_word", "fts_tri_head", "fts_tri"];

const META_SCHEMA: &str = "schema_version";
const META_STATE: &str = "state";
/// The file's size right after the last rebuild (scale slice S5).
const META_REBUILT_BYTES: &str = "rebuilt_bytes";
/// Pages one FTS `merge` step may write before it yields. Sent negated:
/// a positive `merge` only merges a level holding `usermerge` (4) segments,
/// which FTS5's own automerge never lets build up, so it found no work; a
/// negative one merges every segment into one, as few as two at a time.
const MERGE_PAGES: i64 = 256;
/// The WAL is truncated to this after a checkpoint, whatever one large
/// transaction grew it to (scale slice S5).
const JOURNAL_SIZE_LIMIT: i64 = 32 * 1024 * 1024;

/// The tables an intact cache has. A file missing any is recreated.
const EXPECTED_TABLES: &[&str] = &[
    "meta",
    "partition",
    "note",
    "note_alias",
    "note_source",
    "chunk",
    "fts_word",
    "fts_tri_head",
    "fts_tri",
];

impl MemoryIndex {
    /// `state/cache/memory-index-v1.sqlite` under the home store (design §2).
    /// Creates `state/` (0700) like every other `state/` accessor; the
    /// `cache/` directory is created by [`MemoryIndex::open`].
    pub fn default_path() -> anyhow::Result<PathBuf> {
        Ok(crate::store::cache_dir()?.join(INDEX_FILE_NAME))
    }

    /// Opens the cache at `path`, creating it when absent. A file that is
    /// not a database, carries another [`SCHEMA_VERSION`] or lacks a table
    /// is deleted and recreated — it is a cache — and the fresh index
    /// reports [`IndexState::Rebuilding`] until [`MemoryIndex::rebuild`]
    /// runs. An intact file keeps the state it recorded. No integrity check
    /// runs here (scale slice S6: `quick_check` read the whole file at every
    /// boot); damage below the catalog is met on use and reported by
    /// [`IndexError::is_corruption`].
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if path.exists() {
            match Self::open_existing(path) {
                Ok(index) => return Ok(index),
                Err(reason) => {
                    tracing::warn!(
                        path = %path.display(),
                        %reason,
                        "memory index cache is unusable; recreating it"
                    );
                    delete_cache_files(path)?;
                }
            }
        }
        let conn = Connection::open(path)?;
        apply_pragmas(&conn)?;
        create_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            path: Some(path.to_path_buf()),
            clock: Arc::new(Utc::now),
            writes: AtomicU64::new(0),
        })
    }

    /// A private in-memory index — tests, and the bounded fallback nothing
    /// persists.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        apply_pragmas(&conn)?;
        create_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            path: None,
            clock: Arc::new(Utc::now),
            writes: AtomicU64::new(0),
        })
    }

    fn open_existing(path: &Path) -> std::result::Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        apply_pragmas(&conn).map_err(|e| e.to_string())?;
        for table in EXPECTED_TABLES {
            let present: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1)",
                    params![table],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !present {
                return Err(format!("table {table} is missing"));
            }
        }
        let version: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![META_SCHEMA],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        match version.as_deref().and_then(|v| v.parse::<i64>().ok()) {
            Some(v) if v == SCHEMA_VERSION => {}
            other => {
                return Err(format!(
                    "schema version {other:?}, this build writes {SCHEMA_VERSION}"
                ));
            }
        }
        Ok(Self {
            conn: Mutex::new(conn),
            path: Some(path.to_path_buf()),
            clock: Arc::new(Utc::now),
            writes: AtomicU64::new(0),
        })
    }

    /// The path this index was opened at; `None` in memory.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Replaces the clock (default [`Utc::now`]) — tests pin "now" the way
    /// `memory_files::Partition::with_clock` does.
    pub fn with_clock(mut self, clock: impl Fn() -> DateTime<Utc> + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("memory index mutex was poisoned; recovering");
            poisoned.into_inner()
        })
    }

    /// The state alone — one row of `meta`, where [`Self::index_state`]
    /// also counts every note and chunk.
    pub fn state(&self) -> Result<IndexState> {
        let conn = self.lock();
        Ok(read_state(&conn)?)
    }

    /// The state and the row counts.
    pub fn index_state(&self) -> Result<IndexStateReport> {
        let conn = self.lock();
        let state = read_state(&conn)?;
        let notes: i64 = conn.query_row("SELECT COUNT(*) FROM note", [], |r| r.get(0))?;
        let chunks: i64 = conn.query_row("SELECT COUNT(*) FROM chunk", [], |r| r.get(0))?;
        let partitions: i64 =
            conn.query_row("SELECT COUNT(DISTINCT partition_key) FROM note", [], |r| {
                r.get(0)
            })?;
        Ok(IndexStateReport {
            state,
            schema_version: SCHEMA_VERSION,
            notes: notes as u64,
            chunks: chunks as u64,
            partitions: partitions as u64,
            path: self.path.clone(),
        })
    }
}

// ============================================================================
// Catalog writes
// ============================================================================

/// What [`MemoryIndex::rebuild`] and [`MemoryIndex::replace_partition`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RebuildReport {
    pub notes: usize,
    pub chunks: usize,
}

impl MemoryIndex {
    /// Indexes one note, replacing any row of the same `(partition_key, id)`
    /// or the same `(partition_key, relative_path)` — a retitle moves the id
    /// to its new path, a file rewritten under a new id replaces the old row.
    /// Synchronous, so a service write is searchable when this returns
    /// (design §5, *Freshness without a watcher*).
    pub fn upsert(&self, record: &IndexRecord) -> Result<usize> {
        self.wrote();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let chunks = upsert_in(&tx, record)?;
        tx.commit()?;
        Ok(chunks)
    }

    /// [`MemoryIndex::upsert`] for many notes in one transaction. All or none.
    pub fn upsert_many(&self, records: &[IndexRecord]) -> Result<RebuildReport> {
        self.wrote();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut report = RebuildReport::default();
        for record in records {
            report.chunks += upsert_in(&tx, record)?;
            report.notes += 1;
        }
        tx.commit()?;
        Ok(report)
    }

    /// Drops everything and indexes `records`, then marks the index `ready`:
    /// [`Self::begin_rebuild`], the records in batches, then
    /// [`Self::finish_rebuild`]. A crash inside leaves an index that says
    /// `rebuilding`.
    pub fn rebuild(&self, records: &[IndexRecord]) -> Result<RebuildReport> {
        self.begin_rebuild()?;
        let mut report = RebuildReport::default();
        for batch in records.chunks(REBUILD_BATCH) {
            let done = self.upsert_many(batch)?;
            report.notes += done.notes;
            report.chunks += done.chunks;
        }
        self.finish_rebuild()?;
        Ok(report)
    }

    /// Starts a whole rebuild: the cache is recreated empty — a new file at
    /// the same path, or a new in-memory database — and says `rebuilding`,
    /// so a crash before [`Self::finish_rebuild`] leaves an index that says
    /// so. Recreating rather than deleting rows leaves no free pages behind
    /// and never reads a damaged page (scale slice S6). The caller then
    /// indexes the notes in batches — each its own transaction, so the
    /// index is never held for the whole rebuild — and finishes.
    pub fn begin_rebuild(&self) -> Result<()> {
        let mut conn = self.lock();
        let fresh = match &self.path {
            Some(path) => {
                // Close this file before it is deleted, so nothing of it is
                // checkpointed into the new one.
                drop(std::mem::replace(&mut *conn, Connection::open_in_memory()?));
                delete_cache_files(path)?;
                Connection::open(path)?
            }
            None => Connection::open_in_memory()?,
        };
        apply_pragmas(&fresh)?;
        create_schema(&fresh)?;
        *conn = fresh;
        Ok(())
    }

    /// Ends a whole rebuild: one merged FTS segment per table, the state
    /// `ready`, and the WAL folded back into the file.
    pub fn finish_rebuild(&self) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        // A bulk load leaves each FTS index as several segments; one merged
        // segment per index makes every later term lookup one doclist read.
        optimize_fts(&tx)?;
        write_state(&tx, IndexState::Ready)?;
        // The size the cache is judged against until the next rebuild
        // (scale slice S5): the database's pages, the WAL's included.
        let bytes = file_bytes(&tx)?;
        write_rebuilt_bytes(&tx, bytes)?;
        tx.commit()?;
        // Fold the WAL back into the file: a rebuild's WAL is the whole index.
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    fn wrote(&self) {
        self.writes.fetch_add(1, Ordering::Relaxed);
    }

    /// Row writes since open (scale slice S5): a caller that sees the same
    /// number twice, a while apart, knows the index sat idle between.
    pub fn writes(&self) -> u64 {
        self.writes.load(Ordering::Relaxed)
    }

    /// Maintenance for an index at rest (scale slice S5, M1): FTS `merge`
    /// steps — each bounded, the index released between them — until
    /// neither table has merge work left or `budget` is spent, then the WAL
    /// folded back into the file and truncated. Reports the file's size and
    /// its size after the last rebuild, so the caller can tell a cache that
    /// has only grown with its notes from one that has bloated. Changes no
    /// row.
    pub fn maintain(&self, budget: Duration) -> Result<Maintenance> {
        let started = Instant::now();
        let mut merge_steps = 0;
        loop {
            let mut worked = false;
            {
                let conn = self.lock();
                for table in FTS_TABLES {
                    let before = conn.total_changes();
                    conn.execute(
                        &format!(
                            "INSERT INTO {table}({table}, rank) VALUES ('merge', -{MERGE_PAGES})"
                        ),
                        [],
                    )?;
                    // FTS5: a delta under two means the step did no work.
                    if conn.total_changes() - before >= 2 {
                        worked = true;
                        merge_steps += 1;
                    }
                }
            }
            if !worked || started.elapsed() >= budget {
                break;
            }
        }
        let conn = self.lock();
        let bytes = file_bytes(&conn)?;
        let rebuilt_bytes = match read_rebuilt_bytes(&conn)? {
            Some(b) => b,
            None => {
                write_rebuilt_bytes(&conn, bytes)?;
                bytes
            }
        };
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(Maintenance {
            merge_steps,
            bytes,
            rebuilt_bytes,
        })
    }

    /// Replaces one partition's rows with `records`, every one of which must
    /// name that partition. Leaves the index state alone. Ends as a rebuild
    /// ends (scale slice S5): its FTS indexes merged and the WAL folded
    /// back and truncated — a whole partition's writes left both behind.
    /// No service path calls it since scale slice S1 (a partition is
    /// re-indexed note by note, and wholly only by a rebuild); the index's
    /// own tests do.
    pub fn replace_partition(
        &self,
        partition_key: &str,
        records: &[IndexRecord],
    ) -> Result<RebuildReport> {
        if let Some(stray) = records.iter().find(|r| r.partition_key != partition_key) {
            return Err(IndexError::InvalidInput(format!(
                "record {} names partition {:?}, not {partition_key:?}",
                stray.id, stray.partition_key
            )));
        }
        self.wrote();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        delete_partition_rows(&tx, partition_key)?;
        let mut report = RebuildReport::default();
        for record in records {
            report.chunks += upsert_in(&tx, record)?;
            report.notes += 1;
        }
        tx.commit()?;
        optimize_fts(&conn)?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(report)
    }

    /// Removes one note by id. `Ok(false)` when there was no such row.
    pub fn remove(&self, partition_key: &str, id: &str) -> Result<bool> {
        self.wrote();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let rowid: Option<i64> = tx
            .query_row(
                "SELECT rowid FROM note WHERE partition_key = ?1 AND id = ?2",
                params![partition_key, id],
                |r| r.get(0),
            )
            .optional()?;
        let removed = match rowid {
            Some(rowid) => {
                delete_note_rows(&tx, rowid)?;
                true
            }
            None => false,
        };
        tx.commit()?;
        Ok(removed)
    }

    /// Removes the note indexed at `relative_path`. The verb for a
    /// reconciliation's `removed` entry: a retitle that already moved the id
    /// to its new path finds no row here and is a no-op, so the new row
    /// survives.
    pub fn remove_path(&self, partition_key: &str, relative_path: &str) -> Result<bool> {
        self.wrote();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let rowid: Option<i64> = tx
            .query_row(
                "SELECT rowid FROM note WHERE partition_key = ?1 AND relative_path = ?2",
                params![partition_key, relative_path],
                |r| r.get(0),
            )
            .optional()?;
        let removed = match rowid {
            Some(rowid) => {
                delete_note_rows(&tx, rowid)?;
                true
            }
            None => false,
        };
        tx.commit()?;
        Ok(removed)
    }

    /// Removes every row of one partition (a purge, a workspace move's
    /// invalidation). Returns how many notes went.
    pub fn remove_partition(&self, partition_key: &str) -> Result<u64> {
        self.wrote();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let removed = delete_partition_rows(&tx, partition_key)?;
        tx.commit()?;
        Ok(removed)
    }

    /// The catalog row of one note, if indexed.
    pub fn get(&self, partition_key: &str, id: &str) -> Result<Option<IndexedNote>> {
        let now = canon_timestamp(&self.now());
        let conn = self.lock();
        conn.query_row(
            "SELECT n.id, n.partition_key, n.revision, n.relative_path, n.effective_status,
                    n.pinned, n.mtime_ns, n.size, n.inode,
                    (SELECT COUNT(*) FROM chunk c WHERE c.note_rowid = n.rowid),
                    n.valid_from IS NOT NULL AND n.valid_from > ?3
             FROM note n WHERE n.partition_key = ?1 AND n.id = ?2",
            params![partition_key, id, now],
            indexed_note_from_row,
        )
        .optional()
        .map_err(IndexError::from)
    }

    /// The active pinned notes of these partitions, newest `updated_at`
    /// first — design §5 step 2's selection, for the caller to budget. A
    /// note whose validity window has ended by the index clock's "now" is
    /// left out, as a current search leaves it out (design §4 *Times*); one
    /// whose window has not begun by that "now" is kept and says `upcoming`.
    pub fn list_pinned(&self, partition_keys: &[String]) -> Result<Vec<IndexedNote>> {
        if partition_keys.is_empty() {
            return Ok(Vec::new());
        }
        let now = canon_timestamp(&self.now());
        let conn = self.lock();
        let placeholders = placeholders(partition_keys.len());
        let now_param = partition_keys.len() + 1;
        let sql = format!(
            "SELECT n.id, n.partition_key, n.revision, n.relative_path, n.effective_status,
                    n.pinned, n.mtime_ns, n.size, n.inode,
                    (SELECT COUNT(*) FROM chunk c WHERE c.note_rowid = n.rowid),
                    n.valid_from IS NOT NULL AND n.valid_from > ?{now_param}
             FROM note n
             WHERE n.pinned = 1 AND n.effective_status = 'active'
               AND n.partition_key IN ({placeholders})
               AND (n.valid_to IS NULL OR n.valid_to > ?{now_param})
             ORDER BY n.updated_at DESC, n.id ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter(partition_keys.iter().chain(std::iter::once(&now))),
            indexed_note_from_row,
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(IndexError::from)
    }

    /// The distinct `fact_key`s of the effectively active notes of these
    /// partitions, the most recently updated first, at most `limit` —
    /// learning's `known_keys` (design 09 §2.3 step 4), read from the
    /// catalog rather than a partition parse. A conversation summary holds
    /// no key for learning (design 09 §13.3): one a model's edit put on it
    /// is not listed. Nor is a key only notes the chat model wrote ever
    /// held (owner decision LQ14, [`LEARNING_KEY_HOLDER`]): a key a learned
    /// or saved note in these partitions holds or held — superseded or
    /// archived since — is listed, whoever holds it now.
    pub fn fact_keys(&self, partition_keys: &[String], limit: usize) -> Result<Vec<String>> {
        if partition_keys.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let conn = self.lock();
        let partitions = placeholders(partition_keys.len());
        let limit_param = partition_keys.len() + 1;
        let sql = format!(
            "SELECT fact_key, MAX(updated_at) AS newest FROM note
             WHERE partition_key IN ({partitions}) AND effective_status = 'active'
               AND fact_key IS NOT NULL
               AND NOT (kind = 'episode' AND origin = 'extracted_other')
               AND fact_key IN (SELECT fact_key FROM note
                                WHERE partition_key IN ({partitions})
                                  AND fact_key IS NOT NULL AND {LEARNING_KEY_HOLDER})
             GROUP BY fact_key
             ORDER BY newest DESC, fact_key ASC
             LIMIT ?{limit_param}"
        );
        let mut stmt = conn.prepare(&sql)?;
        let limit = limit as i64;
        let params: Vec<&dyn rusqlite::ToSql> = partition_keys
            .iter()
            .map(|k| k as &dyn rusqlite::ToSql)
            .chain(std::iter::once(&limit as &dyn rusqlite::ToSql))
            .collect();
        let rows = stmt.query_map(params.as_slice(), |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(IndexError::from)
    }

    /// Which of `fact_keys` are learning's in these partitions (owner
    /// decision LQ14, [`LEARNING_KEY_HOLDER`]): a note of theirs other than
    /// a conversation summary and a note only the chat model wrote holds or
    /// held it, whatever its status now.
    pub fn learning_keys_among(
        &self,
        partition_keys: &[String],
        fact_keys: &[String],
    ) -> Result<std::collections::HashSet<String>> {
        if partition_keys.is_empty() || fact_keys.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        let conn = self.lock();
        let keys_from = partition_keys.len() + 1;
        let sql = format!(
            "SELECT DISTINCT fact_key FROM note
             WHERE partition_key IN ({}) AND fact_key IN ({})
               AND {LEARNING_KEY_HOLDER}",
            placeholders(partition_keys.len()),
            (keys_from..keys_from + fact_keys.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = partition_keys
            .iter()
            .chain(fact_keys.iter())
            .map(|k| k as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(IndexError::from)
    }

    /// The notes of these partitions whose `sources` cite any of
    /// `source_keys` (`message:<id>`, `task:<id>`), whatever their status —
    /// the caller filters (design 09 §2.3 step 2). Ordered by partition,
    /// then id.
    pub fn notes_citing(
        &self,
        partition_keys: &[String],
        source_keys: &[String],
    ) -> Result<Vec<CatalogNote>> {
        if partition_keys.is_empty() || source_keys.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock();
        let mut out: Vec<CatalogNote> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for batch in source_keys.chunks(200) {
            let sources = (partition_keys.len() + 1..=partition_keys.len() + batch.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT {CATALOG_COLUMNS} FROM note n
                 WHERE n.partition_key IN ({})
                   AND n.rowid IN (SELECT s.note_rowid FROM note_source s
                                   WHERE s.source_key IN ({sources}))
                 ORDER BY n.partition_key, n.id",
                placeholders(partition_keys.len())
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(
                rusqlite::params_from_iter(partition_keys.iter().chain(batch.iter())),
                catalog_note_from_row,
            )?;
            for row in rows {
                let row = row?;
                if seen.insert((row.partition_key.clone(), row.id.clone())) {
                    out.push(row);
                }
            }
        }
        out.sort_by(|a, b| {
            a.partition_key
                .cmp(&b.partition_key)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    /// The pending suggestions of these partitions (design 09 §3.5, LR18):
    /// every effective candidate whose own `supersedes` names a note, newest
    /// `updated_at` first — read from the catalog, never a partition parse.
    /// Both origins; the caller verifies each file it lists.
    pub fn suggestion_rows(&self, partition_keys: &[String]) -> Result<Vec<CatalogNote>> {
        if partition_keys.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock();
        let sql = format!(
            "SELECT {CATALOG_COLUMNS} FROM note n
             WHERE n.partition_key IN ({})
               AND n.effective_status = 'candidate' AND n.supersedes <> '[]'
             ORDER BY n.updated_at DESC, n.id ASC",
            placeholders(partition_keys.len())
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter(partition_keys.iter()),
            catalog_note_from_row,
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(IndexError::from)
    }

    /// One note's catalog row, whatever its status.
    pub fn catalog_note(&self, partition_key: &str, id: &str) -> Result<Option<CatalogNote>> {
        let conn = self.lock();
        conn.query_row(
            &format!(
                "SELECT {CATALOG_COLUMNS} FROM note n WHERE n.partition_key = ?1 AND n.id = ?2"
            ),
            params![partition_key, id],
            catalog_note_from_row,
        )
        .optional()
        .map_err(IndexError::from)
    }

    /// The winner of `fact_key` in one partition (design 03 §3 rule 3, §7c
    /// R9): the effectively active note with that key that is not a
    /// same-key loser. Derivation leaves at most one such note per key and
    /// partition. A conversation summary is never learning's target (design
    /// 09 §13.3): a key only a summary holds — a model's edit put it there —
    /// has no winner here; nor has a key only notes the chat model wrote
    /// ever held in `holders` — the partitions learning reads together
    /// (owner decision LQ14, [`LEARNING_KEY_HOLDER`]). A key a learned or a
    /// saved note in any of them holds or held has its winner here, whoever
    /// wrote that.
    pub fn fact_key_winner(
        &self,
        partition_key: &str,
        fact_key: &str,
        holders: &[String],
    ) -> Result<Option<CatalogNote>> {
        if holders.is_empty() {
            return Ok(None);
        }
        let conn = self.lock();
        let held = (3..3 + holders.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let params: Vec<&dyn rusqlite::ToSql> = [
            &partition_key as &dyn rusqlite::ToSql,
            &fact_key as &dyn rusqlite::ToSql,
        ]
        .into_iter()
        .chain(holders.iter().map(|k| k as &dyn rusqlite::ToSql))
        .collect();
        conn.query_row(
            &format!(
                "SELECT {CATALOG_COLUMNS} FROM note n
                 WHERE n.partition_key = ?1 AND n.fact_key = ?2
                   AND n.effective_status = 'active' AND n.same_key_loser = 0
                   AND NOT (n.kind = 'episode' AND n.origin = 'extracted_other')
                   AND EXISTS (SELECT 1 FROM note
                               WHERE partition_key IN ({held}) AND fact_key = ?2
                                 AND {LEARNING_KEY_HOLDER})
                 ORDER BY n.updated_at DESC, n.id ASC
                 LIMIT 1"
            ),
            params.as_slice(),
            catalog_note_from_row,
        )
        .optional()
        .map_err(IndexError::from)
    }
}

/// A note whose `fact_key` makes it a key background learning is offered
/// (owner decision LQ14, 2026-09-26), as an SQL condition on a `note` row,
/// whatever its status: any note but a conversation summary (design 09
/// §13.3) and a note only the chat model's `memory_store` wrote. A key is
/// learning's when any note of the partitions learning reads together
/// holds or held it so — superseded or archived since, too (repair round 1
/// of the L8 product slice) — so the chat model's replacement of a learned
/// note keeps its key learning's, and a key the user saved at home is
/// learning's in the project too. A note only the chat model wrote is
/// origin `extracted_user` with no `observed_at`: `observed_at` is set by the
/// extractor alone — the service refuses it on every other surface, and an
/// edit's patch cannot carry it — and every note the extractor writes from
/// the user's words has one, the time of its newest cited row. So a key the
/// chat model coined, and only it holds, is not offered to the extractor to
/// reuse or to replace by; a key learning or the user also holds still is.
/// The note itself stays searched, recalled and listed.
pub const LEARNING_KEY_HOLDER: &str = "NOT (kind = 'episode' AND origin = 'extracted_other') \
     AND NOT (origin = 'extracted_user' AND observed_at IS NULL)";

/// The facts a note's row was built from, as the row holds them (scale
/// slice S1): what a caller compares with the note on disk to decide
/// whether the row must be written again — so a service that has just
/// started re-indexes only what changed, never a whole partition. Every
/// field is a `note` column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowFacts {
    pub id: String,
    pub revision: String,
    pub relative_path: String,
    pub effective_status: Option<EffectiveStatus>,
    pub origin: String,
    pub conflict: bool,
    pub same_key_loser: bool,
    pub source_forgotten: bool,
    pub superseded_by: Vec<String>,
    pub pinned: bool,
    pub file: FileStat,
}

impl MemoryIndex {
    /// Every row of one partition, as [`RowFacts`].
    pub fn row_facts(&self, partition_key: &str) -> Result<Vec<RowFacts>> {
        let conn = self.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT id, revision, relative_path, effective_status, origin, conflict,
                    same_key_loser, source_forgotten, superseded_by, pinned,
                    mtime_ns, size, inode
             FROM note WHERE partition_key = ?1",
        )?;
        let rows = stmt.query_map(params![partition_key], |r| {
            let status: String = r.get(3)?;
            let superseded: String = r.get(8)?;
            let size: i64 = r.get(11)?;
            let inode: i64 = r.get(12)?;
            Ok(RowFacts {
                id: r.get(0)?,
                revision: r.get(1)?,
                relative_path: r.get(2)?,
                effective_status: EffectiveStatus::parse(&status),
                origin: r.get(4)?,
                conflict: r.get::<_, i64>(5)? != 0,
                same_key_loser: r.get::<_, i64>(6)? != 0,
                source_forgotten: r.get::<_, i64>(7)? != 0,
                superseded_by: serde_json::from_str(&superseded).unwrap_or_default(),
                pinned: r.get::<_, i64>(9)? != 0,
                file: FileStat {
                    mtime_ns: r.get(10)?,
                    size: size as u64,
                    inode: inode as u64,
                },
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(IndexError::from)
    }
}

/// A catalog row with what learning's lookup reads (design 09 §2.3): the
/// note's identity and filter fields, its own `supersedes` list and its
/// title — no body and no chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogNote {
    pub id: String,
    pub partition_key: String,
    pub revision: String,
    pub relative_path: String,
    pub title: String,
    pub kind: String,
    /// The stored `NoteStatus`.
    pub status: String,
    pub effective_status: EffectiveStatus,
    pub origin: String,
    pub fact_key: Option<String>,
    pub same_key_loser: bool,
    pub supersedes: Vec<String>,
    pub updated_at: String,
}

const CATALOG_COLUMNS: &str = "n.id, n.partition_key, n.revision, n.relative_path, n.title, \
     n.kind, n.status, n.effective_status, n.origin, n.fact_key, n.same_key_loser, \
     n.supersedes, n.updated_at";

fn catalog_note_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogNote> {
    let effective: String = row.get(7)?;
    let supersedes: String = row.get(11)?;
    Ok(CatalogNote {
        id: row.get(0)?,
        partition_key: row.get(1)?,
        revision: row.get(2)?,
        relative_path: row.get(3)?,
        title: row.get(4)?,
        kind: row.get(5)?,
        status: row.get(6)?,
        effective_status: EffectiveStatus::parse(&effective).unwrap_or(EffectiveStatus::Active),
        origin: row.get(8)?,
        fact_key: row.get(9)?,
        same_key_loser: row.get::<_, i64>(10)? != 0,
        supersedes: serde_json::from_str(&supersedes).unwrap_or_default(),
        updated_at: row.get(12)?,
    })
}

fn indexed_note_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IndexedNote> {
    let status: String = row.get(4)?;
    let mtime_ns: i64 = row.get(6)?;
    let size: i64 = row.get(7)?;
    let inode: i64 = row.get(8)?;
    let chunks: i64 = row.get(9)?;
    let upcoming: bool = row.get(10)?;
    Ok(IndexedNote {
        id: row.get(0)?,
        partition_key: row.get(1)?,
        revision: row.get(2)?,
        relative_path: row.get(3)?,
        effective_status: EffectiveStatus::parse(&status).unwrap_or(EffectiveStatus::Active),
        pinned: row.get::<_, i64>(5)? != 0,
        file: FileStat {
            mtime_ns,
            size: size as u64,
            inode: inode as u64,
        },
        chunks: chunks as u32,
        upcoming,
    })
}

/// `?1, ?2, …` for an `IN` list.
pub(crate) fn placeholders(n: usize) -> String {
    (1..=n)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Design §4's authority ordering, as a rank the ranking reads:
/// `user` > `extracted_user` > `imported` > `external` > `extracted_other`.
/// An unknown origin ranks with the lowest.
pub(crate) fn authority_rank(origin: &str) -> i64 {
    match origin {
        "user" => 4,
        "extracted_user" => 3,
        "imported" => 2,
        "external" => 1,
        _ => 0,
    }
}

fn validate(record: &IndexRecord) -> Result<()> {
    for (field, value) in [
        ("id", &record.id),
        ("revision", &record.revision),
        ("partition_key", &record.partition_key),
        ("relative_path", &record.relative_path),
    ] {
        if value.trim().is_empty() {
            return Err(IndexError::InvalidInput(format!(
                "{field} must not be empty"
            )));
        }
    }
    if record.body.len() > MAX_NOTE_BYTES {
        return Err(IndexError::InvalidInput(format!(
            "note {} body is {} bytes; the limit is {MAX_NOTE_BYTES}",
            record.id,
            record.body.len()
        )));
    }
    Ok(())
}

/// The insert behind every write. Returns the number of chunk rows written.
fn upsert_in(tx: &rusqlite::Transaction<'_>, record: &IndexRecord) -> Result<usize> {
    validate(record)?;
    let updated_at = canonicalize_timestamp("updated_at", &record.updated_at)?;
    let created_at = canonicalize_timestamp("created_at", &record.created_at)?;
    let observed_at = record
        .observed_at
        .as_deref()
        .map(|v| canonicalize_timestamp("observed_at", v))
        .transpose()?;
    // Design 09 §13.1: a note is dated by what it observed, else by when it
    // was first written — never by `updated_at`, which any edit moves.
    let date = observed_at.clone().unwrap_or_else(|| created_at.clone());
    let valid_from = record
        .valid_from
        .as_deref()
        .map(|v| canonicalize_timestamp("valid_from", v))
        .transpose()?;
    let valid_to = record
        .valid_to
        .as_deref()
        .map(|v| canonicalize_timestamp("valid_to", v))
        .transpose()?;

    // Replace whatever holds this id or this path in the partition.
    let mut stale = tx.prepare_cached(
        "SELECT rowid FROM note WHERE partition_key = ?1 AND (id = ?2 OR relative_path = ?3)",
    )?;
    let stale_rows: Vec<i64> = stale
        .query_map(
            params![record.partition_key, record.id, record.relative_path],
            |r| r.get(0),
        )?
        .collect::<rusqlite::Result<_>>()?;
    drop(stale);
    for rowid in stale_rows {
        delete_note_rows(tx, rowid)?;
    }

    let norm_title = query::normalize_exact(&record.title);
    let norm_aliases: Vec<String> = record
        .aliases
        .iter()
        .map(|a| query::normalize_exact(a))
        .filter(|a| !a.is_empty())
        .collect();
    let norm_tags: Vec<String> = record
        .tags
        .iter()
        .map(|t| query::normalize_exact(t))
        .filter(|t| !t.is_empty())
        .collect();
    // The header's text for the ranker and the short-term pass, once per
    // note, one alias or tag per line (design 09 §6.5.1).
    let header_aliases = norm_aliases.join("\n");
    let header_tags = norm_tags.join("\n");
    let aliases_json = serde_json::to_string(&record.aliases).unwrap_or_else(|_| "[]".into());
    let tags_json = serde_json::to_string(&record.tags).unwrap_or_else(|_| "[]".into());
    let superseded_json =
        serde_json::to_string(&record.superseded_by).unwrap_or_else(|_| "[]".into());
    let supersedes_json = serde_json::to_string(&record.supersedes).unwrap_or_else(|_| "[]".into());

    tx.execute(
        "INSERT INTO note (
            partition_key, id, scope, revision, relative_path, title, norm_title,
            aliases, tags, kind, status, effective_status, origin, authority,
            fact_key, valid_from, valid_to, updated_at, pinned, source_forgotten,
            conflict, same_key_loser, superseded_by, supersedes, body_line_offset,
            mtime_ns, size, inode, created_at, observed_at, date, norm_aliases, norm_tags
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
            ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31,
            ?32, ?33
        )",
        params![
            record.partition_key,
            record.id,
            record.scope,
            record.revision,
            record.relative_path,
            record.title,
            norm_title,
            aliases_json,
            tags_json,
            record.kind,
            record.status,
            record.effective_status.as_str(),
            record.origin,
            authority_rank(&record.origin),
            record.fact_key,
            valid_from,
            valid_to,
            updated_at,
            record.pinned as i64,
            record.source_forgotten as i64,
            record.conflict as i64,
            record.same_key_loser as i64,
            superseded_json,
            supersedes_json,
            record.body_line_offset as i64,
            record.file.mtime_ns,
            record.file.size as i64,
            record.file.inode as i64,
            created_at,
            observed_at,
            date,
            header_aliases,
            header_tags,
        ],
    )?;
    let note_rowid = tx.last_insert_rowid();

    {
        let mut alias_stmt =
            tx.prepare_cached("INSERT INTO note_alias (note_rowid, norm_alias) VALUES (?1, ?2)")?;
        for alias in &norm_aliases {
            alias_stmt.execute(params![note_rowid, alias])?;
        }
    }
    {
        let mut source_stmt =
            tx.prepare_cached("INSERT INTO note_source (note_rowid, source_key) VALUES (?1, ?2)")?;
        let mut seen = std::collections::HashSet::new();
        for key in &record.sources {
            if !key.is_empty() && seen.insert(key.as_str()) {
                source_stmt.execute(params![note_rowid, key])?;
            }
        }
    }

    let mut chunks = chunker::chunk_body(&record.body);
    if chunks.is_empty() {
        // Title, aliases and tags stay searchable through an empty chunk.
        chunks.push(chunker::Chunk {
            ordinal: 0,
            heading: None,
            line_from: 0,
            line_to: 0,
            text: String::new(),
        });
    }
    // The FTS tables are contentless: the upsert writes each chunk's rows
    // itself, keyed by the chunk's id, and the `chunk_ad` trigger removes
    // them however the chunk row goes (design 09 §6.5.1). The word table
    // reads the text split at script boundaries; the header trigram table
    // the header as it is; the body trigram table only a chunk that carries
    // an unsegmented character.
    let word_title = query::split_scripts(&norm_title);
    let word_aliases = query::split_scripts(&header_aliases);
    let word_tags = query::split_scripts(&header_tags);
    let mut chunk_stmt = tx.prepare_cached(
        "INSERT INTO chunk (
            note_rowid, partition_key, ordinal, heading, line_from, line_to, body
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    let mut word_stmt = tx.prepare_cached(
        "INSERT INTO fts_word (rowid, title, aliases, tags, body) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    let mut head_stmt = tx.prepare_cached(
        "INSERT INTO fts_tri_head (rowid, title, aliases, tags) VALUES (?1, ?2, ?3, ?4)",
    )?;
    let mut body_stmt = tx.prepare_cached("INSERT INTO fts_tri (rowid, body) VALUES (?1, ?2)")?;
    let offset = record.body_line_offset;
    for chunk in &chunks {
        let (line_from, line_to) = if chunk.line_from == 0 {
            (0, 0)
        } else {
            (chunk.line_from + offset, chunk.line_to + offset)
        };
        let body = query::normalize(&chunk.text);
        chunk_stmt.execute(params![
            note_rowid,
            record.partition_key,
            chunk.ordinal as i64,
            chunk.heading,
            line_from as i64,
            line_to as i64,
            body,
        ])?;
        let chunk_id = tx.last_insert_rowid();
        word_stmt.execute(params![
            chunk_id,
            word_title,
            word_aliases,
            word_tags,
            query::split_scripts(&body),
        ])?;
        head_stmt.execute(params![chunk_id, norm_title, header_aliases, header_tags])?;
        if body.chars().any(query::is_unsegmented) {
            body_stmt.execute(params![chunk_id, body])?;
        }
    }
    Ok(chunks.len())
}

/// `optimize` on every FTS table: one merged segment each, and a
/// contentless-delete table's tombstones folded in.
fn optimize_fts(conn: &Connection) -> rusqlite::Result<()> {
    for table in FTS_TABLES {
        conn.execute(&format!("INSERT INTO {table}({table}) VALUES ('optimize')"), [])?;
    }
    Ok(())
}

/// Deletes one note's chunks (the delete trigger removes their FTS rows),
/// aliases and catalog row.
fn delete_note_rows(tx: &rusqlite::Transaction<'_>, note_rowid: i64) -> rusqlite::Result<()> {
    tx.execute(
        "DELETE FROM chunk WHERE note_rowid = ?1",
        params![note_rowid],
    )?;
    tx.execute(
        "DELETE FROM note_alias WHERE note_rowid = ?1",
        params![note_rowid],
    )?;
    tx.execute(
        "DELETE FROM note_source WHERE note_rowid = ?1",
        params![note_rowid],
    )?;
    tx.execute("DELETE FROM note WHERE rowid = ?1", params![note_rowid])?;
    Ok(())
}

fn delete_partition_rows(
    tx: &rusqlite::Transaction<'_>,
    partition_key: &str,
) -> rusqlite::Result<u64> {
    tx.execute(
        "DELETE FROM chunk WHERE partition_key = ?1",
        params![partition_key],
    )?;
    tx.execute(
        "DELETE FROM note_alias WHERE note_rowid IN (SELECT rowid FROM note WHERE partition_key = ?1)",
        params![partition_key],
    )?;
    tx.execute(
        "DELETE FROM note_source WHERE note_rowid IN (SELECT rowid FROM note WHERE partition_key = ?1)",
        params![partition_key],
    )?;
    let removed = tx.execute(
        "DELETE FROM note WHERE partition_key = ?1",
        params![partition_key],
    )?;
    tx.execute(
        "DELETE FROM partition WHERE partition_key = ?1",
        params![partition_key],
    )?;
    Ok(removed as u64)
}

/// The page cache the connection keeps: the FTS indexes of a 10K-note corpus
/// run to a couple of hundred MiB, and a search touches many of their pages;
/// with SQLite's 2 MiB default every query re-reads and re-decodes them.
const CACHE_SIZE_KIB: i64 = 64 * 1024;
/// Memory-mapped I/O bound, so page reads are loads rather than syscalls.
const MMAP_BYTES: i64 = 256 * 1024 * 1024;

fn apply_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(&format!(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;
         PRAGMA temp_store = MEMORY;
         PRAGMA cache_size = -{CACHE_SIZE_KIB};
         PRAGMA mmap_size = {MMAP_BYTES};
         PRAGMA journal_size_limit = {JOURNAL_SIZE_LIMIT};"
    ))
}

/// The database's size: its pages, free ones included, times the page size.
fn file_bytes(conn: &Connection) -> rusqlite::Result<u64> {
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    Ok((pages * page_size) as u64)
}

fn read_rebuilt_bytes(conn: &Connection) -> rusqlite::Result<Option<u64>> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![META_REBUILT_BYTES],
            |r| r.get(0),
        )
        .optional()?;
    Ok(value.and_then(|v| v.parse().ok()))
}

fn write_rebuilt_bytes(conn: &Connection, bytes: u64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)",
        params![META_REBUILT_BYTES, bytes.to_string()],
    )?;
    Ok(())
}

fn create_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA_SQL)?;
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)",
        params![META_SCHEMA, SCHEMA_VERSION.to_string()],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO meta(key, value) VALUES (?1, ?2)",
        params![META_STATE, IndexState::Rebuilding.as_str()],
    )?;
    conn.execute(
        "INSERT INTO fts_word(fts_word, rank) VALUES ('rank', ?1)",
        params![RANK_WEIGHTS],
    )?;
    conn.execute(
        "INSERT INTO fts_tri_head(fts_tri_head, rank) VALUES ('rank', ?1)",
        params![HEAD_RANK_WEIGHTS],
    )?;
    Ok(())
}

fn read_state(conn: &Connection) -> rusqlite::Result<IndexState> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![META_STATE],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match value.as_deref() {
        Some("ready") => IndexState::Ready,
        _ => IndexState::Rebuilding,
    })
}

fn write_state(conn: &Connection, state: IndexState) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)",
        params![META_STATE, state.as_str()],
    )?;
    Ok(())
}

/// Removes the cache file and its WAL sidecars; absent is fine.
fn delete_cache_files(path: &Path) -> std::io::Result<()> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        match std::fs::remove_file(PathBuf::from(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// The canonical timestamp form the catalog compares: RFC 3339 UTC to the
/// second with `Z` (design §4).
pub(crate) fn canon_timestamp(value: &DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Parses any RFC 3339 timestamp and re-formats it to the canon.
pub(crate) fn canonicalize_timestamp(field: &str, value: &str) -> Result<String> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|e| {
        IndexError::InvalidInput(format!("{field} is not RFC 3339 ({value:?}): {e}"))
    })?;
    Ok(canon_timestamp(&parsed.with_timezone(&Utc)))
}

#[cfg(test)]
mod bench;
#[cfg(test)]
mod fixture_tests;
#[cfg(test)]
mod tests;

// ============================================================================
// Reconciliation
// ============================================================================

impl MemoryIndex {
    /// One reconciliation stat scan of a partition (design §5, *Freshness
    /// without a watcher*): `scanner` lists the files under the partition's
    /// `notes/` with their mtime, size and inode; the index compares that
    /// listing to its rows and reports what is new, changed or gone. It
    /// changes no row — only `memory_files` can re-hash and re-parse a file —
    /// and records the scan on the partition so `index_state` can say when
    /// it last ran.
    pub fn reconcile<F>(&self, partition_key: &str, scanner: F) -> Result<ReconcileReport>
    where
        F: FnOnce() -> std::io::Result<Vec<ScanEntry>>,
    {
        let entries = scanner()?;
        let conn = self.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT id, relative_path, mtime_ns, size, inode FROM note WHERE partition_key = ?1",
        )?;
        let indexed: Vec<(String, String, FileStat)> = stmt
            .query_map(params![partition_key], |r| {
                let size: i64 = r.get(3)?;
                let inode: i64 = r.get(4)?;
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    FileStat {
                        mtime_ns: r.get(2)?,
                        size: size as u64,
                        inode: inode as u64,
                    },
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);

        let on_disk: std::collections::HashMap<&str, &FileStat> = entries
            .iter()
            .map(|e| (e.relative_path.as_str(), &e.stat))
            .collect();
        let mut report = ReconcileReport {
            partition_key: partition_key.to_string(),
            scanned: entries.len(),
            ..ReconcileReport::default()
        };
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (id, path, stat) in &indexed {
            seen.insert(path.as_str());
            match on_disk.get(path.as_str()) {
                None => report.removed.push((id.clone(), path.clone())),
                Some(current) if **current != *stat => {
                    report.changed.push((id.clone(), path.clone()));
                }
                Some(_) => {}
            }
        }
        for entry in &entries {
            if !seen.contains(entry.relative_path.as_str()) {
                report.added.push(entry.relative_path.clone());
            }
        }
        report.added.sort();
        report.changed.sort();
        report.removed.sort();

        self.stamp_in(&conn, partition_key, entries.len())?;
        Ok(report)
    }

    /// Records that `partition_key` was scanned now with `files` note files
    /// — what [`MemoryIndex::reconcile`] records — without comparing any
    /// row: the stamp a caller makes when it has just brought the rows in
    /// line with a scan of its own (a write's re-index, design §5 step 4;
    /// scale slice S4, O10).
    pub fn stamp_scan(&self, partition_key: &str, files: usize) -> Result<()> {
        let conn = self.lock();
        self.stamp_in(&conn, partition_key, files)
    }

    fn stamp_in(&self, conn: &Connection, partition_key: &str, files: usize) -> Result<()> {
        conn.execute(
            "INSERT INTO partition (partition_key, last_scan_at, last_scan_files)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(partition_key) DO UPDATE SET
                 last_scan_at = excluded.last_scan_at,
                 last_scan_files = excluded.last_scan_files",
            params![partition_key, canon_timestamp(&self.now()), files as i64],
        )?;
        Ok(())
    }

    /// When a partition was last scanned, if ever.
    pub fn last_scan(&self, partition_key: &str) -> Result<Option<(String, u64)>> {
        let conn = self.lock();
        conn.query_row(
            "SELECT last_scan_at, last_scan_files FROM partition WHERE partition_key = ?1",
            params![partition_key],
            |r| {
                let files: i64 = r.get(1)?;
                Ok((r.get::<_, String>(0)?, files as u64))
            },
        )
        .optional()
        .map_err(IndexError::from)
    }
}
