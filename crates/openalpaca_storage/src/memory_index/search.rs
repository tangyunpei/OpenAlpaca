//! Search: candidate generation from both FTS tables, the substring pass and
//! the exact lookups; the filters, applied before the limit; ranking.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};

use super::query::{self, ParsedQuery};
use super::{
    CANDIDATE_CAP, EffectiveStatus, IndexError, IndexState, MAX_LIMIT, MemoryIndex, Result,
    SUBSTRING_SCAN_CAP, canon_timestamp, placeholders, read_state,
};

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
    /// The automatic per-turn recall: both are excluded, and so is a
    /// conversation summary (design 09 §13.3,
    /// [`crate::memory_files::is_conversation_summary`]).
    Automatic,
}

/// How a search orders its hits (design 09 §13.1, `order_by`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchOrder {
    /// The ranking of design §5 step 6.
    #[default]
    Relevance,
    /// The newest `date` first — `observed_at`, else `created_at`.
    Newest,
    /// The oldest `date` first.
    Oldest,
}

impl SearchOrder {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::Newest => "newest",
            Self::Oldest => "oldest",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "relevance" => Some(Self::Relevance),
            "newest" => Some(Self::Newest),
            "oldest" => Some(Self::Oldest),
            _ => None,
        }
    }
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
    /// 1–[`MAX_LIMIT`] (50).
    pub limit: usize,
    /// `true` includes superseded and archived notes and, with no `as_of`,
    /// notes whose validity interval has ended. Candidates are never
    /// returned.
    pub historical: bool,
    /// Evaluate the half-open validity interval at this instant. Absent: a
    /// note whose interval ended by now is dropped and one that has not
    /// begun is kept (design §4 *Times*), unless `historical`, which then
    /// applies no validity filter.
    pub as_of: Option<DateTime<Utc>>,
    /// Only notes dated at or after this instant (design 09 §13.1: the date
    /// is `observed_at`, else `created_at`), applied before the limit.
    pub date_from: Option<DateTime<Utc>>,
    /// Only notes dated at or before this instant.
    pub date_to: Option<DateTime<Utc>>,
    pub order: SearchOrder,
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
            date_from: None,
            date_to: None,
            order: SearchOrder::Relevance,
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
    /// The note's window has not begun at the instant the validity filter
    /// used — `as_of`, else the index clock's "now" — so the recall header
    /// and the filter agree on one clock (design 09 §6.1).
    pub upcoming: bool,
    /// The note's window has ended at that same instant — only a
    /// `historical` search without `as_of` returns such a note (design 09
    /// §13.1: an index row marks it).
    pub ended: bool,
    /// `observed_at`, else `created_at`, in the canon (design 09 §13.1).
    pub date: String,
    pub updated_at: String,
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

// ============================================================================
// Ranking weights (design §5 step 6: pinned and tested, not claimed optimal)
// ============================================================================

/// The whole normalized query equals the title.
pub(crate) const W_EXACT_TITLE: f64 = 100.0;
/// The whole normalized query equals an alias.
pub(crate) const W_EXACT_ALIAS: f64 = 90.0;
/// The query equals the `fact_key`.
pub(crate) const W_EXACT_FACT_KEY: f64 = 90.0;
/// Per identifier or path literal matched as a phrase.
pub(crate) const W_PHRASE: f64 = 12.0;
/// Per word matched as a whole token.
pub(crate) const W_TERM: f64 = 10.0;
/// Per word matched only as a substring — the trigram-only hit, below a
/// whole-token hit (design §5 step 4).
pub(crate) const W_TRIGRAM: f64 = 5.0;
/// Per one- or two-character term found by the substring pass.
pub(crate) const W_SHORT: f64 = 5.0;
/// BM25 relevance from the `unicode61` table, normalized to 0..=1 over the
/// candidates.
pub(crate) const W_BM25_WORD: f64 = 2.0;
/// BM25 relevance from the trigram table, normalized likewise.
pub(crate) const W_BM25_TRIGRAM: f64 = 1.0;
/// Per authority rank (design §4: `user` = 4 … `extracted_other` = 0).
pub(crate) const W_AUTHORITY: f64 = 0.5;
/// A note whose explicit `valid_from`/`valid_to` interval holds at the
/// evaluation instant.
pub(crate) const W_VALID_NOW: f64 = 1.0;

// ============================================================================
// Search
// ============================================================================

/// The filters of design §5 step 5 as one SQL fragment over the `note`
/// alias `n`, applied inside every candidate query — so the caps count
/// eligible rows and the limit is applied last.
struct Filter {
    clause: String,
    params: Vec<Value>,
    /// The instant validity is evaluated at, for the ranking bonus.
    instant: String,
}

impl Filter {
    fn build(q: &SearchQuery, now: DateTime<Utc>) -> Self {
        let mut clause = format!(
            "n.partition_key IN ({})",
            placeholders_unnumbered(q.partition_keys.len())
        );
        let mut params: Vec<Value> = q
            .partition_keys
            .iter()
            .map(|k| Value::Text(k.clone()))
            .collect();
        if let Some(kind) = &q.kind {
            clause.push_str(" AND n.kind = ?");
            params.push(Value::Text(kind.clone()));
        }
        if q.historical {
            clause.push_str(" AND n.effective_status IN ('active', 'superseded', 'archived')");
        } else {
            clause.push_str(" AND n.effective_status = 'active'");
        }
        let now = canon_timestamp(&now);
        let instant = q.as_of.map(|t| canon_timestamp(&t)).unwrap_or(now);
        // Design §4 *Times*: a historical query evaluates the requested
        // instant — the whole half-open interval; current recall excludes a
        // note whose interval has **ended**, and keeps one that has not begun
        // (a trip planned for next month is current knowledge).
        if q.as_of.is_some() {
            clause.push_str(
                " AND (n.valid_from IS NULL OR n.valid_from <= ?) AND (n.valid_to IS NULL OR n.valid_to > ?)",
            );
            params.push(Value::Text(instant.clone()));
            params.push(Value::Text(instant.clone()));
        } else if !q.historical {
            clause.push_str(" AND (n.valid_to IS NULL OR n.valid_to > ?)");
            params.push(Value::Text(instant.clone()));
        }
        if q.mode == SearchMode::Automatic {
            clause.push_str(" AND n.same_key_loser = 0 AND n.source_forgotten = 0");
            // Design 09 §13.3 (MD-16): a conversation summary is found by an
            // explicit search and never delivered on its own.
            clause.push_str(" AND NOT (n.kind = 'episode' AND n.origin = 'extracted_other')");
        }
        // Design 09 §13.1: the date bounds are inclusive and, like every
        // filter here, run before the limit.
        if let Some(from) = q.date_from {
            clause.push_str(" AND n.date >= ?");
            params.push(Value::Text(canon_timestamp(&from)));
        }
        if let Some(to) = q.date_to {
            clause.push_str(" AND n.date <= ?");
            params.push(Value::Text(canon_timestamp(&to)));
        }
        Self {
            clause,
            params,
            instant,
        }
    }
}

fn placeholders_unnumbered(n: usize) -> String {
    vec!["?"; n].join(", ")
}

/// A candidate chunk with the evidence each generator produced.
#[derive(Default)]
struct ChunkEvidence {
    note_rowid: i64,
    word_rank: Option<f64>,
    tri_rank: Option<f64>,
}

#[derive(Default)]
struct ExactEvidence {
    title: bool,
    alias: bool,
    fact_key: bool,
}

struct NoteRow {
    rowid: i64,
    id: String,
    partition_key: String,
    scope: String,
    revision: String,
    relative_path: String,
    title: String,
    kind: String,
    status: String,
    effective_status: EffectiveStatus,
    origin: String,
    fact_key: Option<String>,
    valid_from: Option<String>,
    valid_to: Option<String>,
    updated_at: String,
    date: String,
    pinned: bool,
    conflict: bool,
    same_key_loser: bool,
    source_forgotten: bool,
    superseded_by: Vec<String>,
    authority: i64,
}

struct ChunkRow {
    id: i64,
    note_rowid: i64,
    ordinal: u32,
    heading: Option<String>,
    line_from: u32,
    line_to: u32,
    excerpt: String,
    title: String,
    aliases: String,
    tags: String,
    body: String,
}

impl MemoryIndex {
    /// One search (design §5 steps 3–6). The filters — partition, kind,
    /// status, derived supersession, validity, and on the automatic path the
    /// same-`fact_key` loser and `source_forgotten` — run inside every
    /// candidate query, before the limit. Verification of the selected files
    /// (step 7) is the caller's: the hit carries the revision and path it
    /// needs for that.
    pub fn search(&self, q: &SearchQuery) -> Result<SearchResults> {
        if q.limit == 0 || q.limit > MAX_LIMIT {
            return Err(IndexError::InvalidInput(format!(
                "limit must be 1–{MAX_LIMIT}, got {}",
                q.limit
            )));
        }
        let parsed = query::parse_query(&q.query)?;
        let conn = self.lock();
        let index_state = read_state(&conn)?;
        if q.partition_keys.is_empty() {
            return Ok(SearchResults {
                hits: Vec::new(),
                count: 0,
                partial: false,
                index_state,
            });
        }
        let filter = Filter::build(q, self.now());
        let mut partial = false;
        let mut chunks: HashMap<i64, ChunkEvidence> = HashMap::new();
        let mut exact: HashMap<i64, ExactEvidence> = HashMap::new();

        // A literal's parts are routed only when the literal itself matches
        // no eligible note.
        let mut words: Vec<String> = parsed.words.clone();
        for literal in &parsed.literals {
            if !literal_has_hit(&conn, &literal.text, &filter)? {
                for part in &literal.parts {
                    if !words.contains(part) {
                        words.push(part.clone());
                    }
                }
            }
        }

        if let Some(m) = parsed.word_match(&words) {
            let rows = fts_candidates(&conn, "fts_word", &m, &filter)?;
            if rows.len() >= CANDIDATE_CAP {
                partial = true;
            }
            for (chunk_id, note_rowid, rank) in rows {
                let e = chunks.entry(chunk_id).or_default();
                e.note_rowid = note_rowid;
                e.word_rank = Some(rank);
            }
        }
        if let Some(m) = parsed.trigram_match(&words) {
            let rows = fts_candidates(&conn, "fts_tri", &m, &filter)?;
            if rows.len() >= CANDIDATE_CAP {
                partial = true;
            }
            for (chunk_id, note_rowid, rank) in rows {
                let e = chunks.entry(chunk_id).or_default();
                e.note_rowid = note_rowid;
                e.tri_rank = Some(rank);
            }
        }
        if !parsed.shorts.is_empty() {
            let (rows, capped) = substring_candidates(&conn, &parsed.shorts, &filter)?;
            partial |= capped;
            for (chunk_id, note_rowid) in rows {
                chunks.entry(chunk_id).or_default().note_rowid = note_rowid;
            }
        }
        if !parsed.normalized.is_empty() {
            for rowid in exact_rows(&conn, "n.norm_title = ?", &parsed.normalized, &filter)? {
                exact.entry(rowid).or_default().title = true;
            }
            for rowid in alias_rows(&conn, &parsed.normalized, &filter)? {
                exact.entry(rowid).or_default().alias = true;
            }
            for rowid in exact_rows(&conn, "n.fact_key = ?", &parsed.normalized, &filter)? {
                exact.entry(rowid).or_default().fact_key = true;
            }
        }

        // Load the rows behind the evidence.
        let mut note_rowids: BTreeSet<i64> = chunks.values().map(|e| e.note_rowid).collect();
        note_rowids.extend(exact.keys().copied());
        if note_rowids.is_empty() {
            return Ok(SearchResults {
                hits: Vec::new(),
                count: 0,
                partial,
                index_state,
            });
        }
        let notes = load_notes(&conn, &note_rowids)?;
        let mut chunk_rows = load_chunks(&conn, &chunks.keys().copied().collect::<Vec<_>>())?;
        let exact_only: Vec<i64> = exact
            .keys()
            .copied()
            .filter(|rowid| !chunks.values().any(|e| e.note_rowid == *rowid))
            .collect();
        chunk_rows.extend(load_first_chunks(&conn, &exact_only)?);
        drop(conn);

        Ok(rank(
            q,
            &parsed,
            &words,
            &filter,
            notes,
            chunk_rows,
            &chunks,
            &exact,
            partial,
            index_state,
        ))
    }
}

/// The best-ranked eligible chunks of one FTS table: `(chunk id, note
/// rowid, rank)`, rank ascending (more negative is better).
fn fts_candidates(
    conn: &Connection,
    table: &str,
    match_expr: &str,
    filter: &Filter,
) -> Result<Vec<(i64, i64, f64)>> {
    // CROSS JOIN pins the FTS scan as the outer loop.
    let sql = format!(
        "SELECT c.id, c.note_rowid, {table}.rank
         FROM {table}
         CROSS JOIN chunk c ON c.id = {table}.rowid
         CROSS JOIN note n ON n.rowid = c.note_rowid
         WHERE {table} MATCH ? AND {filter}
         ORDER BY {table}.rank, c.id
         LIMIT ?",
        filter = filter.clause
    );
    let mut params: Vec<Value> = Vec::with_capacity(filter.params.len() + 2);
    params.push(Value::Text(match_expr.to_string()));
    params.extend(filter.params.iter().cloned());
    params.push(Value::Integer(CANDIDATE_CAP as i64));
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, f64>(2)?,
        ))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(IndexError::from)
}

/// The bounded substring pass for one- and two-character terms: scans at
/// most [`SUBSTRING_SCAN_CAP`] eligible chunks in index order and returns
/// the chunks containing any of `shorts`, plus whether the cap was hit.
fn substring_candidates(
    conn: &Connection,
    shorts: &[String],
    filter: &Filter,
) -> Result<(Vec<(i64, i64)>, bool)> {
    let sql = format!(
        "SELECT c.id, c.note_rowid, c.title, c.aliases, c.tags, c.body
         FROM chunk c
         CROSS JOIN note n ON n.rowid = c.note_rowid
         WHERE {filter}
         ORDER BY c.id
         LIMIT ?",
        filter = filter.clause
    );
    let mut params: Vec<Value> = filter.params.clone();
    params.push(Value::Integer(SUBSTRING_SCAN_CAP as i64 + 1));
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut rows = stmt.query(params_from_iter(params.iter()))?;
    let mut hits = Vec::new();
    let mut scanned = 0usize;
    while let Some(row) = rows.next()? {
        scanned += 1;
        if scanned > SUBSTRING_SCAN_CAP {
            return Ok((hits, true));
        }
        let id: i64 = row.get(0)?;
        let note_rowid: i64 = row.get(1)?;
        let fields: [String; 4] = [row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?];
        if shorts
            .iter()
            .any(|s| fields.iter().any(|f| f.contains(s.as_str())))
        {
            hits.push((id, note_rowid));
        }
    }
    Ok((hits, false))
}

/// Whether a literal, as a phrase, matches any eligible note on either
/// table — the probe that decides whether its parts are routed.
fn literal_has_hit(conn: &Connection, literal: &str, filter: &Filter) -> Result<bool> {
    let phrase = format!("\"{}\"", literal.replace('"', "\"\""));
    let mut tables = vec!["fts_word"];
    if literal.chars().count() >= 3 {
        tables.push("fts_tri");
    }
    for table in tables {
        let sql = format!(
            "SELECT EXISTS(
                SELECT 1 FROM {table}
                CROSS JOIN chunk c ON c.id = {table}.rowid
                CROSS JOIN note n ON n.rowid = c.note_rowid
                WHERE {table} MATCH ? AND {filter})",
            filter = filter.clause
        );
        let mut params: Vec<Value> = Vec::with_capacity(filter.params.len() + 1);
        params.push(Value::Text(phrase.clone()));
        params.extend(filter.params.iter().cloned());
        let mut stmt = conn.prepare_cached(&sql)?;
        let hit: bool = stmt.query_row(params_from_iter(params.iter()), |r| r.get(0))?;
        if hit {
            return Ok(true);
        }
    }
    Ok(false)
}

fn exact_rows(
    conn: &Connection,
    predicate: &str,
    value: &str,
    filter: &Filter,
) -> Result<Vec<i64>> {
    let sql = format!(
        "SELECT n.rowid FROM note n WHERE {predicate} AND {filter}",
        filter = filter.clause
    );
    let mut params: Vec<Value> = Vec::with_capacity(filter.params.len() + 1);
    params.push(Value::Text(value.to_string()));
    params.extend(filter.params.iter().cloned());
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| r.get::<_, i64>(0))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(IndexError::from)
}

fn alias_rows(conn: &Connection, value: &str, filter: &Filter) -> Result<Vec<i64>> {
    let sql = format!(
        "SELECT DISTINCT n.rowid FROM note_alias a
         JOIN note n ON n.rowid = a.note_rowid
         WHERE a.norm_alias = ? AND {filter}",
        filter = filter.clause
    );
    let mut params: Vec<Value> = Vec::with_capacity(filter.params.len() + 1);
    params.push(Value::Text(value.to_string()));
    params.extend(filter.params.iter().cloned());
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| r.get::<_, i64>(0))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(IndexError::from)
}

/// `IN` lists are batched so a candidate set never exceeds SQLite's
/// parameter limit.
const IN_BATCH: usize = 200;

fn load_notes(conn: &Connection, rowids: &BTreeSet<i64>) -> Result<HashMap<i64, NoteRow>> {
    let mut out = HashMap::with_capacity(rowids.len());
    let ids: Vec<i64> = rowids.iter().copied().collect();
    for batch in ids.chunks(IN_BATCH) {
        let sql = format!(
            "SELECT rowid, id, partition_key, scope, revision, relative_path, title, kind,
                    status, effective_status, origin, fact_key, valid_from, valid_to,
                    updated_at, pinned, conflict, same_key_loser, source_forgotten,
                    superseded_by, authority, date
             FROM note WHERE rowid IN ({})",
            placeholders(batch.len())
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(batch.iter()), |r| {
            let status: String = r.get(9)?;
            let superseded: String = r.get(19)?;
            Ok(NoteRow {
                rowid: r.get(0)?,
                id: r.get(1)?,
                partition_key: r.get(2)?,
                scope: r.get(3)?,
                revision: r.get(4)?,
                relative_path: r.get(5)?,
                title: r.get(6)?,
                kind: r.get(7)?,
                status: r.get(8)?,
                effective_status: EffectiveStatus::parse(&status)
                    .unwrap_or(EffectiveStatus::Active),
                origin: r.get(10)?,
                fact_key: r.get(11)?,
                valid_from: r.get(12)?,
                valid_to: r.get(13)?,
                updated_at: r.get(14)?,
                pinned: r.get::<_, i64>(15)? != 0,
                conflict: r.get::<_, i64>(16)? != 0,
                same_key_loser: r.get::<_, i64>(17)? != 0,
                source_forgotten: r.get::<_, i64>(18)? != 0,
                superseded_by: serde_json::from_str(&superseded).unwrap_or_default(),
                authority: r.get(20)?,
                date: r.get(21)?,
            })
        })?;
        for row in rows {
            let row = row?;
            out.insert(row.rowid, row);
        }
    }
    Ok(out)
}

const CHUNK_COLUMNS: &str =
    "id, note_rowid, ordinal, heading, line_from, line_to, excerpt, title, aliases, tags, body";

fn chunk_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ChunkRow> {
    Ok(ChunkRow {
        id: r.get(0)?,
        note_rowid: r.get(1)?,
        ordinal: r.get::<_, i64>(2)? as u32,
        heading: r.get(3)?,
        line_from: r.get::<_, i64>(4)? as u32,
        line_to: r.get::<_, i64>(5)? as u32,
        excerpt: r.get(6)?,
        title: r.get(7)?,
        aliases: r.get(8)?,
        tags: r.get(9)?,
        body: r.get(10)?,
    })
}

fn load_chunks(conn: &Connection, ids: &[i64]) -> Result<Vec<ChunkRow>> {
    let mut out = Vec::with_capacity(ids.len());
    for batch in ids.chunks(IN_BATCH) {
        let sql = format!(
            "SELECT {CHUNK_COLUMNS} FROM chunk WHERE id IN ({})",
            placeholders(batch.len())
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(batch.iter()), chunk_from_row)?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

/// The first chunk of each note, for a note found only by an exact lookup.
fn load_first_chunks(conn: &Connection, note_rowids: &[i64]) -> Result<Vec<ChunkRow>> {
    let mut out = Vec::with_capacity(note_rowids.len());
    for batch in note_rowids.chunks(IN_BATCH) {
        let sql = format!(
            "SELECT {CHUNK_COLUMNS} FROM chunk WHERE ordinal = 0 AND note_rowid IN ({})",
            placeholders(batch.len())
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(batch.iter()), chunk_from_row)?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

// ============================================================================
// Ranking
// ============================================================================

/// The normalized text fields of one chunk, tokenized once.
struct ChunkText<'a> {
    fields: [&'a str; 4],
    tokens: [HashSet<&'a str>; 4],
    token_lists: [Vec<&'a str>; 4],
}

impl<'a> ChunkText<'a> {
    fn new(c: &'a ChunkRow) -> Self {
        let fields = [
            c.title.as_str(),
            c.aliases.as_str(),
            c.tags.as_str(),
            c.body.as_str(),
        ];
        let token_lists = fields.map(|f| query::tokens(f).collect::<Vec<_>>());
        let tokens = token_lists
            .iter()
            .map(|l| l.iter().copied().collect::<HashSet<_>>())
            .collect::<Vec<_>>()
            .try_into()
            .unwrap_or_else(|_| unreachable!("four fields"));
        Self {
            fields,
            tokens,
            token_lists,
        }
    }

    fn has_token(&self, term: &str) -> bool {
        self.tokens.iter().any(|set| set.contains(term))
    }

    fn contains(&self, text: &str) -> bool {
        self.fields.iter().any(|f| f.contains(text))
    }

    /// A literal as a phrase: a substring, or its tokens in sequence.
    fn has_phrase(&self, literal: &str) -> bool {
        if self.contains(literal) {
            return true;
        }
        let needle: Vec<&str> = query::tokens(literal).collect();
        if needle.is_empty() {
            return false;
        }
        self.token_lists
            .iter()
            .any(|list| list.windows(needle.len()).any(|w| w == needle.as_slice()))
    }
}

/// Per-note accumulated evidence.
struct NoteScore {
    reasons: BTreeSet<MatchReason>,
    terms: HashSet<String>,
    trigram_only: HashSet<String>,
    phrases: HashSet<String>,
    shorts: HashSet<String>,
    word_rel: f64,
    tri_rel: f64,
    /// `(coverage score, -ordinal)` of the best chunk, and the chunk.
    best: Option<(f64, i64, usize)>,
}

#[allow(clippy::too_many_arguments)]
fn rank(
    q: &SearchQuery,
    parsed: &ParsedQuery,
    words: &[String],
    filter: &Filter,
    notes: HashMap<i64, NoteRow>,
    chunk_rows: Vec<ChunkRow>,
    evidence: &HashMap<i64, ChunkEvidence>,
    exact: &HashMap<i64, ExactEvidence>,
    partial: bool,
    index_state: IndexState,
) -> SearchResults {
    let max_word = evidence
        .values()
        .filter_map(|e| e.word_rank)
        .map(|r| -r)
        .fold(0.0_f64, f64::max);
    let max_tri = evidence
        .values()
        .filter_map(|e| e.tri_rank)
        .map(|r| -r)
        .fold(0.0_f64, f64::max);

    let mut scores: HashMap<i64, NoteScore> = HashMap::new();
    for (i, chunk) in chunk_rows.iter().enumerate() {
        let Some(note) = notes.get(&chunk.note_rowid) else {
            continue;
        };
        let text = ChunkText::new(chunk);
        let entry = scores.entry(note.rowid).or_insert_with(|| NoteScore {
            reasons: BTreeSet::new(),
            terms: HashSet::new(),
            trigram_only: HashSet::new(),
            phrases: HashSet::new(),
            shorts: HashSet::new(),
            word_rel: 0.0,
            tri_rel: 0.0,
            best: None,
        });
        let mut coverage = 0.0;
        for word in words {
            let word = word.as_str();
            if text.has_token(word) {
                entry.reasons.insert(MatchReason::Term);
                entry.terms.insert(word.to_string());
                entry.trigram_only.remove(word);
                coverage += W_TERM;
            } else if word.chars().count() >= 3 && text.contains(word) {
                entry.reasons.insert(MatchReason::Trigram);
                if !entry.terms.contains(word) {
                    entry.trigram_only.insert(word.to_string());
                }
                coverage += W_TRIGRAM;
            }
        }
        for literal in parsed.literals.iter().map(|l| l.text.as_str()) {
            if text.has_phrase(literal) {
                entry.reasons.insert(MatchReason::Phrase);
                entry.phrases.insert(literal.to_string());
                coverage += W_PHRASE;
            }
        }
        for short in &parsed.shorts {
            if text.contains(short) {
                entry.shorts.insert(short.to_string());
                coverage += W_SHORT;
            }
        }
        let (wr, tr) = match evidence.get(&chunk.id) {
            Some(e) => (
                e.word_rank
                    .map(|r| -r / max_word.max(f64::MIN_POSITIVE))
                    .unwrap_or(0.0),
                e.tri_rank
                    .map(|r| -r / max_tri.max(f64::MIN_POSITIVE))
                    .unwrap_or(0.0),
            ),
            None => (0.0, 0.0),
        };
        entry.word_rel = entry.word_rel.max(wr);
        entry.tri_rel = entry.tri_rel.max(tr);
        let chunk_score = coverage + W_BM25_WORD * wr + W_BM25_TRIGRAM * tr;
        let key = (chunk_score, -(chunk.ordinal as i64));
        let better = match entry.best {
            None => true,
            Some((s, o, _)) => key > (s, o),
        };
        if better {
            entry.best = Some((chunk_score, -(chunk.ordinal as i64), i));
        }
    }

    let mut hits: Vec<SearchHit> = Vec::with_capacity(scores.len());
    for (rowid, s) in scores {
        let note = &notes[&rowid];
        let Some((_, _, chunk_index)) = s.best else {
            continue;
        };
        let chunk = &chunk_rows[chunk_index];
        let mut reasons = s.reasons;
        let mut score = W_TERM * s.terms.len() as f64
            + W_TRIGRAM * s.trigram_only.len() as f64
            + W_PHRASE * s.phrases.len() as f64
            + W_SHORT * s.shorts.len() as f64
            + W_BM25_WORD * s.word_rel
            + W_BM25_TRIGRAM * s.tri_rel
            + W_AUTHORITY * note.authority as f64;
        if let Some(e) = exact.get(&rowid) {
            if e.title {
                reasons.insert(MatchReason::Title);
                score += W_EXACT_TITLE;
            }
            if e.alias {
                reasons.insert(MatchReason::Alias);
                score += W_EXACT_ALIAS;
            }
            if e.fact_key {
                reasons.insert(MatchReason::FactKey);
                score += W_EXACT_FACT_KEY;
            }
        }
        let explicit = note.valid_from.is_some() || note.valid_to.is_some();
        let effective_now = note
            .valid_from
            .as_deref()
            .is_none_or(|from| from <= filter.instant.as_str())
            && note
                .valid_to
                .as_deref()
                .is_none_or(|to| to > filter.instant.as_str());
        if explicit && effective_now {
            score += W_VALID_NOW;
        }
        // A trigram-only hit only ever carries the trigram reason; a word
        // matched whole somewhere in the note drops it.
        if reasons.contains(&MatchReason::Trigram) && s.trigram_only.is_empty() {
            reasons.remove(&MatchReason::Trigram);
        }
        hits.push(SearchHit {
            id: note.id.clone(),
            revision: note.revision.clone(),
            partition_key: note.partition_key.clone(),
            scope: note.scope.clone(),
            relative_path: note.relative_path.clone(),
            title: note.title.clone(),
            kind: note.kind.clone(),
            status: note.status.clone(),
            effective_status: note.effective_status,
            origin: note.origin.clone(),
            fact_key: note.fact_key.clone(),
            valid_from: note.valid_from.clone(),
            valid_to: note.valid_to.clone(),
            upcoming: note
                .valid_from
                .as_deref()
                .is_some_and(|from| from > filter.instant.as_str()),
            ended: note
                .valid_to
                .as_deref()
                .is_some_and(|to| to <= filter.instant.as_str()),
            date: note.date.clone(),
            updated_at: note.updated_at.clone(),
            pinned: note.pinned,
            conflict: note.conflict,
            same_key_loser: note.same_key_loser,
            source_forgotten: note.source_forgotten,
            superseded_by: note.superseded_by.clone(),
            excerpt: chunk.excerpt.clone(),
            heading: chunk.heading.clone(),
            line_range: (chunk.line_from, chunk.line_to),
            chunk_ordinal: chunk.ordinal,
            matches: reasons.into_iter().collect(),
            score,
        });
    }
    // Score, then newest `updated_at`, then id — deterministic to the end.
    // A date order (design 09 §13.1) sorts every match by its date first and
    // keeps that same order among notes of one date.
    let by_score = |a: &SearchHit, b: &SearchHit| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.id.cmp(&b.id))
    };
    match q.order {
        SearchOrder::Relevance => hits.sort_by(by_score),
        SearchOrder::Newest => {
            hits.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| by_score(a, b)))
        }
        SearchOrder::Oldest => {
            hits.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| by_score(a, b)))
        }
    }
    hits.truncate(q.limit);
    let count = hits.len();
    SearchResults {
        hits,
        count,
        partial,
        index_state,
    }
}
