//! Derived supersession, authority, same-`fact_key` resolution and
//! `source_forgotten` over one partition's notes and its control snapshot
//! (design §3 rule 3, §4 *Authority*, §6 *Derived supersession* and *Forget
//! contract*, §7b P3/P4/P6). Pure: it reads notes and a snapshot and returns
//! per-note facts and the conflicts `doctor` and the GUI report.
//!
//! The rules, as implemented:
//! - **Origin** is the snapshot's record when the file's revision matches it;
//!   otherwise the note is `external` (design §4, *Revisions*).
//! - **A note is superseded** when an active or archived note, or a tombstone,
//!   of the **same partition** names it in `supersedes`, and the superseder's
//!   origin ranks **at or above** the target's. A candidate supersedes
//!   nothing. A lower-authority link is not honored and is reported as a
//!   `supersedes_authority` conflict on the note that carries it (P3).
//! - **Same `fact_key`**: among the notes that are still effectively active,
//!   the highest authority wins; a tie goes to the newest `observed_at` when
//!   every tied note has one; after that (or first, when any lacks one) to
//!   the newest `created_at` (an edit never flips it), then `updated_at`,
//!   then the lowest id (R9); every other note in the group is a loser — out
//!   of automatic recall, `conflict: true` in a search (P4). `doctor` reports
//!   the pair as `same_fact_key` unless the loser's own un-honored link to
//!   the winner already reports it (one conflict per pair; fixtures 18 and 24
//!   read together).
//! - **`source_forgotten`**: a note citing a source some tombstone suppresses,
//!   unless the note was written in a later second than that forget (an edit
//!   lifts it). The clock is to the second, so a write in the forget's own
//!   second counts as before it — suppressed until the next edit, the safe
//!   direction.

use super::note::{Note, NoteId, NoteStatus};
use super::snapshot::{ControlSnapshot, Origin};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, HashMap};
use std::fmt;

/// A note's status once links, tombstones and authority are applied.
/// *Superseded* is derived, never stored (design §4).
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
            EffectiveStatus::Active => "active",
            EffectiveStatus::Superseded => "superseded",
            EffectiveStatus::Archived => "archived",
            EffectiveStatus::Candidate => "candidate",
        }
    }
}

impl fmt::Display for EffectiveStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The two conflicts `doctor` lists (design §6, *Management operations*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConflictKind {
    /// `note` names `target` in `supersedes` with lower authority; the link is
    /// not honored and both notes stay active.
    SupersedesAuthority,
    /// `note` and `target` share a `fact_key` with no link between them;
    /// `target` won by authority (then the tie-break of R9) and `note` is
    /// the loser.
    SameFactKey,
}

impl ConflictKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ConflictKind::SupersedesAuthority => "supersedes_authority",
            ConflictKind::SameFactKey => "same_fact_key",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Conflict {
    pub kind: ConflictKind,
    pub note: NoteId,
    pub target: NoteId,
}

/// What the derivation says about one note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedNote {
    pub id: NoteId,
    /// The snapshot's origin when the file matches its record, else `external`.
    pub origin: Origin,
    /// Whether the snapshot records this note at its current revision.
    pub recorded: bool,
    pub effective_status: EffectiveStatus,
    /// The notes and tombstones whose honored links retire this one.
    pub superseded_by: Vec<NoteId>,
    /// Targets this note names with too little authority to retire them.
    pub unhonored_links: Vec<NoteId>,
    /// The same-`fact_key` winner this note lost to, if any.
    pub same_key_loser_to: Option<NoteId>,
    /// A cited source is suppressed by a tombstone at or after this note's
    /// record (design §6, *Forget contract*).
    pub source_forgotten: bool,
    /// P3: carries an un-honored link, or is a same-`fact_key` loser.
    pub conflict: bool,
}

impl DerivedNote {
    /// Whether the automatic path may deliver this note (design §5 step 5):
    /// effectively active, not a same-key loser, no forgotten source.
    pub fn recallable(&self) -> bool {
        self.effective_status == EffectiveStatus::Active
            && self.same_key_loser_to.is_none()
            && !self.source_forgotten
    }
}

/// One note with the revision of the bytes it was parsed from.
#[derive(Debug, Clone, Copy)]
pub struct NoteInput<'a> {
    pub note: &'a Note,
    pub revision: &'a str,
}

/// The derivation over a whole partition.
#[derive(Debug, Clone, Default)]
pub struct Derivation {
    pub notes: BTreeMap<NoteId, DerivedNote>,
    pub conflicts: Vec<Conflict>,
}

/// Runs the rules above. `inputs` holds one entry per id (duplicates are the
/// caller's to exclude first); `snapshot` is the partition's control snapshot,
/// or an empty one for a quarantined partition (every note then reads as
/// `external`).
pub fn derive(inputs: &[NoteInput<'_>], snapshot: &ControlSnapshot) -> Derivation {
    let by_id: HashMap<NoteId, &Note> = inputs.iter().map(|i| (i.note.id, i.note)).collect();

    // Origin and the time the record (or, unrecorded, the file) says.
    let mut origin: HashMap<NoteId, Origin> = HashMap::new();
    let mut recorded: HashMap<NoteId, bool> = HashMap::new();
    let mut written_at: HashMap<NoteId, DateTime<Utc>> = HashMap::new();
    for input in inputs {
        let id = input.note.id;
        match snapshot.notes.get(&id) {
            Some(record) if record.revision == input.revision => {
                origin.insert(id, record.origin);
                recorded.insert(id, true);
                written_at.insert(id, record.written_at);
            }
            _ => {
                origin.insert(id, Origin::External);
                recorded.insert(id, false);
                written_at.insert(id, input.note.updated_at);
            }
        }
    }

    let mut superseded_by: HashMap<NoteId, Vec<NoteId>> = HashMap::new();
    let mut unhonored: HashMap<NoteId, Vec<NoteId>> = HashMap::new();
    let mut conflicts: Vec<Conflict> = Vec::new();

    // Links carried by notes.
    for input in inputs {
        let note = input.note;
        if note.status == NoteStatus::Candidate {
            continue;
        }
        let my_rank = origin[&note.id].rank();
        for target in &note.supersedes {
            if *target == note.id || !by_id.contains_key(target) {
                continue; // self, another partition, a tombstoned id: ignored
            }
            if my_rank >= origin[target].rank() {
                superseded_by.entry(*target).or_default().push(note.id);
            } else {
                unhonored.entry(note.id).or_default().push(*target);
                conflicts.push(Conflict {
                    kind: ConflictKind::SupersedesAuthority,
                    note: note.id,
                    target: *target,
                });
            }
        }
    }

    // Links carried by tombstones: forgetting a replacement revives nothing.
    for tomb in &snapshot.tombstones {
        for target in &tomb.supersedes {
            if !by_id.contains_key(target) {
                continue;
            }
            if tomb.origin.rank() >= origin[target].rank() {
                superseded_by.entry(*target).or_default().push(tomb.id);
            }
        }
    }

    let effective = |id: &NoteId| -> EffectiveStatus {
        if superseded_by.get(id).is_some_and(|s| !s.is_empty()) {
            return EffectiveStatus::Superseded;
        }
        match by_id[id].status {
            NoteStatus::Active => EffectiveStatus::Active,
            NoteStatus::Archived => EffectiveStatus::Archived,
            NoteStatus::Candidate => EffectiveStatus::Candidate,
        }
    };

    // Same fact_key among the effectively active notes.
    let mut groups: BTreeMap<&str, Vec<NoteId>> = BTreeMap::new();
    for input in inputs {
        if let Some(key) = input.note.fact_key.as_deref()
            && effective(&input.note.id) == EffectiveStatus::Active
        {
            groups.entry(key).or_default().push(input.note.id);
        }
    }
    let mut loser_to: HashMap<NoteId, NoteId> = HashMap::new();
    for ids in groups.values() {
        if ids.len() < 2 {
            continue;
        }
        let winner = same_key_winner(ids, &by_id, &origin);
        for loser in ids.iter().filter(|id| **id != winner) {
            loser_to.insert(*loser, winner);
            let already_reported = by_id[loser].supersedes.contains(&winner);
            if !already_reported {
                conflicts.push(Conflict {
                    kind: ConflictKind::SameFactKey,
                    note: *loser,
                    target: winner,
                });
            }
        }
    }

    let mut notes = BTreeMap::new();
    for input in inputs {
        let id = input.note.id;
        // `<=`: a write in the forget's own second is not "after" it.
        let source_forgotten = input.note.sources.iter().any(|s| {
            let key = s.suppression_key();
            snapshot
                .tombstones
                .iter()
                .any(|t| t.suppressed_sources.contains(&key) && written_at[&id] <= t.forgotten_at)
        });
        let mut by = superseded_by.get(&id).cloned().unwrap_or_default();
        by.sort();
        by.dedup();
        let links = unhonored.get(&id).cloned().unwrap_or_default();
        let loser = loser_to.get(&id).copied();
        notes.insert(
            id,
            DerivedNote {
                id,
                origin: origin[&id],
                recorded: recorded[&id],
                effective_status: effective(&id),
                superseded_by: by,
                conflict: !links.is_empty() || loser.is_some(),
                unhonored_links: links,
                same_key_loser_to: loser,
                source_forgotten,
            },
        );
    }

    Derivation { notes, conflicts }
}

/// The same-`fact_key` winner of one group (design §3 rule 3, §7c R9): the
/// highest authority; among the notes tied on it, the newest `observed_at`
/// when **every** one of them has one; after that (or first, when any lacks
/// one) the newest `created_at` — the note first written most recently,
/// which an edit does not move — then the newest `updated_at`, then the
/// lowest id. Deciding the clock once for the tied set keeps this an order:
/// "`observed_at` when both have one" is not transitive over three notes, and
/// the winner would then depend on the order the files were listed in.
fn same_key_winner(
    ids: &[NoteId],
    by_id: &HashMap<NoteId, &Note>,
    origin: &HashMap<NoteId, Origin>,
) -> NoteId {
    let top = ids
        .iter()
        .map(|id| origin[id].rank())
        .max()
        .expect("a group has at least two ids");
    let tied: Vec<NoteId> = ids
        .iter()
        .copied()
        .filter(|id| origin[id].rank() == top)
        .collect();
    let observed = tied.iter().all(|id| by_id[id].observed_at.is_some());
    tied.into_iter()
        .max_by(|a, b| {
            let (na, nb) = (by_id[a], by_id[b]);
            let by_observation = if observed {
                na.observed_at.cmp(&nb.observed_at)
            } else {
                std::cmp::Ordering::Equal
            };
            by_observation
                .then_with(|| na.created_at.cmp(&nb.created_at))
                .then_with(|| na.updated_at.cmp(&nb.updated_at))
                .then_with(|| b.cmp(a)) // lowest id wins a full tie: deterministic
        })
        .expect("the top rank has at least one note")
}
