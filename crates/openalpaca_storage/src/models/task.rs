//! Task data models for the storage layer

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Status of a task in its lifecycle
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Paused,
    /// The daemon went away while this run was in flight (§5.6b).
    ///
    /// Written **only** by the boot sweep, for a row the previous incarnation
    /// left `queued` / `running` / `paused`. It is not a failure — nothing
    /// about the work went wrong — and saying `failed` with a fabricated
    /// message is what §5.6b calls the sweep lying.
    ///
    /// **Terminal.** A new incarnation cannot re-enter the loop that was
    /// running: its tokio task, its steering inbox and its in-memory history
    /// are gone. So the row is finished, and the restart affordance is Phase
    /// 5's `rerun` (a new id carrying `source_task_id` back to this one).
    /// `start` refuses an interrupted row exactly as it refuses any other
    /// terminal one (R43): it re-launches in place and would spend whatever
    /// partial result the run left behind.
    Interrupted,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Paused => "paused",
            Self::Interrupted => "interrupted",
        }
    }

    /// Whether this status represents a terminal (final) state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

impl std::str::FromStr for TaskStatus {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "paused" => Ok(Self::Paused),
            "interrupted" => Ok(Self::Interrupted),
            _ => anyhow::bail!("Invalid task status: {}", s),
        }
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The kind of outcome a completed task produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    TextOnly,
    ArtifactOnly,
    Mixed,
    Failed,
    /// The run produced no outcome because the daemon went away (§5.6b).
    /// Written by the boot sweep beside [`TaskStatus::Interrupted`], so a
    /// client reading `outcome_kind` alone is told the same truth the status
    /// tells and never sees `failed` for a crash.
    Interrupted,
}

impl OutcomeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TextOnly => "text_only",
            Self::ArtifactOnly => "artifact_only",
            Self::Mixed => "mixed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }
}

impl std::str::FromStr for OutcomeKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "text_only" => Ok(Self::TextOnly),
            "artifact_only" => Ok(Self::ArtifactOnly),
            "mixed" => Ok(Self::Mixed),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            _ => anyhow::bail!("Invalid outcome kind: {}", s),
        }
    }
}

impl std::fmt::Display for OutcomeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A task tracked in the system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub priority: i32,
    pub progress_current: Option<i32>,
    pub progress_total: Option<i32>,
    pub result_summary: Option<String>,
    pub created_by: String,
    pub source_lane: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing)]
    pub state_json: Option<String>,
    pub state_version: i32,
    #[serde(skip_serializing)]
    pub outcome_json: Option<String>,
    pub outcome_kind: Option<OutcomeKind>,
    pub artifact_count: i32,
    /// The project this run belonged to — the workspace root the *request*
    /// supplied (`x-workspace-path` on `POST /v1/chat`, `workspace_path` on
    /// `POST /v1/command`), already resolved to its root.
    ///
    /// `None` for every turn that arrived without one: connector lanes,
    /// scheduled skills, and any client that sends no workspace. Never derived
    /// from the daemon's current directory (ruling R22) — a CWD-derived value
    /// here would claim a run belonged to whatever repository the daemon
    /// happened to start in. Column added by migration 036.
    pub workspace_id: Option<String>,
    /// The run this one was copied from — set only by `rerun` (GAP-06), which
    /// dispatches a **new** id carrying the old row's goal.
    ///
    /// `None` for every other row, including one that `start` re-launched: that
    /// verb keeps the id (D5), so there are not two runs to link. Column and
    /// index added by migration 037.
    pub source_task_id: Option<String>,
    /// The session this run was started from — written at dispatch from the
    /// lane's active session (migration 039, §5.1).
    ///
    /// It is what makes the completion report land in the conversation that
    /// asked for the work, even when the user has since opened another one.
    /// `None` for a run dispatched on a lane that had no session row, and for
    /// every pre-039 row. Not a foreign key: deleting a session nulls this
    /// rather than taking the run's record with it.
    pub session_id: Option<String>,
}

