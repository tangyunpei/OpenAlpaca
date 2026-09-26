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
pub(super) fn verified_target(
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
    let (excerpt, excerpt_left_out) = target_excerpt(&note.body);
    Ok(Some(SuggestionTarget {
        id: note_id,
        revision,
        title: note.title,
        excerpt,
        excerpt_left_out,
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
    let (knowledge, knowledge_left_out) = suggested_knowledge(&note.body);
    Some(Suggestion {
        id,
        revision,
        knowledge,
        knowledge_left_out,
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

/// The replaced note's text as a suggestion serves it (`target.excerpt`):
/// its Knowledge section, cut at [`SUGGESTION_EXCERPT_BYTES`], and how many
/// characters the cut left out (0 when it is whole), which the wire carries
/// as `target.excerpt_left_out` for the card to say (A6).
/// `memory_confirm`'s preview quotes and counts this same field.
pub(super) fn target_excerpt(body: &str) -> (String, usize) {
    cut_counting(&knowledge_text(body), SUGGESTION_EXCERPT_BYTES)
}

/// The suggested text as a suggestion serves it (`knowledge`): its
/// Knowledge section, cut at [`SUGGESTION_KNOWLEDGE_BYTES`], and how many
/// characters the cut left out (`knowledge_left_out`, 0 when whole).
/// `memory_confirm`'s preview quotes and counts this same field.
pub(super) fn suggested_knowledge(body: &str) -> (String, usize) {
    cut_counting(&knowledge_text(body), SUGGESTION_KNOWLEDGE_BYTES)
}

/// How many hidden characters one suggestion holds, counted as every
/// surface that decides on it counts them — the Memory view's Suggestions
/// card, `openalpaca memory suggestions` and `memory_confirm`'s preview —
/// over the five fields as the listing serves them: the two titles and the
/// reason with the one-line escape (`escape_hidden_line`, so a tab or a
/// line break in them counts), the replaced note's excerpt and the
/// suggestion's knowledge with the arguments' (`escape_hidden`). One
/// function for the Rust surfaces; the GUI's copy is held to it by
/// `hidden-chars.json`'s `suggestion_cases`.
pub fn suggestion_hidden(
    target_title: &str,
    title: &str,
    reason: Option<&str>,
    excerpt: &str,
    knowledge: &str,
) -> usize {
    use crate::utils::text::{escape_hidden, escape_hidden_line};
    [target_title, title, reason.unwrap_or("")]
        .iter()
        .map(|t| escape_hidden_line(t).hidden)
        .chain([excerpt, knowledge].iter().map(|t| escape_hidden(t).hidden))
        .sum()
}

/// `text` cut to `max` bytes on a character boundary, and how many of its
/// characters the cut left out.
fn cut_counting(text: &str, max: usize) -> (String, usize) {
    let (kept, cut) = cut_bytes(text, max);
    let left_out = if cut {
        text[kept.len()..].chars().count()
    } else {
        0
    };
    (kept, left_out)
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

#[cfg(test)]
mod tests {
    use super::suggestion_hidden;

    /// The GUI's Suggestions card counts one suggestion with its own copy of
    /// [`suggestion_hidden`]; both are held to `hidden-chars.json`'s
    /// `suggestion_cases`, whose fields tell the two escapes apart: each
    /// title and the reason holds a tab or a line break, which counts there
    /// and not in a text, so a field counted with the wrong escape fails.
    /// They tell the fields apart too: in the first case each of the five
    /// holds a different number of hidden characters (1 to 5), so a field
    /// read in place of another, or left out, fails.
    #[test]
    fn suggestion_hidden_is_the_gui_tables_suggestion_cases() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/openalpaca-gui/src/lib/hidden-chars.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        let table: serde_json::Value = serde_json::from_str(&text).expect("the table is JSON");
        let cases = table["suggestion_cases"]
            .as_array()
            .expect("suggestion_cases is an array");
        assert!(!cases.is_empty());
        for case in cases {
            let field = |key: &str| case[key].as_str().expect(key);
            assert_eq!(
                suggestion_hidden(
                    field("target_title"),
                    field("title"),
                    case["reason"].as_str(),
                    field("excerpt"),
                    field("knowledge"),
                ),
                case["hidden"].as_u64().expect("hidden") as usize,
                "{case}"
            );
        }
    }
}
