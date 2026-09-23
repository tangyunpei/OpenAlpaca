//! The Markdown memory store's file layer (memory redesign, design §2, §4, §6,
//! §7a, §7b — `tasks/memory-redesign/03-design.md` rev 4.1).
//!
//! This module owns everything that touches a memory **partition on disk** and
//! nothing that touches SQLite:
//!
//! - the note file format ([`Note`]): frontmatter parse and the canonical
//!   serialization whose exact bytes the revision is a hash of, unknown fields
//!   preserved verbatim;
//! - the owner key and partition key derivations (design §2);
//! - the control snapshot ([`ControlSnapshot`], design §7a), replaced
//!   atomically, one per partition, under `state/memory-control/`;
//! - a [`Partition`]: confinement of the path to it (design §3 rule 1), the
//!   project `.gitignore` assertion (MD-7), private modes, the atomic write,
//!   `.history`, the mutation protocol with its single `pending` field and the
//!   recovery table (design §6), tombstone-first forget, the scanner and the
//!   reconciliation of external changes;
//! - the derived supersession, authority and same-`fact_key` computation over
//!   a partition's notes and snapshot ([`derive`], design §6, §7b P3/P4/P6);
//! - the offline management operations `doctor`, `doctor --repair` and
//!   `adopt` (design §6, *Management operations*).
//!
//! It has no LLM, no network and no dependency on the search index: the index
//! is `memory_index`, which takes pre-computed records, and the service that
//! maps one to the other is slice 3.
//!
//! **Nothing here reaches a real home directory on its own.** Every path
//! starts from [`crate::store::home_root`] (which honours
//! `OPENALPACA_HOME_STORE`) or from an explicit project root.

mod authority;
mod fsx;
mod note;
mod partition;
mod snapshot;

#[cfg(test)]
mod tests;

pub use authority::{
    Conflict, ConflictKind, Derivation, DerivedNote, EffectiveStatus, NoteInput, derive,
};
pub use note::{
    NewNote, Note, NoteId, NoteKind, NoteParseError, NotePatch, NoteScope, NoteSource, NoteStatus,
    SourceType, UnknownField, is_valid_fact_key, revision_of,
};
pub use partition::{
    Actor, CrashPoint, Created, Dedup, ForgetOutcome, ForgetStatus, LoadedPartition, MalformedFile,
    MoveSnapshotState, NoteView, Partition, PartitionHealth, ReconcileChange, ReconcileChangeKind,
    RepairReport, ScanEntry, UpdateAction, Updated, adopt, classify_move_snapshots,
    list_owner_keys, list_snapshots, snapshot_path_for,
};
pub use snapshot::{
    ControlSnapshot, NoteRecord, Origin, Pending, SNAPSHOT_SCHEMA, SnapshotError, SnapshotScope,
    Tombstone,
};

use sha2::{Digest, Sha256};
use std::fmt;
use std::path::PathBuf;

// ============================================================================
// Limits (design §4, "Initial limits") and layout constants (design §2)
// ============================================================================

/// The one frontmatter schema this build reads and writes.
pub const NOTE_SCHEMA: u32 = 1;
/// A note file's whole size, frontmatter included (design §4: 32 KiB).
pub const MAX_NOTE_BYTES: usize = 32 * 1024;
/// `memory_store.title`: 1–200 characters (design §7a).
pub const MAX_TITLE_CHARS: usize = 200;
/// `aliases` and `tags`: at most 16 entries each (design §7a).
pub const MAX_LIST_ENTRIES: usize = 16;
/// `sources`: at most 8 entries (design §7a).
pub const MAX_SOURCES: usize = 8;
/// `.history/<id>/` keeps the last 5 service-written revisions, the current one
/// included (design §2, §6).
pub const HISTORY_KEEP: usize = 5;
/// The slug budget of a note file name (design §2: `slugify(title, 48)`).
pub const SLUG_MAX_BYTES: usize = 48;
/// The slug used when [`crate::store::slugify`] has nothing left of a title.
pub const EMPTY_TITLE_SLUG: &str = "note";
/// The project partition's `.gitignore`, byte-exact (MD-7).
pub const PROJECT_GITIGNORE_TEXT: &str = "*\n";
/// Directory names inside a partition (design §2).
pub const OWNERS_DIR: &str = "owners";
pub const NOTES_DIR: &str = "notes";
pub const HISTORY_DIR: &str = ".history";
pub const NAVIGATION_FILE: &str = "MEMORY.md";
/// `state/memory-control/` holds one snapshot per partition (design §7a).
pub const CONTROL_DIR: &str = "memory-control";
/// The owner key is the first 32 lowercase hex characters of SHA-256 over the
/// UTF-8 bytes of the owner id (design §2).
pub const OWNER_KEY_HEX: usize = 32;
/// The directory key is the first 16 lowercase hex characters of SHA-256 over
/// the canonical partition directory string (design §2).
pub const DIR_KEY_HEX: usize = 16;

// ============================================================================
// Key derivations (design §2)
// ============================================================================

/// Lowercase hex SHA-256 of `bytes` — a revision when `bytes` is a note file.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The owner key: the first [`OWNER_KEY_HEX`] characters of
/// `sha256_hex(owner_id)`.
pub fn owner_key(owner_id: &str) -> String {
    let mut key = sha256_hex(owner_id.as_bytes());
    key.truncate(OWNER_KEY_HEX);
    key
}

/// The directory key of a project partition: the first [`DIR_KEY_HEX`]
/// characters of `sha256_hex` over the **canonical** partition directory
/// string (`…/.openalpaca/memory/owners/<owner-key>` after `canonicalize()`,
/// hashed as the platform returns it).
pub fn dir_key(canonical_partition_dir: &str) -> String {
    let mut key = sha256_hex(canonical_partition_dir.as_bytes());
    key.truncate(DIR_KEY_HEX);
    key
}

/// `home-<owner-key>`.
pub fn home_partition_key(owner_key: &str) -> String {
    format!("home-{owner_key}")
}

/// `proj-<owner-key>-<dir-key>`.
pub fn project_partition_key(owner_key: &str, canonical_partition_dir: &str) -> String {
    format!("proj-{owner_key}-{}", dir_key(canonical_partition_dir))
}

/// Whether `s` is a 64-character lowercase hex revision.
pub fn is_revision(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// ============================================================================
// Errors
// ============================================================================

/// Every refusal the file layer can answer with, by the class the design and
/// the fixtures name (design §7a, fixtures README *Error classes*).
#[derive(Debug)]
pub enum MemoryFilesError {
    /// `invalid_input`: a limit, a grammar or a shape was violated; nothing was
    /// written.
    InvalidInput(String),
    /// `not_found`: no note of this partition has that id.
    NotFound(NoteId),
    /// `revision_conflict`: the note's current revision is not the one the
    /// caller expected. Carries the current one.
    RevisionConflict {
        id: NoteId,
        current_revision: String,
    },
    /// `authority_conflict`: a tool call tried to edit, archive or supersede a
    /// `user`-origin note (design §4, G2).
    AuthorityConflict { id: NoteId, message: String },
    /// `confirm_refused`: `confirm` from a context that is not a user-directed
    /// surface (design §4).
    ConfirmRefused { id: NoteId, message: String },
    /// `quarantined`: the partition's snapshot is missing or invalid (design §2).
    Quarantined {
        partition_key: String,
        reason: String,
    },
    /// The path to the partition has a symlink, a junction or a non-directory
    /// at `component` (design §3 rule 1); the partition is refused. The caller
    /// decides the fallback and logs the one WARN.
    PathRefused { component: PathBuf, reason: String },
    /// `doctor --repair` met two files sharing one id (design §6).
    DuplicateId { id: NoteId, files: Vec<PathBuf> },
    /// `adopt` met an existing partition of the current owner in a scope it
    /// would rename into (design §6).
    AdoptCollision(String),
    /// A test-injected crash: the mutation stopped after the named step with
    /// the on-disk state exactly as it was at that point.
    #[cfg(test)]
    Crashed(CrashPoint),
    /// Anything the file system refused.
    Io(anyhow::Error),
}

impl fmt::Display for MemoryFilesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(m) => write!(f, "invalid input: {m}"),
            Self::NotFound(id) => write!(f, "note {id} not found"),
            Self::RevisionConflict {
                id,
                current_revision,
            } => write!(
                f,
                "note {id} changed since it was read: current revision is {current_revision}"
            ),
            Self::AuthorityConflict { message, .. } => write!(f, "{message}"),
            Self::ConfirmRefused { message, .. } => write!(f, "{message}"),
            Self::Quarantined {
                partition_key,
                reason,
            } => write!(
                f,
                "memory partition {partition_key} is quarantined ({reason}); run `openalpaca memory doctor --repair`"
            ),
            Self::PathRefused { component, reason } => write!(
                f,
                "memory partition refused: {} is {reason}",
                component.display()
            ),
            Self::DuplicateId { id, files } => {
                let names: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
                write!(
                    f,
                    "two note files share the id {id}: {}; the service cannot choose, remove one by hand",
                    names.join(" and ")
                )
            }
            Self::AdoptCollision(m) => write!(f, "adopt refused: {m}"),
            #[cfg(test)]
            Self::Crashed(point) => write!(f, "crashed after {point:?}"),
            Self::Io(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for MemoryFilesError {}

impl From<anyhow::Error> for MemoryFilesError {
    fn from(e: anyhow::Error) -> Self {
        Self::Io(e)
    }
}

impl From<std::io::Error> for MemoryFilesError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(anyhow::Error::from(e))
    }
}

impl MemoryFilesError {
    /// The fixture-README error class (`invalid_input`, `not_found`, …), for
    /// reports and tests.
    pub fn class(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid_input",
            Self::NotFound(_) => "not_found",
            Self::RevisionConflict { .. } => "revision_conflict",
            Self::AuthorityConflict { .. } => "authority_conflict",
            Self::ConfirmRefused { .. } => "confirm_refused",
            Self::Quarantined { .. } => "quarantined",
            Self::PathRefused { .. } => "fixture:path_refused",
            Self::DuplicateId { .. } => "fixture:duplicate_id",
            Self::AdoptCollision(_) => "fixture:adopt_collision",
            #[cfg(test)]
            Self::Crashed(_) => "crashed",
            Self::Io(_) => "io",
        }
    }
}
