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
        }
    }

    /// Whether this status represents a terminal (final) state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
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
}

impl OutcomeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TextOnly => "text_only",
            Self::ArtifactOnly => "artifact_only",
            Self::Mixed => "mixed",
            Self::Failed => "failed",
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
}

