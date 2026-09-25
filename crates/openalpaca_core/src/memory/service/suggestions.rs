//! The pending suggestions (Learning v2, design 09 §3.1, §3.5; MD-11): the
//! candidates learning wrote that name a note they would replace — a change
//! to a note the user saved or edited, or an inferred change to any note —
//! which retire nothing until the user confirms one.
//!
//! The list is read from the index's `note.supersedes` column (ruling
//! LR18), and each listed file — the suggestion and its target — is re-read
//! and re-hashed before it is served (design §5 step 7), as recall does. No
//! partition is parsed unless the stat scan finds it changed.

use super::recall::{Verified, verify};
use super::{
    MemoryAccessContext, MemoryError, MemoryService, Suggestion, SuggestionTarget, cut_bytes,
    scope_of, section_of,
};
use openalpaca_storage::memory_files::{
    NoteId, NoteStatus, Origin, Partition, PartitionHealth, knowledge_text,
};
use openalpaca_storage::memory_index::{CatalogNote, EffectiveStatus as IndexStatus};
use std::collections::HashSet;

/// `GET /v1/memories/suggestions?limit`: default 20.
pub const SUGGESTIONS_DEFAULT_LIMIT: usize = 20;
/// … and at most 100.
pub const SUGGESTIONS_MAX_LIMIT: usize = 100;
/// The target's excerpt shown beside the suggested text.
pub const SUGGESTION_EXCERPT_BYTES: usize = 600;
/// The suggested text itself (a learned note's knowledge is at most 1,000
/// characters, design 09 §2.2).
const SUGGESTION_KNOWLEDGE_BYTES: usize = 4 * 1024;

impl MemoryService {
    /// The pending suggestions of the owner's partitions (design 09 §3.5),
    /// newest first. Where several name one note, only the newest is listed
    /// and its `older_count` says how many are not. A suggestion whose
    /// target is gone or no longer effectively active is moot and not
    /// listed (it stays a candidate in the list, where it can still be
    /// undone). Quarantined partitions contribute nothing. Records nothing:
    /// a listing is not a recall.
    pub fn suggestions(
        &self,
        ctx: &MemoryAccessContext,
        limit: Option<usize>,
    ) -> Result<Vec<Suggestion>, MemoryError> {
        Self::require(ctx.capabilities.read, "memory_read")?;
        let limit = limit.unwrap_or(SUGGESTIONS_DEFAULT_LIMIT);
        if limit == 0 || limit > SUGGESTIONS_MAX_LIMIT {
            return Err(MemoryError::invalid(format!(
                "limit must be between 1 and {SUGGESTIONS_MAX_LIMIT}"
            )));
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
        let by_key = |key: &str| {
            partitions
                .iter()
                .copied()
                .find(|p| p.partition_key() == key)
        };

        // Newest first; one group per (partition, target) in that order. A
        // candidate naming several notes cannot be confirmed (S5) and is not
        // a suggestion here.
        let mut groups: Vec<((String, String), Vec<CatalogNote>)> = Vec::new();
        for row in self.index.suggestion_rows(&keys)? {
            let [target] = row.supersedes.as_slice() else {
                continue;
            };
            let group = (row.partition_key.clone(), target.clone());
            match groups.iter_mut().find(|(g, _)| *g == group) {
                Some((_, rows)) => rows.push(row),
                None => groups.push((group, vec![row])),
            }
        }

        let mut dirty: HashSet<String> = HashSet::new();
        let mut out: Vec<Suggestion> = Vec::new();
        for ((partition_key, target_id), rows) in groups {
            if out.len() >= limit {
                break;
            }
            let Some(p) = by_key(&partition_key) else {
                continue;
            };
            let Some(target) = verified_target(self, p, &target_id, &mut dirty)? else {
                continue;
            };
            let total = rows.len();
            for (skipped, row) in rows.into_iter().enumerate() {
                if let Some(mut suggestion) = verified_suggestion(p, &row, &target, &mut dirty) {
                    suggestion.older_count = total - skipped - 1;
                    out.push(suggestion);
                    break;
                }
            }
        }
        for key in dirty {
            if let Some(p) = by_key(&key) {
                self.reindex(p, true)?;
            }
        }
        out.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.to_string().cmp(&b.id.to_string()))
        });
        Ok(out)
    }
}

/// The note a suggestion names, re-read: `None` when it is gone or no longer
/// effectively active — nothing is left to replace.
fn verified_target(
    service: &MemoryService,
    p: &Partition,
    id: &str,
    dirty: &mut HashSet<String>,
) -> Result<Option<SuggestionTarget>, MemoryError> {
    let Some(row) = service.index.catalog_note(p.partition_key(), id)? else {
        return Ok(None);
    };
    if row.effective_status != IndexStatus::Active {
        return Ok(None);
    }
    let Ok(note_id) = NoteId::parse(&row.id) else {
        return Ok(None);
    };
    let (note, revision, stale) = match verify(p, &row.relative_path, note_id, &row.revision) {
        Verified::Fresh {
            note,
            revision,
            stale,
            ..
        } => (*note, revision, stale),
        Verified::Gone => {
            dirty.insert(p.partition_key().to_string());
            return Ok(None);
        }
    };
    let origin = if stale {
        dirty.insert(p.partition_key().to_string());
        if note.status != NoteStatus::Active {
            return Ok(None);
        }
        Origin::External
    } else {
        Origin::parse(&row.origin).unwrap_or(Origin::External)
    };
    let (excerpt, _) = cut_bytes(&knowledge_text(&note.body), SUGGESTION_EXCERPT_BYTES);
    Ok(Some(SuggestionTarget {
        id: note_id,
        revision,
        title: note.title,
        excerpt,
        origin,
        scope: scope_of(p),
    }))
}

/// One suggestion row, re-read: `None` when the file is gone, is another
/// note now, or is no longer a candidate naming exactly `target`.
fn verified_suggestion(
    p: &Partition,
    row: &CatalogNote,
    target: &SuggestionTarget,
    dirty: &mut HashSet<String>,
) -> Option<Suggestion> {
    let id = NoteId::parse(&row.id).ok()?;
    let (note, revision, stale) = match verify(p, &row.relative_path, id, &row.revision) {
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
    if stale {
        dirty.insert(p.partition_key().to_string());
    }
    if note.status != NoteStatus::Candidate || note.supersedes != [target.id] {
        return None;
    }
    let origin = if stale {
        Origin::External
    } else {
        Origin::parse(&row.origin).unwrap_or(Origin::External)
    };
    let (knowledge, _) = cut_bytes(&knowledge_text(&note.body), SUGGESTION_KNOWLEDGE_BYTES);
    Some(Suggestion {
        id,
        revision,
        knowledge,
        reason: replace_reason(&note.body),
        title: note.title,
        created_at: note.created_at,
        observed_at: note.observed_at,
        origin,
        scope: scope_of(p),
        sources: note.sources,
        older_count: 0,
        target: target.clone(),
    })
}

/// The reason a learned replacement gives (design 09 §2.6): its evidence
/// section's `Replaces “<title>” (<label>): <reason>` sentence, the reason
/// alone. `None` for a note with no such sentence — one written by hand, or
/// before Learning v2.
pub(crate) fn replace_reason(body: &str) -> Option<String> {
    let evidence = section_of(body, "Evidence and limits")?;
    let at = evidence.find("Replaces “")?;
    let rest = &evidence[at..];
    let colon = rest.find("): ")?;
    let reason = rest[colon + 3..].lines().next()?.trim();
    (!reason.is_empty()).then(|| reason.to_string())
}
