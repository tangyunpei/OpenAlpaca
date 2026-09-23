//! `memory_index` — the disposable lexical catalog over Markdown memory notes
//! (design `tasks/memory-redesign/03-design.md` §5, §8; plan slice 2).
//!
//! One SQLite file at `state/cache/memory-index-v1.sqlite` holds, per note, a
//! catalog row (identity, revision, relative path, the pre-computed filter
//! fields), its chunks with line spans, and two FTS5 indexes over the chunks:
//! a `unicode61` table for whole-token hits and a `trigram` companion for
//! substring hits (`prefer` inside `prefers`, any CJK run of three or more
//! characters). Every search ORs every normalized term of three or more
//! characters against **both** tables; a trigram-only hit is weighted below a
//! whole-token hit; one- and two-character queries go through a bounded
//! substring pass that reports `partial` when its cap is hit; no stemmer.
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
mod search;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, params};

pub use search::{MatchReason, SearchHit, SearchMode, SearchQuery, SearchResults};

/// The cache file's name under `state/cache/`.
pub const INDEX_FILE_NAME: &str = "memory-index-v1.sqlite";
/// The on-disk schema this build writes. A file carrying another value is
/// recreated on open (and reports `rebuilding`), never migrated: the index is
/// disposable.
pub const SCHEMA_VERSION: i64 = 1;
/// Design §4: a note is at most 32 KiB. A larger body is refused by
/// [`MemoryIndex::upsert`] so the cache stays bounded by what `memory_files`
/// would have written.
pub const MAX_NOTE_BYTES: usize = 32 * 1024;
/// Design §7a: `memory_search.query` is 1–512 characters.
pub const MAX_QUERY_CHARS: usize = 512;
/// Design §7a: `limit` is 1–20, default 8.
pub const DEFAULT_LIMIT: usize = 8;
/// Design §7a: `limit` is 1–20.
pub const MAX_LIMIT: usize = 20;
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

/// The cache database. One connection behind a mutex, like [`crate::Database`].
pub struct MemoryIndex {
    conn: Mutex<Connection>,
    path: Option<PathBuf>,
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
    kind             TEXT NOT NULL,
    status           TEXT NOT NULL,
    effective_status TEXT NOT NULL,
    origin           TEXT NOT NULL,
    authority        INTEGER NOT NULL,
    fact_key         TEXT,
    valid_from       TEXT,
    valid_to         TEXT,
    updated_at       TEXT NOT NULL,
    pinned           INTEGER NOT NULL,
    source_forgotten INTEGER NOT NULL,
    conflict         INTEGER NOT NULL,
    same_key_loser   INTEGER NOT NULL,
    superseded_by    TEXT NOT NULL,
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
CREATE TABLE IF NOT EXISTS note_alias (
    note_rowid INTEGER NOT NULL REFERENCES note(rowid) ON DELETE CASCADE,
    norm_alias TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS note_alias_norm ON note_alias(norm_alias, note_rowid);
CREATE INDEX IF NOT EXISTS note_alias_note ON note_alias(note_rowid);
CREATE TABLE IF NOT EXISTS chunk (
    id            INTEGER PRIMARY KEY,
    note_rowid    INTEGER NOT NULL REFERENCES note(rowid) ON DELETE CASCADE,
    partition_key TEXT NOT NULL,
    ordinal       INTEGER NOT NULL,
    heading       TEXT,
    line_from     INTEGER NOT NULL,
    line_to       INTEGER NOT NULL,
    excerpt       TEXT NOT NULL,
    title         TEXT NOT NULL,
    aliases       TEXT NOT NULL,
    tags          TEXT NOT NULL,
    body          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS chunk_note ON chunk(note_rowid, ordinal);
CREATE INDEX IF NOT EXISTS chunk_partition ON chunk(partition_key, id);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_word USING fts5(
    title, aliases, tags, body,
    content='chunk', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_tri USING fts5(
    title, aliases, tags, body,
    content='chunk', content_rowid='id',
    tokenize='trigram'
);
CREATE TRIGGER IF NOT EXISTS chunk_ai AFTER INSERT ON chunk BEGIN
    INSERT INTO fts_word(rowid, title, aliases, tags, body)
        VALUES (new.id, new.title, new.aliases, new.tags, new.body);
    INSERT INTO fts_tri(rowid, title, aliases, tags, body)
        VALUES (new.id, new.title, new.aliases, new.tags, new.body);
END;
CREATE TRIGGER IF NOT EXISTS chunk_ad AFTER DELETE ON chunk BEGIN
    INSERT INTO fts_word(fts_word, rowid, title, aliases, tags, body)
        VALUES ('delete', old.id, old.title, old.aliases, old.tags, old.body);
    INSERT INTO fts_tri(fts_tri, rowid, title, aliases, tags, body)
        VALUES ('delete', old.id, old.title, old.aliases, old.tags, old.body);
END;
"#;

/// BM25 column weights: title, aliases, tags, body. Pinned by
/// `search::tests`; not claimed optimal (design §5 step 6).
const RANK_WEIGHTS: &str = "bm25(4.0, 3.0, 1.5, 1.0)";

const META_SCHEMA: &str = "schema_version";
const META_STATE: &str = "state";

/// The tables an intact cache has. A file missing any is recreated.
const EXPECTED_TABLES: &[&str] = &[
    "meta",
    "partition",
    "note",
    "note_alias",
    "chunk",
    "fts_word",
    "fts_tri",
];

impl MemoryIndex {
    /// `state/cache/memory-index-v1.sqlite` under the home store (design §2).
    /// Creates `state/` (0700) like every other `state/` accessor; the
    /// `cache/` directory is created by [`MemoryIndex::open`].
    pub fn default_path() -> anyhow::Result<PathBuf> {
        Ok(crate::store::state_dir()?
            .join("cache")
            .join(INDEX_FILE_NAME))
    }

    /// Opens the cache at `path`, creating it when absent. A file that fails
    /// `quick_check`, carries another [`SCHEMA_VERSION`] or lacks a table is
    /// deleted and recreated — it is a cache — and the fresh index reports
    /// [`IndexState::Rebuilding`] until [`MemoryIndex::rebuild`] runs. An
    /// intact file keeps the state it recorded.
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
        })
    }

    fn open_existing(path: &Path) -> std::result::Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        apply_pragmas(&conn).map_err(|e| e.to_string())?;
        let check: String = conn
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if check != "ok" {
            return Err(format!("quick_check: {check}"));
        }
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
        })
    }

    /// The path this index was opened at; `None` in memory.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("memory index mutex was poisoned; recovering");
            poisoned.into_inner()
        })
    }

    /// The state and the row counts.
    pub fn index_state(&self) -> Result<IndexStateReport> {
        let conn = self.lock();
        let state = read_state(&conn)?;
        let notes: i64 = conn.query_row("SELECT COUNT(*) FROM note", [], |r| r.get(0))?;
        let chunks: i64 = conn.query_row("SELECT COUNT(*) FROM chunk", [], |r| r.get(0))?;
        let partitions: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT partition_key) FROM note",
            [],
            |r| r.get(0),
        )?;
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

fn apply_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;
        PRAGMA busy_timeout = 5000;
        PRAGMA temp_store = MEMORY;
        "#,
    )
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
        "INSERT INTO fts_tri(fts_tri, rank) VALUES ('rank', ?1)",
        params![RANK_WEIGHTS],
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
mod tests;
