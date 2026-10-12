//! `memory_timeline` (design 09 §13.1, MD-14): what surrounds one note in
//! time — or the first hit of a search, or an instant — as index rows in
//! time order. A note leaves recall when it is superseded, archived or its
//! window has ended, and a timeline lists it anyway, marked by `status`, so
//! "what did I decide before the June move" has one tool; a candidate (a
//! suggestion included) is never listed. Every row is re-read from its file
//! before it is served, as a search hit is (design §5 step 7).

use super::recall::{Verified, verify};
use super::{
    IndexRow, MemoryAccessContext, MemoryError, MemoryService, MemoryUsed, RowStatus, SearchInput,
    TIMELINE_DEFAULT_SIDE, TIMELINE_MAX_SIDE, TimelineAnchor, TimelineInput, TimelineOutput,
    UsedReason, approx_tokens, scope_of,
};
use openalpaca_storage::memory_files::{NoteId, Partition};
use openalpaca_storage::memory_index::{self as index, TimelinePoint, TimelineQuery, TimelineRow};
use std::collections::HashSet;

/// A row's place relative to the anchor — its `reason` on the wire.
const BEFORE: &str = "before";
const ANCHOR: &str = "anchor";
const AFTER: &str = "after";

fn side(value: Option<usize>, name: &str) -> Result<usize, MemoryError> {
    let n = value.unwrap_or(TIMELINE_DEFAULT_SIDE);
    if n > TIMELINE_MAX_SIDE {
        return Err(MemoryError::invalid(format!(
            "{name} must be between 0 and {TIMELINE_MAX_SIDE}"
        )));
    }
    Ok(n)
}

fn map_status(s: index::EffectiveStatus) -> openalpaca_storage::memory_files::EffectiveStatus {
    use openalpaca_storage::memory_files::EffectiveStatus as E;
    match s {
        index::EffectiveStatus::Active => E::Active,
        index::EffectiveStatus::Superseded => E::Superseded,
        index::EffectiveStatus::Archived => E::Archived,
        index::EffectiveStatus::Candidate => E::Candidate,
    }
}

impl MemoryService {
    /// `memory_timeline` (design 09 §13.1): the notes around an anchor in
    /// the order of their dates — `observed_at`, else `created_at` — the
    /// nearest `before` earlier and `after` later ones (default 5, at most
    /// 20 each), over every partition the context may read.
    ///
    /// A note anchor must be one of the owner's notes (`not_found`
    /// otherwise, exactly as `memory_read` answers) and not a candidate
    /// (`invalid_input`); a query anchor is the first hit of a historical
    /// search, and a query nothing matches draws no timeline; a date anchor
    /// lists the notes dated before it, then those dated at or after it.
    pub fn timeline(
        &self,
        ctx: &MemoryAccessContext,
        input: TimelineInput,
    ) -> Result<TimelineOutput, MemoryError> {
        Self::require(ctx.capabilities.read, "memory_read")?;
        let before = side(input.before, "before")?;
        let after = side(input.after, "after")?;
        // The timeline is the index's order, so it has no file-scan answer;
        // and a model waits on it, so it never waits out a rebuild either
        // (the L8 product slice's P8): while the index is rebuilt — or when a
        // rebuild starts under the read — it says so at once, a refusal the
        // model relays, and search and recall answer from the files.
        let (output, revisions) = self.read_index_or(
            || {
                let resolved = self.resolve(ctx)?;
                let partitions: Vec<&Partition> = resolved.all();
                for p in &partitions {
                    if self.touch_serving(p)?.is_none() {
                        return Ok(None);
                    }
                }
                let keys: Vec<String> = partitions
                    .iter()
                    .map(|p| p.partition_key().to_string())
                    .collect();
                let by_key = |key: &str| {
                    partitions
                        .iter()
                        .copied()
                        .find(|p| p.partition_key() == key)
                };

                // One re-run when a re-read found the index out of date, so the
                // second pass reads a fresh index (as a search does, design §5
                // step 7).
                let mut output = None;
                for pass in 0..2 {
                    let (point, anchor_note, anchor_date) = match &input.anchor {
                        TimelineAnchor::Note(id) => {
                            let key = self.timeline_note(&keys, *id)?;
                            (
                                TimelinePoint::Note {
                                    partition_key: key,
                                    id: id.to_string(),
                                },
                                Some(*id),
                                None,
                            )
                        }
                        TimelineAnchor::Query(query) => {
                            let hits = self.search_unrecorded(
                                ctx,
                                SearchInput {
                                    query: query.clone(),
                                    historical: true,
                                    ..Default::default()
                                },
                            )?;
                            let Some(first) = hits.results.first() else {
                                let nothing = TimelineOutput {
                                    anchor_note: None,
                                    anchor_date: None,
                                    rows: Vec::new(),
                                    index_state: hits.index_state,
                                };
                                return Ok(Some((nothing, Vec::new())));
                            };
                            let key = self.timeline_note(&keys, first.id)?;
                            (
                                TimelinePoint::Note {
                                    partition_key: key,
                                    id: first.id.to_string(),
                                },
                                Some(first.id),
                                None,
                            )
                        }
                        TimelineAnchor::Date(at) => (TimelinePoint::Instant(*at), None, Some(*at)),
                    };
                    let found = self.indexed(self.index.timeline(&TimelineQuery {
                        partition_keys: keys.clone(),
                        point,
                        before,
                        after,
                    }))?;
                    let mut dirty: HashSet<String> = HashSet::new();
                    let listed = found
                        .before
                        .iter()
                        .map(|r| (r, BEFORE))
                        .chain(found.anchor.iter().map(|r| (r, ANCHOR)))
                        .chain(found.after.iter().map(|r| (r, AFTER)));
                    let mut rows = Vec::new();
                    let mut revisions = Vec::new();
                    for (row, reason) in listed {
                        let Some(p) = by_key(&row.partition_key) else {
                            continue;
                        };
                        if let Some((served, revision)) =
                            Self::timeline_row(p, row, reason, &mut dirty)
                        {
                            rows.push(served);
                            revisions.push(revision);
                        }
                    }
                    let had_dirty = !dirty.is_empty();
                    for key in dirty {
                        if let Some(p) = by_key(&key) {
                            self.reindex(p)?;
                        }
                    }
                    output = Some((
                        TimelineOutput {
                            anchor_note,
                            anchor_date,
                            rows,
                            index_state: found.index_state,
                        },
                        revisions,
                    ));
                    if !had_dirty || pass == 1 {
                        break;
                    }
                }
                Ok(Some(output.expect("one pass always runs")))
            },
            || Err(MemoryError::IndexRebuilding),
        )?;
        // A listed row is fetched through a memory tool, as a search hit is
        // (design §7a `memory_used` reason `tool`), at the revision re-read.
        if ctx.surface == super::Surface::ModelTool {
            let used: Vec<MemoryUsed> = output
                .rows
                .iter()
                .zip(revisions)
                .map(|(r, revision)| MemoryUsed {
                    id: r.id,
                    revision,
                    reason: UsedReason::Tool,
                })
                .collect();
            self.record_used(ctx, &used);
        }
        Ok(output)
    }

    /// The partition key of the anchor note `id`: one of the owner's notes,
    /// and not a candidate — a timeline lists none, so it is drawn around
    /// none.
    fn timeline_note(&self, keys: &[String], id: NoteId) -> Result<String, MemoryError> {
        for key in keys {
            if let Some(note) = self.indexed(self.index.catalog_note(key, &id.to_string()))? {
                if note.effective_status == index::EffectiveStatus::Candidate {
                    return Err(MemoryError::invalid(format!(
                        "note {id} is a candidate, and a timeline lists no candidates: confirm it \
                         first, or draw the timeline around another note or a date"
                    )));
                }
                return Ok(key.clone());
            }
        }
        Err(MemoryError::NotFound(id))
    }

    /// One listed row, re-read from its file: the title, the kind and the
    /// date are the file's, the status the index derived. A file that is
    /// gone, or no longer the note the index named, is not served, and its
    /// partition is re-indexed.
    fn timeline_row(
        p: &Partition,
        row: &TimelineRow,
        reason: &str,
        dirty: &mut HashSet<String>,
    ) -> Option<(IndexRow, String)> {
        let id = NoteId::parse(&row.id).ok()?;
        let (note, revision, stale) = match verify(p, &row.relative_path, id, &row.revision) {
            Verified::Fresh {
                note,
                revision,
                stale,
                ..
            } => (note, revision, stale),
            Verified::Gone => {
                dirty.insert(row.partition_key.clone());
                return None;
            }
        };
        if stale {
            dirty.insert(row.partition_key.clone());
        }
        let served = IndexRow {
            id,
            date: note.observed_at.unwrap_or(note.created_at),
            kind: note.kind,
            scope: scope_of(p),
            status: RowStatus::of(map_status(row.effective_status), row.ended),
            title: note.title.clone(),
            reason: reason.to_string(),
            size: approx_tokens(note.body.len()),
            conflict: row.conflict,
        };
        Some((served, revision))
    }
}
