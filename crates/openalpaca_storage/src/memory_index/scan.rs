//! The bounded file scan a search falls back to while the index is being
//! rebuilt (design §5, *Freshness without a watcher*: "search meanwhile
//! serves a bounded scoped file scan with `index_state: rebuilding`"; scale
//! slice S6).
//!
//! The caller reads the notes from their files — the partitions it
//! resolved, at most as many as its bound allows — and hands them over as
//! [`IndexRecord`]s, exactly what it would have indexed. The scan applies
//! the index's own filters (design §5 step 5) and its own ranking (§5 step
//! 6, [`super::search`]'s `rank`) to them, in memory: no SQL, no FTS, so
//! it waits on nothing the rebuild holds. A term is matched in the note's
//! normalized title, aliases, tags and body chunks with the index's own
//! evidence (design 09 §6.5.3), through the same `ChunkText`: as a whole
//! token of the text split at script boundaries (`term`), else as a token's
//! beginning (`prefix`: `prefer` in `prefers`), else as a substring where
//! the trigram tables find one (`trigram`: in a title, an alias or a tag,
//! or a term carrying an unsegmented character in unsegmented text); a
//! literal as a phrase; a one- or two-character term by the substring rule
//! — and the whole query is compared with the title, the aliases and the
//! `fact_key`. So an answer given during a rebuild finds what the index
//! finds. The scan reads every note, so it has no cap and needs no
//! whole-word query. What it cannot have is BM25 (no index statistics), so
//! it orders hits by the same term and authority weights with no relevance
//! tie-break but recency; and every answer says `rebuilding`, and `partial`
//! when the caller left notes out.

use std::collections::HashMap;

use super::search::{ChunkEvidence, ChunkRow, ChunkText, ExactEvidence, Filter, NoteRow, rank};
use super::{
    EffectiveStatus, IndexError, IndexRecord, IndexState, IndexedNote, MAX_LIMIT, MemoryIndex,
    Result, SearchMode, SearchQuery, SearchResults, authority_rank, canon_timestamp,
    canonicalize_timestamp, chunker, query,
};

impl MemoryIndex {
    /// Answers `q` from `records` — notes the caller read from their files
    /// — with the index's filters and ranking (module docs). `truncated`
    /// says the caller's bound left notes out, and sets `partial`. The
    /// results always say [`IndexState::Rebuilding`]. Takes no lock.
    pub fn scan_search(
        &self,
        records: &[IndexRecord],
        q: &SearchQuery,
        truncated: bool,
    ) -> Result<SearchResults> {
        if q.limit == 0 || q.limit > MAX_LIMIT {
            return Err(IndexError::InvalidInput(format!(
                "limit must be 1–{MAX_LIMIT}, got {}",
                q.limit
            )));
        }
        let parsed = query::parse_query(&q.query)?;
        let filter = Filter::build(q, self.now());
        let empty = SearchResults {
            hits: Vec::new(),
            count: 0,
            partial: truncated,
            index_state: IndexState::Rebuilding,
        };
        if q.partition_keys.is_empty() {
            return Ok(empty);
        }

        // The eligible notes and their chunks, as the index would hold them.
        let mut notes: HashMap<i64, NoteRow> = HashMap::new();
        let mut aliases: HashMap<i64, Vec<String>> = HashMap::new();
        let mut chunks: Vec<ChunkRow> = Vec::new();
        for (i, record) in records.iter().enumerate() {
            let rowid = i as i64;
            let Some(row) = eligible_row(rowid, record, q, &filter.instant) else {
                continue;
            };
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
            let norm_title = query::normalize_exact(&record.title);
            let mut body_chunks = chunker::chunk_body(&record.body);
            if body_chunks.is_empty() {
                body_chunks.push(chunker::Chunk {
                    ordinal: 0,
                    heading: None,
                    line_from: 0,
                    line_to: 0,
                    text: String::new(),
                });
            }
            for chunk in body_chunks {
                let (line_from, line_to) = if chunk.line_from == 0 {
                    (0, 0)
                } else {
                    (
                        chunk.line_from + record.body_line_offset,
                        chunk.line_to + record.body_line_offset,
                    )
                };
                chunks.push(ChunkRow {
                    id: chunks.len() as i64,
                    note_rowid: rowid,
                    ordinal: chunk.ordinal,
                    heading: chunk.heading,
                    line_from,
                    line_to,
                    body: query::normalize(&chunk.text),
                    title: norm_title.clone(),
                    aliases: norm_aliases.join("\n"),
                    tags: norm_tags.join("\n"),
                });
            }
            aliases.insert(rowid, norm_aliases);
            notes.insert(rowid, row);
        }

        // A literal's parts are routed only when the literal itself matches
        // no eligible note — the index's rule.
        let mut words: Vec<String> = parsed.words.clone();
        for literal in &parsed.literals {
            let hit = chunks
                .iter()
                .any(|c| ChunkText::new(c).has_phrase(&literal.text));
            if !hit {
                for part in &literal.parts {
                    if !words.contains(part) {
                        words.push(part.clone());
                    }
                }
            }
        }

        // The chunks some term matches, and the notes the whole query names.
        let mut evidence: HashMap<i64, ChunkEvidence> = HashMap::new();
        for chunk in &chunks {
            let text = ChunkText::new(chunk);
            let matched = words
                .iter()
                .any(|w| text.has_token(w) || text.has_prefix(w) || text.contains_indexed(w))
                || parsed.literals.iter().any(|l| text.has_phrase(&l.text))
                || parsed.shorts.iter().any(|s| text.contains(s));
            if matched {
                evidence.insert(
                    chunk.id,
                    ChunkEvidence {
                        note_rowid: chunk.note_rowid,
                        ..ChunkEvidence::default()
                    },
                );
            }
        }
        let mut exact: HashMap<i64, ExactEvidence> = HashMap::new();
        if !parsed.normalized.is_empty() {
            for (rowid, note) in &notes {
                let e = ExactEvidence {
                    title: query::normalize_exact(&note.title) == parsed.normalized,
                    alias: aliases[rowid].contains(&parsed.normalized),
                    fact_key: note.fact_key.as_deref() == Some(parsed.normalized.as_str()),
                };
                if e.title || e.alias || e.fact_key {
                    exact.insert(*rowid, e);
                }
            }
        }
        if evidence.is_empty() && exact.is_empty() {
            return Ok(empty);
        }
        // The matching chunks, and a note found only by an exact lookup
        // stands on its first chunk — what the index loads for one.
        let matched_notes: std::collections::HashSet<i64> =
            evidence.values().map(|e| e.note_rowid).collect();
        let chunk_rows: Vec<ChunkRow> = chunks
            .into_iter()
            .filter(|c| {
                evidence.contains_key(&c.id)
                    || (c.ordinal == 0
                        && exact.contains_key(&c.note_rowid)
                        && !matched_notes.contains(&c.note_rowid))
            })
            .collect();
        notes.retain(|rowid, _| matched_notes.contains(rowid) || exact.contains_key(rowid));
        Ok(rank(
            q,
            &parsed,
            &words,
            &filter,
            notes,
            chunk_rows,
            &evidence,
            &exact,
            truncated,
            IndexState::Rebuilding,
        ))
    }

    /// The pinned notes of `records` that [`MemoryIndex::list_pinned`]
    /// would return for `partition_keys`, in its order — the file scan's
    /// design §5 step 2 while the index is rebuilt. Takes no lock.
    pub fn scan_pinned(
        &self,
        records: &[IndexRecord],
        partition_keys: &[String],
    ) -> Vec<IndexedNote> {
        let now = canon_timestamp(&self.now());
        let canon = |v: &Option<String>, field: &str| {
            v.as_deref()
                .and_then(|v| canonicalize_timestamp(field, v).ok())
        };
        let mut pinned: Vec<(String, IndexedNote)> = records
            .iter()
            .filter(|r| {
                r.pinned
                    && r.effective_status == EffectiveStatus::Active
                    && partition_keys.contains(&r.partition_key)
                    && canon(&r.valid_to, "valid_to").is_none_or(|to| to > now)
            })
            .map(|r| {
                let chunks = chunker::chunk_body(&r.body).len().max(1) as u32;
                (
                    canonicalize_timestamp("updated_at", &r.updated_at).unwrap_or_default(),
                    IndexedNote {
                        id: r.id.clone(),
                        partition_key: r.partition_key.clone(),
                        revision: r.revision.clone(),
                        relative_path: r.relative_path.clone(),
                        effective_status: r.effective_status,
                        pinned: true,
                        file: r.file,
                        chunks,
                        upcoming: canon(&r.valid_from, "valid_from").is_some_and(|f| f > now),
                    },
                )
            })
            .collect();
        pinned.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        pinned.into_iter().map(|(_, n)| n).collect()
    }
}

/// The note as the index would hold it, when it passes the filters of
/// design §5 step 5 — the same as `Filter::build`'s SQL, in Rust. `None`
/// for a note the search leaves out (or whose timestamps do not parse,
/// which the index would have refused to hold).
fn eligible_row(rowid: i64, r: &IndexRecord, q: &SearchQuery, instant: &str) -> Option<NoteRow> {
    if !q.partition_keys.contains(&r.partition_key) {
        return None;
    }
    if q.kind.as_ref().is_some_and(|k| *k != r.kind) {
        return None;
    }
    let status_ok = if q.historical {
        matches!(
            r.effective_status,
            EffectiveStatus::Active | EffectiveStatus::Superseded | EffectiveStatus::Archived
        )
    } else {
        r.effective_status == EffectiveStatus::Active
    };
    if !status_ok {
        return None;
    }
    let canon = |field: &str, v: &Option<String>| -> Option<Option<String>> {
        match v {
            None => Some(None),
            Some(v) => canonicalize_timestamp(field, v).ok().map(Some),
        }
    };
    let valid_from = canon("valid_from", &r.valid_from)?;
    let valid_to = canon("valid_to", &r.valid_to)?;
    let updated_at = canonicalize_timestamp("updated_at", &r.updated_at).ok()?;
    let created_at = canonicalize_timestamp("created_at", &r.created_at).ok()?;
    let observed_at = canon("observed_at", &r.observed_at)?;
    if q.as_of.is_some() {
        let begun = valid_from.as_deref().is_none_or(|f| f <= instant);
        let running = valid_to.as_deref().is_none_or(|t| t > instant);
        if !(begun && running) {
            return None;
        }
    } else if !q.historical && valid_to.as_deref().is_some_and(|t| t <= instant) {
        return None;
    }
    if q.mode == SearchMode::Automatic
        && (r.same_key_loser
            || r.source_forgotten
            || (r.kind == "episode" && r.origin == "extracted_other"))
    {
        return None;
    }
    let date = observed_at.clone().unwrap_or_else(|| created_at.clone());
    if q
        .date_from
        .is_some_and(|from| date < canon_timestamp(&from))
        || q.date_to.is_some_and(|to| date > canon_timestamp(&to))
    {
        return None;
    }
    Some(NoteRow {
        rowid,
        id: r.id.clone(),
        partition_key: r.partition_key.clone(),
        scope: r.scope.clone(),
        revision: r.revision.clone(),
        relative_path: r.relative_path.clone(),
        title: r.title.clone(),
        kind: r.kind.clone(),
        status: r.status.clone(),
        effective_status: r.effective_status,
        origin: r.origin.clone(),
        fact_key: r.fact_key.clone(),
        valid_from,
        valid_to,
        updated_at,
        date,
        pinned: r.pinned,
        conflict: r.conflict,
        same_key_loser: r.same_key_loser,
        source_forgotten: r.source_forgotten,
        superseded_by: r.superseded_by.clone(),
        authority: authority_rank(&r.origin),
    })
}
