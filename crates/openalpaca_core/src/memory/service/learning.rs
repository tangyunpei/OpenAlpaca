//! The reads background learning needs from the partitions (Learning v2,
//! design 09 §1.4, §2.3), on the `Extraction` surface only.
//!
//! None is tool-reachable and none records anything — not in the
//! recently-used ring, not in a turn's `memory_used`, not on the bus:
//! `Extraction`'s capabilities stay write-only (design §7a; 09 §11 amends the
//! Extraction row to "reads through `learning_view` only"). The lookups read
//! the index and re-read only the files they return (design §5 step 7);
//! none parses a whole partition unless the stat scan finds it changed.

use super::recall::{Verified, verify};
use super::{
    KnownNote, LearningView, MemoryAccessContext, MemoryError, MemoryService, Surface, scope_of,
};
use openalpaca_storage::memory_files::{
    EffectiveStatus, NoteId, NoteKind, NoteStatus, Origin, Partition, PartitionHealth, SourceType,
    is_conversation_summary, knowledge_text,
};
use openalpaca_storage::memory_index::{self as index, SearchHit, SearchMode, SearchQuery};
use std::collections::{HashMap, HashSet};

/// The most notes one lookup offers the prompt (design 09 §2.3 step 3).
pub const LEARNING_MAX_NOTES: usize = 20;
/// The most keys one lookup offers the prompt (design 09 §2.3 step 4).
pub const LEARNING_MAX_KEYS: usize = 150;
/// The hits each query keeps before the union (design 09 §2.3 step 3).
pub const LEARNING_HITS_PER_QUERY: usize = 5;
/// How many hits each lookup query asks the index for — the search's old
/// cap, kept when the explicit search's rose to 50 (design 09 §13.1), so
/// what the extractor is shown does not change with it.
const LOOKUP_CANDIDATES: usize = 20;

/// A catalog row, as the index named it, before its file is re-read.
struct Named<'a> {
    id: &'a str,
    relative_path: &'a str,
    revision: &'a str,
    origin: &'a str,
    effective_status: index::EffectiveStatus,
}

impl MemoryService {
    fn require_extraction(ctx: &MemoryAccessContext) -> Result<(), MemoryError> {
        if ctx.surface != Surface::Extraction {
            return Err(MemoryError::invalid(
                "this read is background learning's own; no other surface may make it",
            ));
        }
        Ok(())
    }

    /// Refuses when a partition the chunk would write to is quarantined
    /// (design §2): checked **before** the model call, so no cost is spent on
    /// a window that cannot be written. The window waits for
    /// `memory doctor --repair`.
    pub(crate) fn learning_preflight(&self, ctx: &MemoryAccessContext) -> Result<(), MemoryError> {
        Self::require_extraction(ctx)?;
        let resolved = self.resolve(ctx)?;
        for partition in resolved.all() {
            if let PartitionHealth::Quarantined(reason) = partition.health() {
                return Err(MemoryError::Quarantined {
                    partition_key: partition.partition_key().to_string(),
                    reason,
                });
            }
        }
        Ok(())
    }

    /// Which of `keys` (`message:<id>`) an explicit forget suppressed in
    /// either partition of `ctx` (design §1.4(d), §6 *Forget contract*): a
    /// replayed window must never cite them again, or a forgotten note would
    /// come back.
    pub(crate) fn suppressed_sources(
        &self,
        ctx: &MemoryAccessContext,
        keys: &[String],
    ) -> Result<HashSet<String>, MemoryError> {
        Self::require_extraction(ctx)?;
        if keys.is_empty() {
            return Ok(HashSet::new());
        }
        let resolved = self.resolve(ctx)?;
        let snapshots: Vec<_> = resolved
            .all()
            .into_iter()
            .filter_map(|p| p.snapshot())
            .collect();
        Ok(keys
            .iter()
            .filter(|k| snapshots.iter().any(|s| s.is_source_suppressed(k)))
            .cloned()
            .collect())
    }

    /// How far the conversation summaries already cover these message rows
    /// (design 09 §13.3): the newest of them any summary cites, or `None`
    /// when none cites any. A summary cites its window's last row, so after a
    /// crash between a summary's write and its episode cursor's advance the
    /// re-run passes exactly the rows it covers and writes no second one.
    /// Only the `Extraction` surface may ask; it records nothing.
    pub(crate) fn summary_coverage(
        &self,
        ctx: &MemoryAccessContext,
        message_ids: &[i64],
    ) -> Result<Option<i64>, MemoryError> {
        Self::require_extraction(ctx)?;
        if message_ids.is_empty() {
            return Ok(None);
        }
        let resolved = self.resolve(ctx)?;
        let partitions: Vec<&Partition> = resolved
            .all()
            .into_iter()
            .filter(|p| !matches!(p.health(), PartitionHealth::Quarantined(_)))
            .collect();
        for p in &partitions {
            self.touch(p)?;
        }
        let keys: Vec<String> = partitions
            .iter()
            .map(|p| p.partition_key().to_string())
            .collect();
        let source_keys: Vec<String> = message_ids
            .iter()
            .map(|id| format!("message:{id}"))
            .collect();
        let wanted: HashSet<i64> = message_ids.iter().copied().collect();
        let mut dirty: HashSet<String> = HashSet::new();
        let mut newest: Option<i64> = None;
        for row in self.index.notes_citing(&keys, &source_keys)? {
            if !is_summary_row(&row.kind, &row.origin, &row.status) {
                continue;
            }
            let (Some(p), Ok(id)) = (
                partitions
                    .iter()
                    .find(|p| p.partition_key() == row.partition_key),
                NoteId::parse(&row.id),
            ) else {
                continue;
            };
            match verify(p, &row.relative_path, id, &row.revision) {
                Verified::Fresh { note, stale, .. } => {
                    if stale {
                        dirty.insert(row.partition_key.clone());
                    }
                    let cited = note
                        .sources
                        .iter()
                        .filter(|s| s.kind == SourceType::Message)
                        .filter_map(|s| s.id.parse::<i64>().ok())
                        .filter(|n| wanted.contains(n))
                        .max();
                    newest = newest.max(cited);
                }
                Verified::Gone => {
                    dirty.insert(row.partition_key.clone());
                }
            }
        }
        for key in dirty {
            if let Some(p) = partitions.iter().find(|p| p.partition_key() == key) {
                self.reindex(p, true)?;
            }
        }
        Ok(newest)
    }

    /// Learning's known-notes lookup for one chunk (design 09 §2.3). Only
    /// the `Extraction` surface may make it; it records nothing.
    ///
    /// 1. The partitions are the context's: home and, when the chunk's rows
    ///    carry a root, that project. A quarantined one contributes nothing.
    /// 2. **This window's notes**: through the index's `note_source`, every
    ///    note citing a `message:<id>` of `chunk_message_ids` whose origin is
    ///    `extracted_user` or `extracted_other` and which is active or a
    ///    candidate — the re-run guard's input (§1.4(b)). A conversation
    ///    summary is not one of them, here or in step 3 (design 09 §13.3).
    /// 3. **Search**: each of `queries` through the index search
    ///    `memory_search` uses, its top five effectively active hits kept —
    ///    recallable or not, so a same-key loser, a note whose sources were
    ///    forgotten and one whose validity has ended all count — and the
    ///    union ranked by each note's best score. This window's active notes
    ///    come first; at most 20 are returned.
    /// 4. **Keys**: the 150 most recently updated distinct `fact_key`s of
    ///    effectively active notes other than a conversation summary, from
    ///    the index.
    ///
    /// Every returned note is re-read and re-hashed first; one that changed
    /// behind the service's back is reported with origin `external` (the
    /// snapshot no longer vouches for it), and its partition is re-indexed.
    pub fn learning_view(
        &self,
        ctx: &MemoryAccessContext,
        queries: &[String],
        chunk_message_ids: &[i64],
    ) -> Result<LearningView, MemoryError> {
        Self::require_extraction(ctx)?;
        let resolved = self.resolve(ctx)?;
        let partitions: Vec<&Partition> = resolved
            .all()
            .into_iter()
            .filter(|p| !matches!(p.health(), PartitionHealth::Quarantined(_)))
            .collect();
        for p in &partitions {
            self.touch(p)?;
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
        let mut dirty: HashSet<String> = HashSet::new();

        // Step 2: this window's learned notes, active and candidate.
        let source_keys: Vec<String> = chunk_message_ids
            .iter()
            .map(|id| format!("message:{id}"))
            .collect();
        let mut from_this_window: Vec<KnownNote> = Vec::new();
        for row in self.index.notes_citing(&keys, &source_keys)? {
            if !matches!(row.origin.as_str(), "extracted_user" | "extracted_other")
                || !matches!(
                    row.effective_status,
                    index::EffectiveStatus::Active | index::EffectiveStatus::Candidate
                )
                || is_summary_row(&row.kind, &row.origin, &row.status)
            {
                continue;
            }
            let Some(p) = by_key(&row.partition_key) else {
                continue;
            };
            let named = Named {
                id: &row.id,
                relative_path: &row.relative_path,
                revision: &row.revision,
                origin: &row.origin,
                effective_status: row.effective_status,
            };
            if let Some(note) = known_note(p, &named, true, &mut dirty) {
                from_this_window.push(note);
            }
        }

        // Step 3: the search, over each query; the union by best score.
        let mut best: HashMap<String, (f64, SearchHit)> = HashMap::new();
        for query in queries {
            let query = query.trim();
            if query.is_empty() || keys.is_empty() {
                continue;
            }
            let mut q = SearchQuery::new(query.to_string(), keys.clone());
            q.limit = LOOKUP_CANDIDATES;
            // Superseded and archived notes come back too and are dropped
            // below: `historical` is what lifts the validity filter, so a
            // note whose window has ended still counts.
            q.historical = true;
            q.mode = SearchMode::Explicit;
            let results = match self.index.search(&q) {
                Ok(r) => r,
                Err(index::IndexError::InvalidInput(_)) => continue,
                Err(e) => return Err(e.into()),
            };
            for hit in results
                .hits
                .into_iter()
                .filter(|h| h.effective_status == index::EffectiveStatus::Active)
                .filter(|h| !is_summary_row(&h.kind, &h.origin, &h.status))
                .take(LEARNING_HITS_PER_QUERY)
            {
                match best.get(&hit.id) {
                    Some((score, _)) if *score >= hit.score => {}
                    _ => {
                        best.insert(hit.id.clone(), (hit.score, hit));
                    }
                }
            }
        }
        let mut ranked: Vec<(f64, SearchHit)> = best.into_values().collect();
        ranked.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.id.cmp(&b.1.id))
        });

        let mut notes: Vec<KnownNote> = from_this_window
            .iter()
            .filter(|n| n.effective_status == EffectiveStatus::Active)
            .cloned()
            .collect();
        for (_, hit) in &ranked {
            if notes.len() >= LEARNING_MAX_NOTES {
                break;
            }
            if notes.iter().any(|n| n.id.to_string() == hit.id) {
                continue;
            }
            let Some(p) = by_key(&hit.partition_key) else {
                continue;
            };
            let named = Named {
                id: &hit.id,
                relative_path: &hit.relative_path,
                revision: &hit.revision,
                origin: &hit.origin,
                effective_status: hit.effective_status,
            };
            if let Some(note) = known_note(p, &named, false, &mut dirty)
                && note.effective_status == EffectiveStatus::Active
            {
                notes.push(note);
            }
        }
        notes.truncate(LEARNING_MAX_NOTES);

        // Step 4: the keys.
        let known_keys = self.index.fact_keys(&keys, LEARNING_MAX_KEYS)?;

        for key in dirty {
            if let Some(p) = by_key(&key) {
                self.reindex(p, true)?;
            }
        }
        Ok(LearningView {
            notes,
            from_this_window,
            keys: known_keys,
        })
    }

    /// The winner of each of `fact_keys` among the chunk's partitions
    /// (design 09 §2.5, ruling LR11): the effectively active note with that
    /// key that is not a same-key loser, looked up in the project partition
    /// first and then the home one, and verified against its file. A key
    /// no note carries is absent from the answer, and so is one only a
    /// conversation summary holds (design 09 §13.3). Only the `Extraction`
    /// surface may ask; it records nothing.
    pub fn learning_key_targets(
        &self,
        ctx: &MemoryAccessContext,
        fact_keys: &[String],
    ) -> Result<HashMap<String, KnownNote>, MemoryError> {
        Self::require_extraction(ctx)?;
        let mut out = HashMap::new();
        if fact_keys.is_empty() {
            return Ok(out);
        }
        let resolved = self.resolve(ctx)?;
        let mut partitions: Vec<&Partition> = Vec::new();
        if let Some(p) = &resolved.project {
            partitions.push(p);
        }
        partitions.push(&resolved.home);
        partitions.retain(|p| !matches!(p.health(), PartitionHealth::Quarantined(_)));
        for p in &partitions {
            self.touch(p)?;
        }
        let mut dirty: HashSet<String> = HashSet::new();
        for key in fact_keys {
            if out.contains_key(key) {
                continue;
            }
            for p in &partitions {
                let Some(row) = self.index.fact_key_winner(p.partition_key(), key)? else {
                    continue;
                };
                let named = Named {
                    id: &row.id,
                    relative_path: &row.relative_path,
                    revision: &row.revision,
                    origin: &row.origin,
                    effective_status: row.effective_status,
                };
                if let Some(note) = known_note(p, &named, false, &mut dirty)
                    && note.effective_status == EffectiveStatus::Active
                {
                    out.insert(key.clone(), note);
                    break;
                }
            }
        }
        for key in dirty {
            if let Some(p) = partitions.iter().find(|p| p.partition_key() == key) {
                self.reindex(p, true)?;
            }
        }
        Ok(out)
    }
}

/// Whether an index row is a conversation summary (design 09 §13.3): one
/// never enters the extractor's prompt, as it never enters any prompt on its
/// own — neither shown nor counted among a window's learned notes. `status`
/// is the row's stored one: an `extracted_other` episode candidate is not a
/// summary.
fn is_summary_row(kind: &str, origin: &str, status: &str) -> bool {
    matches!(
        (NoteKind::parse(kind), Origin::parse(origin), NoteStatus::parse(status)),
        (Some(k), Some(o), Some(s)) if is_conversation_summary(k, o, s)
    )
}

/// One named note, re-read and re-hashed (design §5 step 7). `None` when the
/// file is gone, tombstoned, unparsable, archived or another note now; its
/// partition is then marked for re-indexing. A file whose bytes moved since
/// indexing is served fresh with origin `external` — the snapshot no longer
/// vouches for it — and marks its partition too.
fn known_note(
    p: &Partition,
    named: &Named<'_>,
    from_this_window: bool,
    dirty: &mut HashSet<String>,
) -> Option<KnownNote> {
    let id = NoteId::parse(named.id).ok()?;
    let (note, revision, stale) = match verify(p, named.relative_path, id, named.revision) {
        Verified::Fresh {
            note,
            revision,
            stale,
            ..
        } => (*note, revision, stale),
        Verified::Gone => {
            dirty.insert(p.partition_key().to_string());
            return None;
        }
    };
    let (origin, effective_status) = if stale {
        dirty.insert(p.partition_key().to_string());
        let status = match note.status {
            NoteStatus::Active => EffectiveStatus::Active,
            NoteStatus::Candidate => EffectiveStatus::Candidate,
            NoteStatus::Archived => return None,
        };
        (Origin::External, status)
    } else {
        let status = match named.effective_status {
            index::EffectiveStatus::Active => EffectiveStatus::Active,
            index::EffectiveStatus::Candidate => EffectiveStatus::Candidate,
            _ => return None,
        };
        (
            Origin::parse(named.origin).unwrap_or(Origin::External),
            status,
        )
    };
    Some(KnownNote {
        id,
        revision,
        partition_key: p.partition_key().to_string(),
        scope: scope_of(p),
        knowledge: knowledge_text(&note.body),
        title: note.title,
        kind: note.kind,
        origin,
        effective_status,
        fact_key: note.fact_key,
        supersedes: note.supersedes,
        from_this_window,
    })
}
