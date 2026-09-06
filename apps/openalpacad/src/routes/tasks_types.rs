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

/// `POST /v1/tasks/{id}/steer` (GAP-02) — a user interjection addressed at one
/// *run*, which is what a GUI has in hand. The lane is not a field: it is read
/// from the run's own `source_lane`, so a client cannot steer a workflow into
/// somebody else's conversation.
#[derive(Debug, Deserialize)]
pub struct SteerTaskRequest {
    pub message: String,
    /// The project this interjection belongs to. Omitted, it is the run's own
    /// `workspace_id`, so a message that outlives the workflow and re-enters as
    /// an `unprocessed_steering` follow-up is scoped to the same project.
    #[serde(default)]
    pub workspace_path: Option<String>,
}

/// `POST /v1/tasks/{id}/steer` — what the queue accepted.
///
/// `accepted` is deliberately not a promise the workflow *read* the message:
/// the rail drains at the next round boundary, so `inbox_depth` (the queue
/// depth after this push) is the only honest acknowledgement there is.
#[derive(Debug, Serialize)]
pub struct SteerTaskResponse {
    pub task_id: String,
    pub accepted: bool,
    pub inbox_depth: usize,
    pub lane_key: String,
}

/// `GET /v1/tasks/{id}`.
///
/// The legacy `assignments` array (`agent_task_history` rows under a serde
/// rename) was deleted with Phase 4's P8: it could only describe a subagent
/// that had already returned, while `GET /v1/tasks/{id}/timeline` serves every
/// lane of the run — in flight, blocked or finished — from `subagent_span`.
#[derive(Debug, Serialize)]
pub struct TaskResponse {
    pub task: Task,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ParsedOutcomeFields>,
}

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

/// Row shape served by `GET /v1/tasks` — a `Task`'s own fields flattened to
/// the top level (matching `Task`'s `#[serde(skip)]`s on the internal
/// `state_json`/`outcome_json` columns), plus `outcome` (present only when the
/// task's `outcome_json` parses — see `parse_outcome`) and `cost_usd`
/// (GAP-08b), sourced from a single grouped `LlmUsageRepository::cost_for_tasks`
/// query over the page's task ids and defaulted to 0.0 for a task with no
/// logged LLM calls.
///
/// The `assigned_agents` summary array went with P8: a run's agents are the
/// timeline's business (`GET /v1/tasks/{id}/timeline`), and building it here
/// cost one `agent_task_history` query per row of every page. What a list row
/// keeps of it is `subagent_count` — how many agents the run spawned, from one
/// grouped `SubagentSpanRepository::counts_for_tasks` query over the page's
/// ids, defaulted to 0 for a run with no spans. Counting spans means an agent
/// still working is counted; `agent_task_history` only ever knew about the
/// ones that had already returned.
#[derive(Debug, Serialize)]
pub struct TaskSummaryResponse {
    #[serde(flatten)]
    pub task: Task,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ParsedOutcomeFields>,
    pub cost_usd: f64,
    pub subagent_count: i64,
}
