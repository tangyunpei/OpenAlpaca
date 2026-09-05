//! The ENABLE axis — per-extension enable/disable bookkeeping (ADR-030, N5).
//!
//! Two axes govern tools. **ALLOW** is per agent (template `capabilities`,
//! skill `requires_capabilities`) and already exists. **ENABLE** is per
//! extension — one MCP server, one plugin — and is what this module carries:
//! the [`ExtensionLedger`], the state vocabulary every consumer renders from,
//! and the [`ExtensionSupervisor`] trait the two implementors (`McpSupervisor`
//! in `apps/openalpacad`, `PluginManager` in `openalpaca_plugins`) satisfy.
//!
//! The trait is declared here because both implementors are *downstream* of
//! `openalpaca_core` and nothing else is upstream of both.
//!
//! Builtins are never on this axis. The ledger is pure bookkeeping: it never
//! holds a client, a process or a file path — teardown and file writes are
//! supervisor work (design §5).

use serde::{Deserialize, Serialize};
use std::fmt;

pub mod describe;
mod ledger;
pub mod scan;
#[cfg(test)]
mod scan_tests;

pub use describe::{Audience, Described};
pub use ledger::{CallGuard, ExtensionLedger, ExtensionRecord, ScopedRun, Transition};
pub use scan::{DependentScan, PendingScan, ScanOutcome, WithdrawnSet};

// ============================================================================
// Identity
// ============================================================================

/// The two kinds of tool-contributing extension. `kind` is the extension point
/// if a third ever appears; connectors and LLM providers are deliberately not
/// on this axis (design §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionKind {
    Mcp,
    Plugin,
}

impl ExtensionKind {
    /// The API's `kind` field (design §8).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Plugin => "plugin",
        }
    }

    /// How the kind reads in the `<kind>` slot of the §7.1 wording table.
    pub fn prose(&self) -> &'static str {
        match self {
            Self::Mcp => "MCP server",
            Self::Plugin => "plugin",
        }
    }

    /// Where this kind's disposition bit lives (design §5) — the store location
    /// appended to the human remedy on the rows §7.1 marks ★.
    pub fn store_location(&self) -> &'static str {
        match self {
            Self::Mcp => "config/mcp.toml",
            Self::Plugin => ".permissions.toml",
        }
    }
}

/// The ledger key. Derived from a `RegisteredTool`, never stored on one
/// (design §3.1 — `RegisteredTool` has 79 construction sites and no `Default`).
///
/// For plugins the `name` is the **directory** name, not the manifest's
/// self-declared `plugin.name` (design §2.2, X-3).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ExtensionId {
    pub kind: ExtensionKind,
    pub name: String,
}

impl ExtensionId {
    pub fn mcp(name: impl Into<String>) -> Self {
        Self {
            kind: ExtensionKind::Mcp,
            name: name.into(),
        }
    }

    pub fn plugin(name: impl Into<String>) -> Self {
        Self {
            kind: ExtensionKind::Plugin,
            name: name.into(),
        }
    }
}

impl fmt::Display for ExtensionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind.as_str(), self.name)
    }
}

/// The owner's persisted toggle. `enabled` in `mcp.toml` / `.permissions.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disposition(pub bool);

/// The plugin consent axis, reported as the API row's `consent` field and
/// `null` for MCP — writing a server into your own `config/mcp.toml` *is* the
/// consent (design §8, §3.3 E1).
///
/// It is deliberately a **separate** word from the disposition: `denied` is a
/// consent decision, never the toggle position `disabled` (design §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Consent {
    Approved,
    /// No decision has been recorded — a decision-less entry, or none at all.
    Pending,
    Denied,
}

impl Consent {
    pub fn word(&self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Pending => "pending",
            Self::Denied => "denied",
        }
    }

    /// The tri-state `approved` field of a `.permissions.toml` entry read as a
    /// consent word: `None` is *pending* for a missing entry and for a
    /// decision-less one alike (design §5).
    pub fn from_approved(approved: Option<bool>) -> Self {
        match approved {
            Some(true) => Self::Approved,
            Some(false) => Self::Denied,
            None => Self::Pending,
        }
    }
}

/// What a plugin's manifest **declares**, read at scan and never a cache of
/// runtime discovery (design §8, X-19).
///
/// It is what lets an `unapproved`/`disabled`/`failed` row show what the plugin
/// asks for without inventing tool names: manifest declarations are static and
/// cannot go stale the way discovered names can.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredContributions {
    pub capabilities: Vec<String>,
    pub virtual_capabilities: Vec<String>,
    /// `plugin.toml`'s `[types]` table, as declared.
    pub types: std::collections::BTreeMap<String, bool>,
}

/// What class of thing an extension contributed. Tools are the only class
/// registered today; MCP resources and prompts are stubbed. The ledger's
/// retained map is keyed by `(ContributionKind, name)` from C1 so a withdrawn
/// resource URI can be attributed the way a tool name is (design §2.3, X-36).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContributionKind {
    Tool,
    Resource,
    Prompt,
}

// ============================================================================
// States
// ============================================================================

/// Observed reality, in memory only, never persisted (design §4). One enum
/// across both kinds — only *reachability* differs, which a single enum
/// expresses for free.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionState {
    /// Loaded, tools published, calls pass.
    Enabled,
    /// The owner turned it off — S2: unloaded.
    Disabled,
    /// Consent gate not passed (plugins).
    Unapproved { reason: UnapprovedReason },
    Failed {
        reason: FailureReason,
        detail: String,
        since: chrono::DateTime<chrono::Utc>,
    },
    /// PLUGIN-ONLY: a `.permissions.toml` entry whose directory is gone.
    Orphaned,
    /// Transient, never persisted; reported literally as "enabling".
    Enabling,
    /// Transient, never persisted; reported literally as "disabling".
    Disabling,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnapprovedReason {
    NeverSeen,
    Denied,
    CapabilitiesGrew { added: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureReason {
    /// actionable
    NeedsAuthorization,
    /// actionable
    NeedsConfig { missing: Vec<String> },
    /// actionable (bad declaration)
    ConfigInvalid,
    /// not actionable — retry
    Unreachable,
    /// not actionable — retry
    Crashed,
}

impl FailureReason {
    /// Drives the GUI's tag tone and CTA. Total by construction, so a future
    /// reason code cannot silently render without one (design §4.2).
    pub fn actionable(&self) -> bool {
        matches!(
            self,
            Self::NeedsAuthorization | Self::NeedsConfig { .. } | Self::ConfigInvalid
        )
    }

    /// The API row's `reason` field (design §8).
    pub fn word(&self) -> &'static str {
        match self {
            Self::NeedsAuthorization => "needs_authorization",
            Self::NeedsConfig { .. } => "needs_config",
            Self::ConfigInvalid => "config_invalid",
            Self::Unreachable => "unreachable",
            Self::Crashed => "crashed",
        }
    }
}

impl UnapprovedReason {
    /// The API row's `reason` field (design §8).
    pub fn word(&self) -> &'static str {
        match self {
            Self::NeverSeen => "never_seen",
            Self::Denied => "denied",
            Self::CapabilitiesGrew { .. } => "capabilities_grew",
        }
    }
}

impl ExtensionState {
    /// The API row's `state` field (design §8). `Enabling`/`Disabling` are
    /// reported literally, never as their target state.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Unapproved { .. } => "unapproved",
            Self::Failed { .. } => "failed",
            Self::Orphaned => "orphaned",
            Self::Enabling => "enabling",
            Self::Disabling => "disabling",
        }
    }

    /// The API row's `reason` field, `None` for states that carry no reason.
    pub fn reason_word(&self) -> Option<&'static str> {
        match self {
            Self::Unapproved { reason } => Some(reason.word()),
            Self::Failed { reason, .. } => Some(reason.word()),
            _ => None,
        }
    }

    /// The API row's `actionable` field — derived, never hand-set.
    pub fn actionable(&self) -> bool {
        match self {
            Self::Failed { reason, .. } => reason.actionable(),
            _ => false,
        }
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

// ============================================================================
// Withdrawal vocabulary
// ============================================================================

/// What ran T1's withdrawal — the wording of the dependent scan, the
/// `ExtensionCapabilityWithdrawn` event and the cron notice is keyed on this,
/// not on the transient state (design §3.2 T1 step 3, §7.3).
///
/// Also stored on the record as `pending_cause` by `begin(ext, Disabling,
/// cause)` so a `reload`'s T0–E5 window reads *reloading*, never *being turned
/// off* (design §3.2 T0, §3.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WithdrawalCause {
    Disable,
    Watcher,
    DeclarationGone,
    Deny,
    Reload,
    Crash,
    ServerListChange,
}

impl WithdrawalCause {
    /// The literal the `ServerEvent` peer and the event log carry — the same
    /// word `serde(rename_all = "snake_case")` produces.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Disable => "disable",
            Self::Watcher => "watcher",
            Self::DeclarationGone => "declaration_gone",
            Self::Deny => "deny",
            Self::Reload => "reload",
            Self::Crash => "crash",
            Self::ServerListChange => "server_list_change",
        }
    }

    /// The §7.3 phrase with **no** `<detail>` interpolated, for a surface on
    /// which the detail must travel separately, wrapped (§7.1) — the cron
    /// notice, which is a chat row the model reads back. `Crash` is the only
    /// row that reads `detail` at all.
    pub fn wording_without_detail(&self, ext: &ExtensionId) -> String {
        match self {
            Self::Crash => "stopped running (crashed)".to_string(),
            other => other.wording(ext, ""),
        }
    }

    /// The §7.3 wording this cause keys. `detail` is only read by `Crash`.
    pub fn wording(&self, ext: &ExtensionId, detail: &str) -> String {
        match self {
            Self::Disable | Self::Watcher | Self::DeclarationGone => "disabled".to_string(),
            Self::Deny => "denied".to_string(),
            Self::Reload => "reloading".to_string(),
            Self::Crash => format!("stopped running (crashed: {detail})"),
            Self::ServerListChange => {
                format!("withdrawn by the server '{}' (still enabled)", ext.name)
            }
        }
    }
}

/// When a capability withholding was observed. Carried on
/// `SystemEvent::ExtensionCapabilityWithheld` (C4) and used as the third
/// component of the warn-dedup key (design §7.4).
///
/// `ScheduledSkip` is exempt from dedup — each cron fire is a distinct
/// unattended event and its scope key is the skill id (design §6.2 #13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Moment {
    AttemptedUse,
    SurfaceAssembly,
    ScheduledSkip,
}

impl Moment {
    /// The literal the `ServerEvent` peer and the event log carry — the same
    /// word `serde(rename_all = "snake_case")` produces.
    pub fn word(&self) -> &'static str {
        match self {
            Self::AttemptedUse => "attempted_use",
            Self::SurfaceAssembly => "surface_assembly",
            Self::ScheduledSkip => "scheduled_skip",
        }
    }
}

// ============================================================================
// Supervisor
// ============================================================================

/// A supervisor-level refusal. The routes (C6) map these to the status codes
/// design §8 fixes; nothing here decides a status code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExtensionError {
    /// No such extension — `404`.
    #[error("unknown extension '{0}'")]
    NotFound(ExtensionId),
    /// `reload` from `Disabled` / `Unapproved{*}` / `Orphaned` — `409 not_loaded`.
    #[error("not_loaded")]
    NotLoaded,
    /// The disposition store cannot be read, so no verb may take a transition
    /// (design §4) — `409 store_unreadable`.
    #[error("store_unreadable")]
    StoreUnreadable(String),
    /// A verb this kind does not have — `409 unsupported_for_kind`.
    #[error("unsupported_for_kind")]
    UnsupportedForKind,
    /// The row is `Orphaned` and only `DELETE` applies — `409 not_orphaned`.
    #[error("not_orphaned")]
    NotOrphaned,
    /// Step W failed, so no CAS was taken and nothing changed — `500`.
    #[error("extension store write failed: {0}")]
    WriteFailed(String),
    #[error("{0}")]
    Internal(String),
}

/// The verbs both supervisors implement (design §3).
///
/// `enable`/`disable` are the two `set_enabled(name, bool)` actuators seen
/// through the trait — **write the bit, then reconcile** — not a second API
/// beside them. The plugin-only `approve`/`deny`/`remove_orphan` and the
/// MCP-only `on_tool_list_changed` live on their implementations, not here.
#[async_trait::async_trait]
pub trait ExtensionSupervisor: Send + Sync {
    /// W (write `enabled = true`) then E0–E5. `200` even when bring-up fails.
    async fn enable(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError>;

    /// W (write `enabled = false`) then T0–T5.
    async fn disable(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError>;

    /// T0–T4 then E0–E5 under one hold of the per-extension mutex, bit
    /// untouched, no W (design §3.4.1).
    async fn reload(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError>;

    /// Bring one extension's reality in line with its declaration + bit.
    async fn reconcile(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError>;

    /// The boot and watcher entry point: diff desired against actual.
    async fn reconcile_all(&self);

    /// Every record this supervisor owns, for `GET /v1/extensions`.
    async fn list(&self) -> Vec<ExtensionRecord>;

    /// T2–T4 for every `Enabled` extension (design §3.5).
    async fn shutdown_all(&self);
}

#[cfg(test)]
mod tests;
