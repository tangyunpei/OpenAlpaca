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
    /// **Ignored** (ruling R79): the stored `created_by` is always the local
    /// user. It is the column every owner-scoped verb reads — `start`, `rerun`,
    /// `resume` and `steer` all refuse a run this caller did not create — so a
    /// client-supplied value would let one request mint a row nobody can
    /// operate, or one attributed to somebody else. Kept on the wire because
    /// removing a required field breaks every existing caller for nothing.
    pub created_by: String,
    /// The lane the run belongs to, and the lane the caller must own: a run
    /// launched onto a lane posts its report into that conversation, so naming
    /// someone else's is `404 LANE_NOT_FOUND` (R79, R40's line — never `403`).
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
    /// `"cancel"` | `"pause"` | `"resume"` — state transitions applied by
    /// `apply_task_action` — plus `"start"`, which is not a transition at all
    /// but a dispatch of the stored row under its own id (D5, GAP-06) and is
    /// answered by the route before it reaches that function.
    pub action: String,
}

/// `POST /v1/tasks/{id}/rerun` (GAP-06) — `201`, and the id in it is **not**
/// the one in the path.
///
/// That asymmetry with `start` is the whole design: a re-run is a second run of
/// the same goal, and the first one's result is the thing the user is comparing
/// against, so it keeps its row and the copy gets a new id. `source_task_id` is
/// the path's id echoed back, because a client that has just been handed an
/// unfamiliar id needs to know what it came from — and it is a stored column,
/// so the link outlives this response.
#[derive(Debug, Serialize)]
pub struct RerunTaskResponse {
    /// The **new** run.
    pub task_id: String,
    /// The run it was copied from — the id in the request path.
    pub source_task_id: String,
    pub title: String,
    /// `queued`, or `running` if the background half got there first. Read from
    /// the registry rather than assumed; both are true.
    pub status: String,
}

/// `POST /v1/tasks/{id}/steer` (GAP-02) — a user interjection addressed at one
/// *run*, which is what a GUI has in hand. The lane is not a field: it is read
/// from the run's own `source_lane`, so a client cannot steer a workflow into
/// somebody else's conversation.
#[derive(Debug, Deserialize)]
pub struct SteerTaskRequest {
    pub message: String,
    /// **Refused** when present (`400 WORKSPACE_PATH_IN_BODY`). The project a
    /// request carries is the `x-workspace-path` header, resolved through the
    /// one resolver every route shares (R22) — the sibling injecting route,
    /// `POST /v1/lanes/{lane}/followups`, already takes it that way and has no
    /// body field at all. Taken from the body it was stored **unresolved**: a
    /// subdirectory, a relative path or a path under no project marker went
    /// straight onto the steering message and from there into an
    /// `unprocessed_steering` follow-up, which is a `workspace_id` the daemon
    /// never agreed to.
    ///
    /// Kept on the wire so a client that still sends it is told, rather than
    /// having it silently ignored. Omitted — the ordinary case — the message
    /// takes the header's project, or the run's own `workspace_id` when the
    /// request carries no header.
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
    /// Whether `POST /v1/tasks/{id}/steer` would take a message for this run
    /// right now (R40) — see [`steerable`](crate::routes::tasks) for the three
    /// predicates. A property of *now*, like the timeline's derived lane
    /// states: it is computed at read time and never stored.
    ///
    /// It sits beside `task` rather than inside it because it is not a column:
    /// the run's row says nothing about who is asking or whether a workflow is
    /// still attached to it.
    pub steerable: bool,
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
    /// Whether `POST /v1/tasks/{id}/steer` would take a message for this run
    /// right now (R40) — the same flag, from the same predicates, that
    /// [`TaskResponse::steerable`] carries. A list row serves it so the run
    /// cards can disable `Steer` with a stated reason instead of letting the
    /// user discover the answer by sending one.
    pub steerable: bool,
}
