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
/// `state_json`/`outcome_json` columns), plus the fields the handler used to
/// post-inject via `serde_json::Value::as_object_mut()`:
/// `assigned_agents` (always present, possibly empty) and `outcome` (present
/// only when the task's `outcome_json` parses — see `parse_outcome`), plus
/// `cost_usd` (GAP-08b), sourced from a single grouped
/// `LlmUsageRepository::cost_for_tasks` query over the page's task ids and
/// defaulted to 0.0 for a task with no logged LLM calls.
///
/// This is the "cheap half" of task-shape normalisation (plan §7); the full
/// `GET /v1/tasks` vs `/{id}` unification lands in Phase 4 with P8.
/// One swimlane of `GET /v1/tasks/{id}/timeline` — the `TimelineLane` the GUI
/// is written against, field for field.
///
/// `state` is the *reported* state, not always the stored one: a span left
/// `running` on a terminal task reports `cancelled`/`"interrupted"`, and a
/// span whose agent instance is waiting on a confirmation reports `blocked`.
/// Neither is ever written to the row — both are properties of *now*.
///
/// `steps_current`/`steps_total` are omitted rather than guessed: nothing
/// counts steps inside a subagent's loop today, and a `0/0` would read as a
/// lane that did nothing.
#[derive(Debug, Clone, Serialize)]
pub struct TimelineLaneResponse {
    /// The span id — the spawn's `node_id`.
    pub lane_id: String,
    pub label: String,
    pub template_id: String,
    pub agent_instance_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub state: String,
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps_current: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps_total: Option<i32>,
}

/// `GET /v1/tasks/{id}/timeline` (GAP-09).
///
/// `now` is the server's clock at the moment of the read: the axis needs a
/// right-hand edge for a run still in flight, and taking it from the client
/// would drift against the lane times, which are the daemon's.
#[derive(Debug, Clone, Serialize)]
pub struct TaskTimelineResponse {
    pub task_id: String,
    pub started_at: String,
    pub now: String,
    pub completed_at: Option<String>,
    pub lanes: Vec<TimelineLaneResponse>,
}

#[derive(Debug, Serialize)]
pub struct TaskSummaryResponse {
    #[serde(flatten)]
    pub task: Task,
    /// Sourced from `agent_task_history`; see `agent_runs_summary` in `tasks.rs`.
    pub assigned_agents: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ParsedOutcomeFields>,
    pub cost_usd: f64,
}
