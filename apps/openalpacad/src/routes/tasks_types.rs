//! Request/response types for task management endpoints.

use openalpaca_core::orchestrator::ParsedOutcomeFields;
use openalpaca_storage::Task;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub priority: Option<i32>,
    pub created_by: String,
    pub source_lane: String,
}

#[derive(Debug, Deserialize)]
pub struct ListTasksQuery {
    pub created_by: Option<String>,
    pub status: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct TaskActionRequest {
    pub action: String, // "cancel", "pause", "resume"
}

#[derive(Debug, Serialize)]
pub struct TaskResponse {
    pub task: Task,
    /// Agent runs recorded for this task (from `agent_task_history`, written
    /// by the dispatcher's `record_agent_history`). Serialized under the
    /// legacy `assignments` key for client compatibility.
    #[serde(rename = "assignments", skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<openalpaca_storage::AgentTaskHistory>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ParsedOutcomeFields>,
}

/// Row shape served by `GET /v1/tasks` — a `Task`'s own fields flattened to
/// the top level (matching `Task`'s `#[serde(skip)]`s on the internal
/// `state_json`/`outcome_json` columns), plus the two fields the handler
/// used to post-inject via `serde_json::Value::as_object_mut()`:
/// `assigned_agents` (always present, possibly empty) and `outcome` (present
/// only when the task's `outcome_json` parses — see `parse_outcome`).
///
/// This is the "cheap half" of task-shape normalisation (plan §7); the full
/// `GET /v1/tasks` vs `/{id}` unification lands in Phase 4 with P8. A later
/// task adds a per-row `cost_usd` field here.
#[derive(Debug, Serialize)]
pub struct TaskSummaryResponse {
    #[serde(flatten)]
    pub task: Task,
    /// Sourced from `agent_task_history`; see `agent_runs_summary` in `tasks.rs`.
    pub assigned_agents: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ParsedOutcomeFields>,
}
