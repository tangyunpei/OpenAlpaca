//! `MemoryAccessContext` (design §7a) — who is asking, for whom, on which
//! surface, with what capabilities.
//!
//! The service builds every context itself ([`super::MemoryService::context`]
//! and its per-surface helpers), never a caller of a tool, so the two rules
//! of construction in design §3 hold by construction: the owner is a
//! `global_user` row, and the surface is what the constructor says it is.

use super::{MemoryError, MemoryService};
use crate::security::policy::Principal;
use openalpaca_storage::memory_files::Actor;
use openalpaca_storage::memory_files::Origin;
use std::path::{Path, PathBuf};

/// Who is asking (design §3, §7a). Set by the constructor, never by a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// Exactly the HTTP API, the CLI verbs that call it, the GUI (design
    /// §4), and a chat memory command typed on an attended app or CLI chat
    /// turn (design 09 §12, [`MemoryService::context_for_chat_command`]):
    /// the only surface that mints origin `user`.
    UserSurface,
    /// Every tool call, whichever loop made it and whatever the model was
    /// told (G2): a write records `extracted_user`, `confirm` is refused.
    ModelTool,
    /// The background extractor: `extracted_user` for the user's own words,
    /// `extracted_other` (a candidate) for assistant or tool output.
    Extraction,
}

impl Surface {
    /// The `memory_changed.surface` wire value (design §7a).
    pub fn as_str(self) -> &'static str {
        match self {
            Surface::UserSurface => "user",
            Surface::ModelTool => "model_tool",
            Surface::Extraction => "extraction",
        }
    }
}

/// What a call may do (design §7a). The service refuses anything else even
/// when a tool was routed to it — a second check behind the sandbox gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub read: bool,
    pub write: bool,
    pub forget: bool,
}

impl Capabilities {
    /// A user-directed surface: everything.
    pub const ALL: Capabilities = Capabilities {
        read: true,
        write: true,
        forget: true,
    };
    /// A worker, the lead, a read tool: `memory_read` only.
    pub const READ_ONLY: Capabilities = Capabilities {
        read: true,
        write: false,
        forget: false,
    };
    /// A write tool on the main conversation's surface (`memory_store`,
    /// `memory_update`): `memory_read` + `memory_write`.
    pub const READ_WRITE: Capabilities = Capabilities {
        read: true,
        write: true,
        forget: false,
    };
    /// The background extractor: `write` only (design §7a).
    pub const EXTRACTION: Capabilities = Capabilities {
        read: false,
        write: true,
        forget: false,
    };
}

/// For events, `memory_used` and the recently-used ring (design §7a).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attribution {
    pub lane_key: Option<String>,
    pub task_id: Option<String>,
    pub request_id: Option<uuid::Uuid>,
    pub source: Option<String>,
}

/// The typed access context every operation receives (design §7a).
#[derive(Debug, Clone)]
pub struct MemoryAccessContext {
    /// A `global_user.id` row (design §3); never from a tool argument.
    pub owner_id: String,
    /// Derived: the first 32 hex characters of `sha256(owner_id)`.
    pub owner_key: String,
    /// The MD-5 marker root of the request workspace; `None` = the global
    /// partition only.
    pub project_root: Option<PathBuf>,
    /// Who is asking. Set by the constructor, never by a caller.
    pub surface: Surface,
    /// What this call may do.
    pub capabilities: Capabilities,
    pub attribution: Attribution,
}

impl MemoryAccessContext {
    /// The file layer's actor for a write from this context. An extraction
    /// write carries its own origin (the extractor decides per item).
    pub(super) fn actor(&self, extraction_origin: Option<Origin>) -> Actor {
        match self.surface {
            Surface::UserSurface => Actor::UserSurface,
            Surface::ModelTool => Actor::ModelTool,
            Surface::Extraction => Actor::Extraction {
                origin: extraction_origin.unwrap_or(Origin::ExtractedOther),
            },
        }
    }
}

/// Where the request says it is working (design §3 rule 1, MD-5).
#[derive(Debug, Clone, Copy)]
pub enum WorkspaceHint<'a> {
    /// The request named no workspace: the global partition only. The
    /// daemon's own CWD is never consulted.
    None,
    /// A root the marker walk already found (`ToolContext.request_workspace_root`,
    /// R22).
    Root(&'a Path),
    /// A directory the request named (the `x-workspace-path` header, a CLI's
    /// CWD): the marker walk runs from it.
    Path(&'a Path),
}

/// Resolves a hint to the project root that scopes memory, or `None` for the
/// global partition only. A directory with no marker above it is global; a
/// root whose store *is* the home store (`$HOME`) is not a project.
pub fn resolve_project_root(hint: WorkspaceHint<'_>) -> Option<PathBuf> {
    let root = match hint {
        WorkspaceHint::None => return None,
        WorkspaceHint::Root(root) => root.to_path_buf(),
        WorkspaceHint::Path(path) => {
            crate::memory::workspace::detect_workspace_root_uncached(path)?
        }
    };
    if openalpaca_storage::store::projects::overlaps_home_store(&root) {
        return None;
    }
    Some(root)
}

/// The derived sources of the chats a person types into directly (design 09
/// §12.2): the app's and the CLI's — both `EventSource::Gui` today, so both
/// derive `gui`; `cli` is the name `HandleRequest::source` reserves for the
/// CLI. A connector (`telegram`, `discord`, `imessage`), a scheduled skill or
/// a follow-up (`internal`) and `/v1/command process` (`api`) are not.
///
/// Wider than `memory_confirm`'s `IN_CHAT_SOURCE`
/// (`tools/builtins/memory_tools.rs`, `gui` only) on purpose: a memory
/// command only needs its reply read, while a confirmation card needs a
/// client that shows its preview and answers it per call. The two coincide
/// until a production path derives `cli`; see `IN_CHAT_SOURCE`.
pub const ATTENDED_CHAT_SOURCES: [&str; 2] = ["gui", "cli"];

/// The chat turn a memory command was typed on (design 09 §12.2) — what
/// [`MemoryService::context_for_chat_command`] builds its context from.
#[derive(Debug, Clone)]
pub struct ChatCommandTurn {
    pub principal: Principal,
    /// The turn's derived source (`HandleRequest::source`).
    pub source: String,
    /// The turn's own M6 declaration: nobody is there to read the answer.
    pub unattended: bool,
    /// The MD-5 marker root the turn resolved from its workspace (R22) —
    /// the root that turn's own tool calls use; `None`: global only.
    pub request_workspace_root: Option<PathBuf>,
    pub lane_key: String,
    pub request_id: uuid::Uuid,
}

impl ChatCommandTurn {
    /// An attended turn of the app's or the CLI's own chat: the one kind of
    /// chat turn on which a person typed the command where they can read
    /// the answer (design 09 §12.2).
    pub fn is_attended_chat(&self) -> bool {
        !self.unattended && ATTENDED_CHAT_SOURCES.contains(&self.source.as_str())
    }
}

impl MemoryService {
    /// A chat memory command's context (design 09 §12; the per-surface table
    /// of design §7a gains this row): the turn's owner, the project root the
    /// turn resolved, `UserSurface`, every capability — so `/remember`
    /// records origin `user`, as the Memory view does.
    ///
    /// G2 holds because no model is anywhere on this path: the orchestrator
    /// answers the command in its deterministic tier, and the text written is
    /// exactly what the person typed. The constructor checks what makes that
    /// true itself, so a `UserSurface` context cannot be built from any other
    /// turn: anything but an attended app or CLI chat turn is
    /// `invalid_input`, and a principal that is not a `global_user` row —
    /// `System`, an unlinked `External` — is [`MemoryError::NoOwner`].
    pub fn context_for_chat_command(
        &self,
        turn: &ChatCommandTurn,
    ) -> Result<MemoryAccessContext, MemoryError> {
        if !turn.is_attended_chat() {
            return Err(MemoryError::invalid(format!(
                "a memory command is a user-directed surface only on an attended turn of the app's or the CLI's chat, not on a `{}` turn{}",
                turn.source,
                if turn.unattended {
                    " nobody attends"
                } else {
                    ""
                }
            )));
        }
        let global_id = match &turn.principal {
            Principal::User { global_id } => global_id,
            Principal::System => return Err(MemoryError::NoOwner("system".to_string())),
            Principal::External { provider, id } => {
                return Err(MemoryError::NoOwner(format!("{provider}:{id}")));
            }
        };
        let hint = match &turn.request_workspace_root {
            Some(root) => WorkspaceHint::Root(root.as_path()),
            None => WorkspaceHint::None,
        };
        self.context(
            global_id,
            hint,
            Surface::UserSurface,
            Capabilities::ALL,
            Attribution {
                lane_key: Some(turn.lane_key.clone()),
                task_id: None,
                request_id: Some(turn.request_id),
                source: Some(turn.source.clone()),
            },
        )
    }
}
