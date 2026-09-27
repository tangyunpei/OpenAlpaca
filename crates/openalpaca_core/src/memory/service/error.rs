//! The service's refusals, by the class the design and the fixtures name
//! (design §7a, fixtures README *Error classes*).

use openalpaca_storage::memory_files::{MemoryFilesError, NoteId, Origin};
use openalpaca_storage::memory_index::IndexError;
use std::fmt;

#[derive(Debug)]
pub enum MemoryError {
    /// `fixture:no_owner` (design §3): the id is not a `global_user` row —
    /// the `system` sentinel, a `provider:id` string, or a daemon with no
    /// database to ask.
    NoOwner(String),
    /// `fixture:capability_denied` (design §7a): the context lacks the
    /// capability the operation needs. Names it (`memory_read`,
    /// `memory_write`, `memory_forget`).
    CapabilityDenied { capability: &'static str },
    /// `invalid_input`: a limit, a grammar, a shape or an unknown field.
    InvalidInput(String),
    /// `not_found`: no note of this owner's partitions has that id.
    NotFound(NoteId),
    /// `not_found`, for `?revision=`: the note exists but `.history` keeps
    /// no copy under that revision (design §2: the last 5 service-written
    /// revisions, and none of an external edit).
    RevisionUnavailable { id: NoteId, revision: String },
    /// `revision_conflict`: carries the current revision.
    RevisionConflict {
        id: NoteId,
        current_revision: String,
    },
    /// `authority_conflict`: a `ModelTool` edit, archive or supersede of a
    /// `user` note (design §4, G2).
    AuthorityConflict { id: NoteId, message: String },
    /// `confirm_refused`: `confirm` from a context that is not a
    /// user-directed surface (design §4).
    ConfirmRefused { id: NoteId, message: String },
    /// `not_undoable` (design §6): the origin is no longer `extracted_user`.
    NotUndoable { id: NoteId, origin: Origin },
    /// `not_a_suggestion` (design 09 §4.2 rule 5): `memory_confirm` names a
    /// note that is not a suggestion learned from the user's own words — not
    /// a candidate naming a note it replaces, inferred from the conversation
    /// (`extracted_other`), or citing no row the user typed. Confirmable, if
    /// at all, only in the Memory view or the CLI.
    NotASuggestion { id: NoteId, message: String },
    /// `approval_required` (design 09 §4.2 rule 6): `memory_confirm` ran
    /// without a person's Approve on its card for this call. Nothing is
    /// written.
    ApprovalRequired { id: NoteId },
    /// `quarantined` (design §2): the partition's snapshot is missing or
    /// invalid; only `doctor --repair` lifts it.
    Quarantined {
        partition_key: String,
        reason: String,
    },
    /// `fixture:write_pending` (design §3, workspace move step 1): a memory
    /// write for that project is in flight.
    WritePending { key: String },
    /// Anything the file system or the index refused.
    Internal(anyhow::Error),
}

impl MemoryError {
    /// The fixture-README class (`invalid_input`, `not_found`, …).
    pub fn class(&self) -> &'static str {
        match self {
            Self::NoOwner(_) => "fixture:no_owner",
            Self::CapabilityDenied { .. } => "fixture:capability_denied",
            Self::InvalidInput(_) => "invalid_input",
            Self::NotFound(_) | Self::RevisionUnavailable { .. } => "not_found",
            Self::RevisionConflict { .. } => "revision_conflict",
            Self::AuthorityConflict { .. } => "authority_conflict",
            Self::ConfirmRefused { .. } => "confirm_refused",
            Self::NotUndoable { .. } => "not_undoable",
            Self::NotASuggestion { .. } => "not_a_suggestion",
            Self::ApprovalRequired { .. } => "approval_required",
            Self::Quarantined { .. } => "quarantined",
            Self::WritePending { .. } => "fixture:write_pending",
            Self::Internal(_) => "io",
        }
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidInput(msg.into())
    }
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoOwner(id) => write!(
                f,
                "no memory owner: `{id}` is not a known user of this daemon, so nothing is remembered for it"
            ),
            Self::CapabilityDenied { capability } => write!(
                f,
                "this context was not granted the `{capability}` capability, so the memory service refused the call"
            ),
            Self::InvalidInput(m) => write!(f, "invalid input: {m}"),
            Self::NotFound(id) => write!(f, "note {id} not found"),
            Self::RevisionUnavailable { id, revision } => write!(
                f,
                "note {id} has no kept revision {revision}: .history holds the last 5 service-written revisions only"
            ),
            Self::RevisionConflict {
                id,
                current_revision,
            } => write!(
                f,
                "note {id} changed since it was read: current revision is {current_revision}"
            ),
            Self::AuthorityConflict { message, .. }
            | Self::ConfirmRefused { message, .. }
            | Self::NotASuggestion { message, .. } => f.write_str(message),
            Self::ApprovalRequired { id } => write!(
                f,
                "note {id} was not confirmed: only the user's Approve on the confirmation card for this call confirms a suggestion, and there was none. Nothing was written."
            ),
            Self::NotUndoable { id, origin } => write!(
                f,
                "note {id} is not undoable: its origin is `{origin}` — neither learned from your words (`extracted_user`) nor a pending suggestion; forget it instead"
            ),
            Self::Quarantined {
                partition_key,
                reason,
            } => write!(
                f,
                "memory partition {partition_key} is quarantined ({reason}); run `openalpaca memory doctor --repair`"
            ),
            Self::WritePending { key } => {
                write!(f, "a memory write for {key} is in flight; try again")
            }
            Self::Internal(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for MemoryError {}

impl From<MemoryFilesError> for MemoryError {
    fn from(e: MemoryFilesError) -> Self {
        match e {
            MemoryFilesError::InvalidInput(m) => Self::InvalidInput(m),
            MemoryFilesError::NotFound(id) => Self::NotFound(id),
            MemoryFilesError::RevisionConflict {
                id,
                current_revision,
            } => Self::RevisionConflict {
                id,
                current_revision,
            },
            MemoryFilesError::AuthorityConflict { id, message } => {
                Self::AuthorityConflict { id, message }
            }
            MemoryFilesError::ConfirmRefused { id, message } => {
                Self::ConfirmRefused { id, message }
            }
            MemoryFilesError::Quarantined {
                partition_key,
                reason,
            } => Self::Quarantined {
                partition_key,
                reason,
            },
            other => Self::Internal(anyhow::anyhow!("{other}")),
        }
    }
}

impl From<IndexError> for MemoryError {
    fn from(e: IndexError) -> Self {
        match e {
            IndexError::InvalidInput(m) => Self::InvalidInput(m),
            // Kept as the error it is (the text is the same), so the service
            // can tell a damaged cache from any other failure.
            other => Self::Internal(anyhow::Error::new(other)),
        }
    }
}

impl MemoryError {
    /// Whether this is the index cache reporting damage
    /// ([`IndexError::is_corruption`]) — what makes the service rebuild it
    /// (scale slice S6).
    pub(crate) fn is_index_corruption(&self) -> bool {
        matches!(self, Self::Internal(e)
            if e.downcast_ref::<IndexError>().is_some_and(IndexError::is_corruption))
    }
}

impl From<anyhow::Error> for MemoryError {
    fn from(e: anyhow::Error) -> Self {
        Self::Internal(e)
    }
}
