//! Task management endpoints
//!
//! POST /v1/tasks           -> create a new task
//! GET  /v1/tasks           -> list tasks (query: created_by, status, limit)
//! GET  /v1/tasks/{id}      -> get a single task + agent runs
//! GET  /v1/tasks/{id}/timeline -> the run's swimlanes (GAP-09)
//! POST /v1/tasks/{id}/action -> perform action (cancel, pause, resume, start)
//! POST /v1/tasks/{id}/steer  -> inject a message into a running run (GAP-02)
//! POST /v1/tasks/{id}/rerun  -> dispatch a new run from a finished one (GAP-06)
//!
//! Neither task shape carries agent runs any more: the legacy
//! `assigned_agents` / `assignments` payload (read from `agent_task_history`)
//! was deleted with Phase 4's P8, and the timeline route serves the same runs
//! from `subagent_span` — in flight as well as finished.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use std::sync::Arc;
use uuid::Uuid;

use openalpaca_core::bus::EventBus;
use openalpaca_core::context::SharedContext;
use openalpaca_core::daemon_config::RoutingConfig;
use openalpaca_core::events::SystemEvent;
use openalpaca_core::orchestrator::{
    Orchestrator, TaskActionError, TaskLaunchError, apply_task_action, parse_outcome,
};
use openalpaca_core::runner::steering::{SteeringMsg, SteeringPushError, push_steering};
use openalpaca_core::security::confirmation::ConfirmationBroker;
use openalpaca_core::security::policy::{Principal, Scope};
use openalpaca_storage::{
    Database, LlmUsageRepository, SPAN_DETAIL_INTERRUPTED, SubagentSpanRepository, Task,
    TaskRepository, TaskStatus,
};

use super::{api_error, tasks_types::*};
use crate::AppState;

// ── Handlers ──────────────────────────────────────────────────────

/// POST /v1/tasks
pub async fn create_task_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateTaskRequest>,
) -> impl IntoResponse {
    // Input validation
    if request.title.is_empty() || request.title.len() > 500 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Title must be 1-500 characters" })),
        );
    }
    if let Some(ref desc) = request.description
        && desc.len() > 10_000
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Description must be at most 10000 characters" })),
        );
    }

    let task_id = Uuid::new_v4().to_string();
    let now = Utc::now();

    let task = Task {
        id: task_id.clone(),
        title: request.title.clone(),
        description: request.description.clone(),
        status: TaskStatus::Queued,
        priority: request.priority.unwrap_or(0),
        progress_current: None,
        progress_total: None,
        result_summary: None,
        created_by: request.created_by.clone(),
        source_lane: request.source_lane.clone(),
        created_at: now,
        updated_at: now,
        completed_at: None,
        state_json: None,
        state_version: 0,
        outcome_json: None,
        outcome_kind: None,
        artifact_count: 0,
        // `CreateTaskRequest` carries no workspace and this route is not a
        // dispatch — the project is recorded where a run actually starts
        // (`dispatch_lead_agent`, §4.7 item 3). `None` says "no project", which
        // is true of a row created here.
        workspace_id: None,
        // Nor is it a re-run: `POST /v1/tasks/{id}/rerun` is the verb that
        // fills this, and it dispatches rather than passing through here.
        source_task_id: None,
        // Same reason as `workspace_id`: the session a run belongs to is
        // resolved where a run actually starts (§5.1), not on a route that
        // only parks a row.
        session_id: None,
    };

    // 1. Persist to DB
    let repo = TaskRepository::new(&state.db);
    if let Err(e) = repo.create(&task) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        );
    }

    // 2. Register in-memory
    state
        .gateway
        .shared_context
        .task_registry
        .register(task_id.clone(), request.title.clone());

    // 3. Create task lane
    state.gateway.lane_manager.create_task_lane(&task_id);

    // 4. Emit event
    let _ = state.gateway.bus.publish(SystemEvent::TaskCreated {
        task_id: task_id.clone(),
        title: request.title,
        created_by: request.created_by,
        timestamp: now,
    });

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "task_id": task_id,
            "status": "queued"
        })),
    )
}

/// GET /v1/tasks
pub async fn list_tasks_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListTasksQuery>,
) -> impl IntoResponse {
    let repo = TaskRepository::new(&state.db);
    let limit = query.limit.unwrap_or(50);

    let tasks = if let Some(ref created_by) = query.created_by {
        repo.list_by_creator(created_by, limit)
    } else if let Some(ref status_str) = query.status {
        if status_str == "active" {
            repo.list_active(limit)
        } else {
            match status_str.parse::<TaskStatus>() {
                Ok(status) => repo.list_by_status(status, limit),
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(
                            serde_json::json!({ "error": format!("Invalid status: {}", status_str) }),
                        ),
                    );
                }
            }
        }
    } else {
        repo.list_recent(limit)
    };

    match tasks {
        Ok(tasks) => {
            // GAP-08b: one grouped query for every task on the page, rather
            // than a per-row lookup. R38's agent count is the same shape over
            // `subagent_span` — the per-row agent signal the deleted
            // `assigned_agents` array carried, at one query per page instead
            // of one per row. A read failure yields an empty map, so the page
            // renders with zeroes rather than 500-ing.
            let task_ids: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
            let costs = LlmUsageRepository::new(&state.db)
                .cost_for_tasks(&task_ids)
                .unwrap_or_default();
            let subagent_counts = SubagentSpanRepository::new(&state.db)
                .counts_for_tasks(&task_ids)
                .unwrap_or_else(|e| {
                    tracing::warn!("Failed to read subagent counts for task list: {e}");
                    Default::default()
                });
            let summaries: Vec<TaskSummaryResponse> = tasks
                .into_iter()
                .map(|t| {
                    let outcome = parse_outcome(&t);
                    let cost_usd = costs.get(&t.id).copied().unwrap_or(0.0);
                    let subagent_count = subagent_counts.get(&t.id).copied().unwrap_or(0);
                    // R40 — an in-memory lookup per row (the registered inbox),
                    // no query: the list stays one page, three queries.
                    let steerable =
                        is_steerable(&t, &state.gateway.shared_context, &state.local_user_id);
                    TaskSummaryResponse {
                        task: t,
                        outcome,
                        cost_usd,
                        subagent_count,
                        steerable,
                    }
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::to_value(summaries).unwrap_or_else(|_| serde_json::json!([]))),
            )
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// GET /v1/tasks/{id}
pub async fn get_task_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let repo = TaskRepository::new(&state.db);

    match repo.get(&id) {
        Ok(Some(task)) => {
            let outcome = parse_outcome(&task);
            let steerable =
                is_steerable(&task, &state.gateway.shared_context, &state.local_user_id);
            (
                StatusCode::OK,
                Json(
                    serde_json::to_value(TaskResponse {
                        task,
                        outcome,
                        steerable,
                    })
                    .unwrap_or_else(|_| serde_json::json!({"error": "serialization_failed"})),
                ),
            )
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Task not found" })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// The lane a pending confirmation is blocking, and the tool it is waiting on.
///
/// Keyed by agent *instance*, because that is what a span is: the template id
/// would blur two lanes of the same kind into one.
fn blocked_lanes(
    broker: Option<&ConfirmationBroker>,
    task_id: &str,
) -> std::collections::HashMap<String, String> {
    let Some(broker) = broker else {
        return std::collections::HashMap::new();
    };
    let mut blocked = std::collections::HashMap::new();
    for request in broker.pending_requests() {
        if request.task_id.as_deref() != Some(task_id) {
            continue;
        }
        if let Some(instance) = request.agent_instance_id {
            // First pending request wins: a lane can only be waiting on one
            // prompt at a time, and the extra ones are queued behind it.
            blocked.entry(instance).or_insert(request.tool_name);
        }
    }
    blocked
}

fn stamp(at: chrono::DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Assemble one run's timeline from its stored spans plus what is pending
/// *right now*. Split out from the handler so every derivation rule is
/// testable without a router.
fn build_timeline(
    task: &Task,
    spans: Vec<openalpaca_storage::SubagentSpanRecord>,
    blocked: &std::collections::HashMap<String, String>,
    now: chrono::DateTime<Utc>,
) -> TaskTimelineResponse {
    let terminal = task.status.is_terminal();
    let lanes = spans
        .into_iter()
        .map(|span| {
            let running = span.state == "running";
            let (state, detail) = if running && terminal {
                // The daemon that would have closed this lane is gone (or the
                // task finalized without it): report the truth, which is that
                // the lane never finished, not that it is still working.
                (
                    "cancelled".to_string(),
                    Some(SPAN_DETAIL_INTERRUPTED.to_string()),
                )
            } else if running && let Some(tool) = blocked.get(&span.agent_instance_id) {
                ("blocked".to_string(), Some(format!("waiting on {tool}")))
            } else {
                (span.state, span.detail)
            };
            TimelineLaneResponse {
                lane_id: span.id,
                label: span.label,
                template_id: span.template_id,
                agent_instance_id: span.agent_instance_id,
                started_at: span.started_at,
                ended_at: span.ended_at,
                state,
                detail,
                steps_current: None,
                steps_total: None,
            }
        })
        .collect();

    TaskTimelineResponse {
        task_id: task.id.clone(),
        started_at: stamp(task.created_at),
        now: stamp(now),
        completed_at: task.completed_at.map(stamp),
        lanes,
    }
}

/// `GET /v1/tasks/{id}/timeline` — the run's swimlanes (GAP-09).
fn task_timeline(
    db: &Database,
    broker: Option<&ConfirmationBroker>,
    id: &str,
) -> (StatusCode, Json<serde_json::Value>) {
    match TaskRepository::new(db).get(id) {
        Ok(Some(task)) => {
            let spans = SubagentSpanRepository::new(db)
                .list_for_task(id)
                .unwrap_or_else(|e| {
                    tracing::warn!(task_id = id, "Failed to read subagent spans: {e}");
                    Vec::new()
                });
            let blocked = blocked_lanes(broker, id);
            let timeline = build_timeline(&task, spans, &blocked, Utc::now());
            (
                StatusCode::OK,
                Json(
                    serde_json::to_value(timeline)
                        .unwrap_or_else(|_| serde_json::json!({"error": "serialization_failed"})),
                ),
            )
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Task not found" })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// GET /v1/tasks/{id}/timeline
pub async fn get_task_timeline_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    task_timeline(&state.db, state.confirmation_broker.as_deref(), &id)
}

/// POST /v1/tasks/{id}/action
pub async fn task_action_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<TaskActionRequest>,
) -> Response {
    // D5 — `start` is answered here, before `apply_task_action`, because it is
    // not a state transition at all: nothing about the row changes on its own,
    // a lead agent is dispatched onto it. `apply_task_action` knows three
    // transitions and would report `UnknownAction` for this one.
    if request.action == START_ACTION {
        return start_task(&state.orchestrator, &state.db, &state.local_user_id, &id);
    }

    // Shared with the orchestrator chat handler: registry-first resolution with
    // DB fallback, transition validation, token cancel, persistence, lane sync,
    // and TaskUpdated event all live in core.
    match apply_task_action(
        &state.gateway.shared_context,
        &state.gateway.lane_manager,
        &state.gateway.bus,
        Some(&state.db),
        &id,
        &request.action,
    ) {
        Ok(new_status) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "task_id": id,
                "status": new_status.as_str()
            })),
        ),
        Err(TaskActionError::NotFound) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Task not found" })),
        ),
        Err(TaskActionError::CannotCancel { current }) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": format!("Cannot cancel a task in '{}' state", current)
            })),
        ),
        Err(TaskActionError::CannotPause { current }) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": format!("Can only pause a running task, current state: '{}'", current)
            })),
        ),
        Err(TaskActionError::CannotResume { current }) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": format!("Can only resume a paused task, current state: '{}'", current)
            })),
        ),
        Err(TaskActionError::UnknownAction) => unknown_action(&request.action),
        Err(TaskActionError::Db(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        ),
    }
    .into_response()
}

/// Every word `POST /v1/tasks/{id}/action` accepts: the three transitions
/// `apply_task_action` knows, plus [`START_ACTION`], which this route answers
/// itself. Named so the refusal cannot list a set the route does not honour.
const VALID_ACTIONS: &str = "cancel, pause, resume, start";

/// The `400` for a word that is none of [`VALID_ACTIONS`]. Keeps the ad-hoc
/// `{"error": "…"}` envelope its three sibling arms use — §7 says not to
/// retrofit those now, and half-converting one handler would be worse.
fn unknown_action(action: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": format!("Unknown action: '{action}'. Valid: {VALID_ACTIONS}")
        })),
    )
}

// ── Re-run and start (GAP-06) ─────────────────────────────────────
//
// Two verbs, deliberately different shapes, because they are different things:
//
//   POST /v1/tasks/{id}/rerun            201, a **new** id
//   POST /v1/tasks/{id}/action {"start"} 200, the **same** id (D5)
//
// A re-run is a second run of a finished run's goal, and both rows have to
// survive — the first one's result is what the user is comparing against — so
// it gets its own id and its own row, carrying `source_task_id` back to the
// original. `start` is not a second anything: it dispatches a row that was
// queued and never ran, and the client is already holding that id.
//
// `start` rides the existing action route rather than getting one of its own
// because that is where a client already looks for "do something to this run",
// and D5 fixed its shape (same id, `200`) to be exactly what the other three
// actions answer with.
//
// Both are owner-scoped, on the same line `POST /v1/tasks/{id}/steer` drew
// (R40): reading a run and cancelling it are not scoped, but a verb that puts
// work into the daemon *as this user* is.

/// The action word the route intercepts before [`apply_task_action`].
const START_ACTION: &str = "start";

/// `Ok(())` when `id` names a run this owner started; the refusal otherwise.
///
/// A run this caller cannot see gets the same `404` a missing one does — never
/// a `403`, which would confirm that the id belongs to somebody.
fn owned_run(db: &Database, owner_id: &str, id: &str) -> Result<(), Response> {
    match TaskRepository::new(db).get(id) {
        Ok(Some(task)) if task.created_by == owner_id => Ok(()),
        Ok(_) => Err(api_error(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "Task not found",
        )),
        Err(e) => Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            e.to_string(),
        )),
    }
}

/// Which verb a [`TaskLaunchError`] came from — the two disagree about exactly
/// one thing, which is what to call a row with no goal to dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchVerb {
    Rerun,
    Start,
}

/// One response per refusal, so a client can say which one happened without
/// parsing prose.
fn launch_refusal(error: TaskLaunchError, verb: LaunchVerb) -> Response {
    match error {
        TaskLaunchError::NotFound => {
            api_error(StatusCode::NOT_FOUND, "NOT_FOUND", "Task not found")
        }
        // `rerun` only. A run still in flight already has an agent on this
        // goal; steering or cancelling it is the answer, not a second one.
        TaskLaunchError::NotTerminal { current } => api_error(
            StatusCode::CONFLICT,
            "TASK_NOT_TERMINAL",
            format!(
                "This run has not finished ({current}) — steer or cancel it instead of \
                 re-running it."
            ),
        ),
        // `start` only (R43). A `start` re-launches the row *in place*, so a
        // finished run would lose its result to the relaunch; `409` because the
        // request is fine and the row's state is what refuses it, and the
        // message names `rerun` because that verb does exactly what the caller
        // wanted, without spending the first run's answer.
        TaskLaunchError::NotStartable { current } => api_error(
            StatusCode::CONFLICT,
            "TASK_NOT_STARTABLE",
            format!(
                "This run has already finished ({current}) — re-run it instead of starting it \
                 again, which would discard its result."
            ),
        ),
        // `start` only, and the reason the verb needs a compare-and-set rather
        // than a look: two of these would put two lead agents on one id.
        TaskLaunchError::AlreadyRunning => api_error(
            StatusCode::CONFLICT,
            "TASK_ALREADY_RUNNING",
            "This run is already running.",
        ),
        // `422`, not `400`: the request is well-formed and names a real run —
        // it is the stored row that has nothing a lead agent could be given.
        //
        // `start`'s word here is `TASK_NOT_DISPATCHABLE`, not the obvious
        // `TASK_NOT_STARTABLE` (R44): that one is spent above, on the `409` for
        // a run that has already finished. One code word cannot carry two
        // conditions at two statuses — a client that switched on it could not
        // tell "this row has no goal" from "this run is over", and the two want
        // opposite next steps.
        TaskLaunchError::NoDescription => match verb {
            LaunchVerb::Rerun => api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "TASK_NOT_RERUNNABLE",
                "This run has no description to re-dispatch — there is no goal to give a \
                 lead agent.",
            ),
            LaunchVerb::Start => api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "TASK_NOT_DISPATCHABLE",
                "This task has no description to dispatch — there is no goal to give a \
                 lead agent.",
            ),
        },
        // Capacity, not a bug: every agent template that could lead a run is
        // busy. `503` says so, and says it is worth trying again.
        TaskLaunchError::Dispatch(reason) => {
            api_error(StatusCode::SERVICE_UNAVAILABLE, "DISPATCH_FAILED", reason)
        }
        TaskLaunchError::Db(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e),
    }
}

/// `POST /v1/tasks/{id}/rerun`, as a `Response`. Split out from the handler so
/// every status code is provable without a router or an `AppState`.
fn rerun_task(orchestrator: &Orchestrator, db: &Database, owner_id: &str, id: &str) -> Response {
    if let Err(refusal) = owned_run(db, owner_id, id) {
        return refusal;
    }
    match orchestrator.rerun_task(id) {
        Ok(outcome) => (
            StatusCode::CREATED,
            Json(RerunTaskResponse {
                task_id: outcome.task_id,
                source_task_id: outcome.source_task_id,
                title: outcome.title,
                status: outcome.status,
            }),
        )
            .into_response(),
        Err(e) => launch_refusal(e, LaunchVerb::Rerun),
    }
}

/// `POST /v1/tasks/{id}/action {"action":"start"}` (D5), as a `Response`.
///
/// The success body is the shape the other three actions answer with, because
/// from the client's side that is what happened: the run it named changed
/// state. The id in it is the id it sent.
fn start_task(orchestrator: &Orchestrator, db: &Database, owner_id: &str, id: &str) -> Response {
    if let Err(refusal) = owned_run(db, owner_id, id) {
        return refusal;
    }
    match orchestrator.start_task(id) {
        Ok(outcome) => Json(serde_json::json!({
            "task_id": outcome.task_id,
            "status": outcome.status,
        }))
        .into_response(),
        Err(e) => launch_refusal(e, LaunchVerb::Start),
    }
}

/// POST /v1/tasks/{id}/rerun
pub async fn rerun_task_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    rerun_task(&state.orchestrator, &state.db, &state.local_user_id, &id)
}

// ── Steerability, in one place (R40) ──────────────────────────────
//
// Every task row carries a `steerable` flag and the steer route enforces the
// same three predicates. They are written once, here, because a hint the route
// would not honour is worse than no hint at all: the client would disable a
// control that works, or offer one that answers 404 forever.
//
// Only the steer route is owner-scoped — reading a run and cancelling it are
// not, deliberately (that asymmetry is a task-surface decision left to the
// owner). What this flag does is make the asymmetry legible on the row.

/// Why a run cannot take a steering message right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SteerRefusal {
    /// Started somewhere else — a connector run carries
    /// `created_by = "<provider>:<id>"`, never this daemon's local user. A
    /// steer injects text into somebody else's running agent, so the route
    /// answers the same `404` it gives for a run that does not exist.
    NotOwned,
    /// The row has reached a terminal status. Its inbox is usually closed
    /// already; this closes the window between `cancel` marking the row and
    /// the workflow closing the inbox on its way out.
    Terminal,
    /// No live inbox: the workflow never registered one (queued, steering
    /// disabled, or a daemon generation that is gone) or it has closed.
    NoInbox,
}

/// The three predicates, evaluated in the order the route reports them.
fn steer_refusal(
    task: &Task,
    shared_context: &SharedContext,
    owner_id: &str,
) -> Option<SteerRefusal> {
    if task.created_by != owner_id {
        return Some(SteerRefusal::NotOwned);
    }
    if task.status.is_terminal() {
        return Some(SteerRefusal::Terminal);
    }
    match shared_context.steering_inbox(&task.id) {
        Some(inbox) if !inbox.is_closed() => None,
        _ => Some(SteerRefusal::NoInbox),
    }
}

/// The `steerable` flag both task shapes serve.
///
/// The rail's `steering_enabled` switch is not a fourth predicate: with the
/// rail off no workflow registers an inbox at all (`dispatch_lead_agent`), so
/// such a run already reports `false` through `NoInbox`.
fn is_steerable(task: &Task, shared_context: &SharedContext, owner_id: &str) -> bool {
    steer_refusal(task, shared_context, owner_id).is_none()
}

// ── POST /v1/tasks/{id}/steer (GAP-02) ────────────────────────────
//
// Pure reuse. The steering rail is `runner/steering.rs`, it is already
// task-addressed, and `push_steering` already publishes `WorkflowSteered` for
// every producer — so this route is one more producer, not a second mechanism.
// What it adds is the *address*: a GUI holds a run id, not a lane, and the
// chat `/steer ` prefix can only aim at the lane's sole running workflow. The
// lane the event is stamped with therefore comes from the run's own
// `source_lane`, never from the request.
//
// The chat prefix is untouched: it is the CLI's and Telegram's only channel.

/// Apply one steering push, as a `Response`. Split out from the handler so
/// every status code is provable without a router or an `AppState`.
fn steer_task(
    db: &Database,
    shared_context: &SharedContext,
    bus: &EventBus,
    routing: &RoutingConfig,
    owner_id: &str,
    id: &str,
    request: SteerTaskRequest,
) -> Response {
    // The rollback switch governs the whole rail, so it answers before the
    // route looks at the request or the row: with steering off there is no
    // inbox on any workflow to inject into, and a 404/409 would misreport why.
    if !routing.steering_enabled {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "STEERING_DISABLED",
            "Steering is disabled — set [orchestrator.routing] steering_enabled = true to \
             inject messages into running workflows.",
        );
    }

    let message = request.message.trim();
    if message.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "EMPTY_MESSAGE",
            "message must not be empty",
        );
    }

    let task = match TaskRepository::new(db).get(id) {
        Ok(Some(task)) => task,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "NOT_FOUND", "Task not found"),
        Err(e) => {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string());
        }
    };

    // The same predicates the row's `steerable` flag is computed from.
    match steer_refusal(&task, shared_context, owner_id) {
        // A run this caller cannot see is a run that does not exist — the same
        // `404` `routes/files.rs` and `routes/artifacts.rs` already answer
        // with, byte for byte, rather than a `403` that would confirm the id
        // belongs to someone.
        Some(SteerRefusal::NotOwned) => {
            return api_error(StatusCode::NOT_FOUND, "NOT_FOUND", "Task not found");
        }
        Some(SteerRefusal::Terminal | SteerRefusal::NoInbox) => {
            return api_error(
                StatusCode::CONFLICT,
                "TASK_NOT_STEERABLE",
                "This run is not accepting steering messages — it is not running, or it has \
                 already finished.",
            );
        }
        None => {}
    }

    let msg = SteeringMsg {
        text: message.to_string(),
        request_id: Uuid::new_v4(),
        // The identity a leftover message re-enters the front door with, if the
        // workflow exits before draining it (follow-up conversion).
        principal: Principal::User {
            global_id: owner_id.to_string(),
        },
        scope: Scope::Global,
        workspace_path: request.workspace_path.or_else(|| task.workspace_id.clone()),
        received_at: Utc::now(),
    };

    match push_steering(
        shared_context,
        bus,
        &task.id,
        &task.source_lane,
        msg,
        Some(db),
    ) {
        Ok(inbox_depth) => Json(SteerTaskResponse {
            task_id: task.id,
            accepted: true,
            inbox_depth,
            lane_key: task.source_lane,
        })
        .into_response(),
        Err(SteeringPushError::Full) => api_error(
            StatusCode::CONFLICT,
            "STEERING_INBOX_FULL",
            format!(
                "The steering queue for this run is full ({} messages) — it has not caught up \
                 with earlier ones yet.",
                routing.steering_inbox_cap,
            ),
        ),
        // The `NoInbox` pre-check above answers this in the ordinary case;
        // what is left here is the race — a workflow closing its inbox between
        // that check and this push — and it gets the same code, because it is
        // the same fact about the run.
        Err(SteeringPushError::Closed) => api_error(
            StatusCode::CONFLICT,
            "TASK_NOT_STEERABLE",
            "This run is not accepting steering messages — it is not running, or it has \
             already finished.",
        ),
    }
}

/// POST /v1/tasks/{id}/steer
pub async fn steer_task_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<SteerTaskRequest>,
) -> Response {
    steer_task(
        &state.db,
        &state.gateway.shared_context,
        &state.gateway.bus,
        &state.daemon_config.load().orchestrator.routing,
        &state.local_user_id,
        &id,
        request,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use openalpaca_core::bus::EventBus;
    use openalpaca_core::context::SharedContext;
    use openalpaca_core::daemon_config::RoutingConfig;
    use openalpaca_core::runner::steering::{SteeringInbox, SteeringMsg};
    use openalpaca_storage::OutcomeKind;

    fn make_test_task() -> Task {
        Task {
            id: "task-1".to_string(),
            title: "Test task".to_string(),
            description: None,
            status: TaskStatus::Completed,
            priority: 0,
            progress_current: None,
            progress_total: None,
            result_summary: Some("Done".to_string()),
            created_by: "user-1".to_string(),
            source_lane: "lane-1".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            completed_at: Some(Utc::now()),
            state_json: None,
            state_version: 1,
            outcome_json: None,
            outcome_kind: None,
            artifact_count: 0,
            workspace_id: None,
            source_task_id: None,
            session_id: None,
        }
    }

    #[test]
    fn test_parsed_outcome_from_task_text_only() {
        let mut task = make_test_task();
        task.outcome_kind = Some(OutcomeKind::TextOnly);
        task.artifact_count = 0;
        task.outcome_json = Some(
            serde_json::json!({
                "summary": "Generated a text summary",
                "outcome_kind": "text_only",
                "no_artifact_reason": "No files were requested",
                "artifacts": []
            })
            .to_string(),
        );

        let outcome = parse_outcome(&task).expect("should parse");
        assert_eq!(
            outcome.outcome_summary.as_deref(),
            Some("Generated a text summary")
        );
        assert_eq!(outcome.outcome_kind, "text_only");
        assert_eq!(outcome.artifact_count, 0);
        assert!(outcome.artifacts.is_empty());
        assert_eq!(
            outcome.no_artifact_reason.as_deref(),
            Some("No files were requested")
        );
    }

    #[test]
    fn test_parsed_outcome_from_task_mixed() {
        let mut task = make_test_task();
        task.outcome_kind = Some(OutcomeKind::Mixed);
        task.artifact_count = 2;
        task.outcome_json = Some(
            serde_json::json!({
                "summary": "Report with charts",
                "outcome_kind": "mixed",
                "artifacts": [
                    {"key": "report.pdf", "label": "Report", "agent_id": "researcher", "step_order": 0},
                    {"key": "chart.png", "label": "Chart", "agent_id": "researcher", "step_order": 0},
                ]
            })
            .to_string(),
        );

        let outcome = parse_outcome(&task).expect("should parse");
        assert_eq!(outcome.outcome_summary.as_deref(), Some("Report with charts"));
        assert_eq!(outcome.outcome_kind, "mixed");
        assert_eq!(outcome.artifact_count, 2);
        assert_eq!(outcome.artifacts.len(), 2);
        assert!(outcome.no_artifact_reason.is_none());
    }

    #[test]
    fn test_parsed_outcome_from_task_none() {
        let task = make_test_task();
        assert!(parse_outcome(&task).is_none());
    }

    #[test]
    fn test_parsed_outcome_from_task_malformed() {
        let mut task = make_test_task();
        task.outcome_json = Some("not valid json".to_string());
        assert!(parse_outcome(&task).is_none());
    }

    #[test]
    fn test_parsed_outcome_from_task_artifact_only() {
        let mut task = make_test_task();
        task.outcome_kind = Some(OutcomeKind::ArtifactOnly);
        task.artifact_count = 1;
        task.outcome_json = Some(
            serde_json::json!({
                "summary": "Generated report",
                "outcome_kind": "artifact_only",
                "artifacts": [
                    {"key": "report.pdf", "label": "Report", "agent_id": "writer", "step_order": 0},
                ]
            })
            .to_string(),
        );

        let outcome = parse_outcome(&task).expect("should parse");
        assert_eq!(outcome.outcome_summary.as_deref(), Some("Generated report"));
        assert_eq!(outcome.outcome_kind, "artifact_only");
        assert_eq!(outcome.artifact_count, 1);
        assert_eq!(outcome.artifacts.len(), 1);
        assert!(outcome.no_artifact_reason.is_none());
    }

    #[test]
    fn test_parsed_outcome_from_task_failed() {
        let mut task = make_test_task();
        task.status = TaskStatus::Failed;
        task.outcome_kind = Some(OutcomeKind::Failed);
        task.artifact_count = 0;
        task.outcome_json = Some(
            serde_json::json!({
                "summary": "Network timeout after 3 retries",
                "outcome_kind": "failed",
                "artifacts": []
            })
            .to_string(),
        );

        let outcome = parse_outcome(&task).expect("should parse");
        assert_eq!(
            outcome.outcome_summary.as_deref(),
            Some("Network timeout after 3 retries")
        );
        assert_eq!(outcome.outcome_kind, "failed");
        assert_eq!(outcome.artifact_count, 0);
        assert!(outcome.artifacts.is_empty());
    }

    /// P8 — the legacy agent-run payload is gone from both task shapes.
    /// `subagent_span` + `GET /v1/tasks/{id}/timeline` carry the run's lanes
    /// now, including the in-flight ones `agent_task_history` never had a row
    /// for. Nothing else about either shape changes, so the fields the clients
    /// actually read are asserted present alongside.
    #[test]
    fn neither_task_shape_carries_the_legacy_agent_run_payload() {
        let detail = serde_json::to_value(TaskResponse {
            task: make_test_task(),
            outcome: None,
            steerable: false,
        })
        .unwrap();
        for key in ["assignments", "assigned_agents", "agents"] {
            assert!(
                detail.get(key).is_none(),
                "GET /v1/tasks/{{id}} still serves `{key}`"
            );
        }
        assert!(detail.get("task").is_some());

        let summary = serde_json::to_value(TaskSummaryResponse {
            task: make_test_task(),
            outcome: None,
            cost_usd: 0.0,
            subagent_count: 0,
            steerable: false,
        })
        .unwrap();
        for key in ["assignments", "assigned_agents", "agents"] {
            assert!(
                summary.get(key).is_none(),
                "GET /v1/tasks still serves `{key}`"
            );
        }
        assert!(summary.get("id").is_some());
        assert!(summary.get("cost_usd").is_some());
    }

    #[test]
    fn test_task_serialization_suppresses_internal_fields() {
        let mut task = make_test_task();
        task.state_json = Some(r#"{"steps":[]}"#.to_string());
        task.outcome_json = Some(r#"{"summary":"done"}"#.to_string());

        let v = serde_json::to_value(&task).unwrap();
        // state_json and outcome_json should not appear in serialized output
        assert!(
            v.get("state_json").is_none(),
            "state_json should be suppressed from serialized Task"
        );
        assert!(
            v.get("outcome_json").is_none(),
            "outcome_json should be suppressed from serialized Task"
        );
        // But other fields should still be present
        assert!(v.get("id").is_some());
        assert!(v.get("status").is_some());
        assert!(v.get("outcome_kind").is_some());
        assert!(v.get("artifact_count").is_some());
    }

    #[test]
    fn test_task_response_excludes_raw_json_fields() {
        let mut task = make_test_task();
        task.outcome_kind = Some(OutcomeKind::TextOnly);
        task.outcome_json = Some(
            serde_json::json!({
                "summary": "Test",
                "outcome_kind": "text_only",
                "artifacts": []
            })
            .to_string(),
        );

        let outcome = parse_outcome(&task);
        let resp = TaskResponse {
            task,
            outcome,
            steerable: false,
        };

        let v = serde_json::to_value(&resp).unwrap();
        // The task sub-object should not contain raw JSON fields
        assert!(v["task"].get("state_json").is_none());
        assert!(v["task"].get("outcome_json").is_none());
        // But the parsed outcome should be present at top level
        assert!(v.get("outcome").is_some());
        assert_eq!(v["outcome"]["outcome_summary"], "Test");
    }

    /// Reproduces `list_tasks_handler`'s pre-refactor shape: `serde_json::to_value(&task)`
    /// with `outcome` (only when it parses) inserted onto the object — the exact algorithm
    /// `TaskSummaryResponse` replaces — plus `cost_usd` (GAP-08b), which never existed
    /// pre-refactor but is always present on the typed struct today. Used below to pin
    /// that the typed struct serializes to byte-for-byte this shape.
    ///
    /// The `assigned_agents` key the old algorithm also injected is deliberately absent:
    /// P8 deleted it, and the test above pins that.
    fn pre_refactor_shape(task: &Task, cost_usd: f64, subagent_count: i64) -> serde_json::Value {
        let mut v = serde_json::to_value(task).unwrap();
        if let Some(obj) = v.as_object_mut() {
            let outcome_val =
                parse_outcome(task).and_then(|parsed| serde_json::to_value(parsed).ok());
            if let Some(outcome_val) = outcome_val {
                obj.insert("outcome".to_string(), outcome_val);
            }
            obj.insert("cost_usd".to_string(), serde_json::json!(cost_usd));
            obj.insert(
                "subagent_count".to_string(),
                serde_json::json!(subagent_count),
            );
            // R40's read-time hint, present on every row.
            obj.insert("steerable".to_string(), serde_json::json!(false));
        }
        v
    }

    // ── GET /v1/tasks/{id}/timeline (GAP-09) ──────────────────────

    /// A temp database with one task row, so spans have a live FK to hang on.
    fn timeline_db(status: TaskStatus) -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open db");
        let mut task = make_test_task();
        task.status = status;
        task.completed_at = status.is_terminal().then(Utc::now);
        TaskRepository::new(&db).create(&task).expect("create task");
        if status.is_terminal() {
            TaskRepository::new(&db)
                .update_status(&task.id, status)
                .expect("terminal status");
        }
        (dir, db)
    }

    fn open_span(db: &Database, span_id: &str, template: &str, instance: &str) {
        SubagentSpanRepository::new(db)
            .open(openalpaca_storage::NewSubagentSpan {
                id: span_id,
                task_id: "task-1",
                template_id: template,
                agent_instance_id: instance,
                objective: Some("do the thing"),
            })
            .expect("open span");
    }

    fn body(response: (StatusCode, Json<serde_json::Value>)) -> serde_json::Value {
        response.1.0
    }

    /// The envelope: the run's own start, a server `now` for the axis's right
    /// edge, and one lane per span in start order.
    #[test]
    fn the_timeline_carries_the_run_window_and_a_lane_per_span() {
        let (_dir, db) = timeline_db(TaskStatus::Running);
        open_span(&db, "n1", "review_agent", "review_agent::a");
        open_span(&db, "n2", "writing_agent", "writing_agent::b");

        let (status, Json(value)) = task_timeline(&db, None, "task-1");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["task_id"], "task-1");
        assert!(value["started_at"].is_string());
        assert!(value["now"].is_string());
        assert!(value["completed_at"].is_null(), "a live run has no end yet");

        let lanes = value["lanes"].as_array().expect("lanes array");
        assert_eq!(lanes.len(), 2);
        assert_eq!(lanes[0]["lane_id"], "n1");
        assert_eq!(lanes[0]["label"], "review\u{b7}1");
        assert_eq!(lanes[0]["template_id"], "review_agent");
        assert_eq!(lanes[0]["agent_instance_id"], "review_agent::a");
        assert_eq!(lanes[1]["label"], "writing\u{b7}1");
        // Nothing counts steps inside a subagent loop, so the two optional
        // fields are absent rather than a fabricated 0/0.
        assert!(lanes[0].get("steps_current").is_none());
        assert!(lanes[0].get("steps_total").is_none());
    }

    /// The correction that motivated the whole table: a lane that has not
    /// finished is *visible*, with a start and no end. `agent_task_history`
    /// has no row at all until the run returns.
    #[test]
    fn an_in_flight_lane_is_visible_with_no_end() {
        let (_dir, db) = timeline_db(TaskStatus::Running);
        open_span(&db, "n1", "review_agent", "review_agent::a");

        let value = body(task_timeline(&db, None, "task-1"));
        let lane = &value["lanes"][0];
        assert_eq!(lane["state"], "running");
        assert!(lane["started_at"].is_string());
        assert!(lane["ended_at"].is_null());
        assert!(lane["detail"].is_null());
    }

    /// A subagent cancelled before it started reports `cancelled`, not
    /// `failed` — the distinction the write site keeps deliberately.
    #[test]
    fn a_cancelled_lane_reports_cancelled_with_its_reason() {
        let (_dir, db) = timeline_db(TaskStatus::Running);
        open_span(&db, "n1", "review_agent", "review_agent::a");
        SubagentSpanRepository::new(&db)
            .close(
                "n1",
                openalpaca_storage::SpanState::Cancelled,
                Some("cancelled before starting"),
                None,
            )
            .expect("close");

        let value = body(task_timeline(&db, None, "task-1"));
        assert_eq!(value["lanes"][0]["state"], "cancelled");
        assert_eq!(value["lanes"][0]["detail"], "cancelled before starting");
        assert!(value["lanes"][0]["ended_at"].is_string());
    }

    /// A pending confirmation flips exactly the lane whose agent instance is
    /// waiting — not its sibling, and not a lane of another run.
    #[test]
    fn a_pending_confirmation_blocks_exactly_one_lane() {
        let (_dir, db) = timeline_db(TaskStatus::Running);
        open_span(&db, "n1", "review_agent", "review_agent::a");
        open_span(&db, "n2", "review_agent", "review_agent::b");

        let broker = ConfirmationBroker::new();
        let _rx = broker.request(
            &openalpaca_core::security::confirmation::ConfirmationRequest {
                request_id: "req-1".to_string(),
                agent_id: "review_agent".to_string(),
                tool_name: "shell_execute".to_string(),
                tool_arguments: serde_json::json!({"cmd": "ls"}),
                stream_id: None,
                lane_key: None,
                task_id: Some("task-1".to_string()),
                agent_instance_id: Some("review_agent::b".to_string()),
                timestamp: Utc::now(),
            },
        );
        // Another run's prompt must not colour this run's lanes.
        let _rx2 = broker.request(
            &openalpaca_core::security::confirmation::ConfirmationRequest {
                request_id: "req-2".to_string(),
                agent_id: "review_agent".to_string(),
                tool_name: "shell_execute".to_string(),
                tool_arguments: serde_json::json!({}),
                stream_id: None,
                lane_key: None,
                task_id: Some("other-task".to_string()),
                agent_instance_id: Some("review_agent::a".to_string()),
                timestamp: Utc::now(),
            },
        );

        let value = body(task_timeline(&db, Some(&broker), "task-1"));
        let lanes = value["lanes"].as_array().unwrap();
        assert_eq!(lanes[0]["state"], "running", "the sibling keeps running");
        assert_eq!(lanes[1]["state"], "blocked");
        assert_eq!(lanes[1]["detail"], "waiting on shell_execute");

        // Answering it puts the lane back: `blocked` is never stored.
        broker
            .respond(
                "req-1",
                openalpaca_core::security::confirmation::ConfirmationResponse {
                    approved: true,
                    approval_scope: None,
                },
            )
            .unwrap();
        let value = body(task_timeline(&db, Some(&broker), "task-1"));
        assert_eq!(value["lanes"][1]["state"], "running");
    }

    /// A lane still `running` on a task that has already finished belongs to a
    /// dead daemon generation: it reports `cancelled` / `"interrupted"` even
    /// before the next boot's `close_orphans` writes that down.
    #[test]
    fn a_stale_lane_on_a_terminal_run_reports_interrupted() {
        let (_dir, db) = timeline_db(TaskStatus::Completed);
        open_span(&db, "n1", "review_agent", "review_agent::a");

        let value = body(task_timeline(&db, None, "task-1"));
        assert_eq!(value["lanes"][0]["state"], "cancelled");
        assert_eq!(value["lanes"][0]["detail"], "interrupted");
        assert!(value["completed_at"].is_string());

        // The row itself is untouched — the derivation is a read-time rule.
        let stored = SubagentSpanRepository::new(&db)
            .list_for_task("task-1")
            .unwrap();
        assert_eq!(stored[0].state, "running");
    }

    /// …and a confirmation cannot resurrect a lane on a finished run: the
    /// terminal rule wins.
    #[test]
    fn a_terminal_run_is_never_blocked() {
        let (_dir, db) = timeline_db(TaskStatus::Failed);
        open_span(&db, "n1", "review_agent", "review_agent::a");

        let broker = ConfirmationBroker::new();
        let _rx = broker.request(
            &openalpaca_core::security::confirmation::ConfirmationRequest {
                request_id: "req-1".to_string(),
                agent_id: "review_agent".to_string(),
                tool_name: "shell_execute".to_string(),
                tool_arguments: serde_json::json!({}),
                stream_id: None,
                lane_key: None,
                task_id: Some("task-1".to_string()),
                agent_instance_id: Some("review_agent::a".to_string()),
                timestamp: Utc::now(),
            },
        );

        let value = body(task_timeline(&db, Some(&broker), "task-1"));
        assert_eq!(value["lanes"][0]["state"], "cancelled");
        assert_eq!(value["lanes"][0]["detail"], "interrupted");
    }

    /// A run with no subagents is an empty lane list, not an error — and an
    /// unknown run is a 404, not an empty timeline.
    #[test]
    fn a_run_without_spans_is_empty_and_an_unknown_run_is_a_404() {
        let (_dir, db) = timeline_db(TaskStatus::Running);

        let (status, Json(value)) = task_timeline(&db, None, "task-1");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["lanes"], serde_json::json!([]));

        let (status, Json(value)) = task_timeline(&db, None, "no-such-task");
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(value["error"], "Task not found");
    }

    #[test]
    fn test_task_summary_response_matches_pre_refactor_shape_with_outcome() {
        let mut task = make_test_task();
        task.outcome_kind = Some(OutcomeKind::TextOnly);
        task.artifact_count = 0;
        task.outcome_json = Some(
            serde_json::json!({
                "summary": "Generated a text summary",
                "outcome_kind": "text_only",
                "no_artifact_reason": "No files were requested",
                "artifacts": []
            })
            .to_string(),
        );
        let expected = pre_refactor_shape(&task, 1.25, 2);
        let outcome = parse_outcome(&task);
        let summary = TaskSummaryResponse {
            task,
            outcome,
            cost_usd: 1.25,
            subagent_count: 2,
            steerable: false,
        };
        let actual = serde_json::to_value(&summary).unwrap();

        assert_eq!(actual, expected);
        // Sanity: the field the old post-injection added is actually present,
        // so this test would fail if it silently dropped out.
        assert!(actual.get("outcome").is_some());
        assert_eq!(actual["outcome"]["outcome_kind"], "text_only");
        assert_eq!(actual["cost_usd"], 1.25);
    }

    #[test]
    fn test_task_summary_response_matches_pre_refactor_shape_without_outcome() {
        // No outcome_kind/outcome_json set: parse_outcome returns None, and the
        // old code never inserted an "outcome" key in that case.
        let task = make_test_task();

        let expected = pre_refactor_shape(&task, 0.0, 0);
        let outcome = parse_outcome(&task);
        assert!(outcome.is_none());
        let summary = TaskSummaryResponse {
            task,
            outcome,
            cost_usd: 0.0,
            subagent_count: 0,
            steerable: false,
        };
        let actual = serde_json::to_value(&summary).unwrap();

        assert_eq!(actual, expected);
        // A task with no logged LLM calls still gets an explicit 0.0, not an
        // omitted field.
        assert_eq!(actual["cost_usd"], 0.0);
        assert!(actual.get("outcome").is_none());
    }

    /// R38 — the per-run agent signal the list route lost with P8, back as a
    /// count rather than the old `agent_task_history` array: one grouped
    /// `SubagentSpanRepository::counts_for_tasks` query over the page's ids,
    /// exactly the shape `cost_for_tasks` already had. Always serialized, so a
    /// run that spawned nothing is distinguishable from a daemon too old to
    /// know the field.
    #[test]
    fn task_summary_rows_carry_the_number_of_agents_the_run_spawned() {
        let v = serde_json::to_value(TaskSummaryResponse {
            task: make_test_task(),
            outcome: None,
            cost_usd: 0.0,
            subagent_count: 3,
            steerable: false,
        })
        .unwrap();
        assert_eq!(v["subagent_count"], 3);

        // A run with no spans is absent from the grouped map — the handler's
        // `unwrap_or(0)` is what turns that into a number, and the key is
        // still present on the wire.
        let counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        let v = serde_json::to_value(TaskSummaryResponse {
            task: make_test_task(),
            outcome: None,
            cost_usd: 0.0,
            subagent_count: counts.get("task-1").copied().unwrap_or(0),
            steerable: false,
        })
        .unwrap();
        assert!(
            v.get("subagent_count").is_some(),
            "a run that spawned nothing still carries the key"
        );
        assert_eq!(v["subagent_count"], 0);

        // A list-row field, like `cost_usd`: the detail route's shape is
        // untouched, and API_MAP §5's warning that the two disagree stands.
        let detail = serde_json::to_value(TaskResponse {
            task: make_test_task(),
            outcome: None,
            steerable: false,
        })
        .unwrap();
        assert!(detail["task"].get("subagent_count").is_none());
    }

    /// §4.7 item 3 — the row shape both task routes serve carries the project
    /// the run belonged to, so `rerun` (Phase 5) and the Library can filter by
    /// it. Always present, `null` for a run that had no project: an omitted
    /// key would be indistinguishable from an older daemon.
    #[test]
    fn task_rows_carry_the_runs_workspace_id() {
        let mut task = make_test_task();
        task.workspace_id = Some("/Users/dev/openalpaca".to_string());

        // GET /v1/tasks — the flattened summary row.
        let summary = TaskSummaryResponse {
            task: task.clone(),
            outcome: None,
            cost_usd: 0.0,
            subagent_count: 0,
            steerable: false,
        };
        let v = serde_json::to_value(&summary).unwrap();
        assert_eq!(v["workspace_id"], "/Users/dev/openalpaca");

        // GET /v1/tasks/{id} — the nested `task` object.
        let single = TaskResponse {
            task,
            outcome: None,
            steerable: false,
        };
        let v = serde_json::to_value(&single).unwrap();
        assert_eq!(v["task"]["workspace_id"], "/Users/dev/openalpaca");

        // A run with no project reports an explicit null, not a missing key.
        let v = serde_json::to_value(make_test_task()).unwrap();
        assert!(v.get("workspace_id").is_some());
        assert!(v["workspace_id"].is_null());
    }

    // ── POST /v1/tasks/{id}/steer (GAP-02) ────────────────────────

    const STEER_OWNER: &str = "user-1";

    /// A temp database holding one `Running` task owned by [`STEER_OWNER`],
    /// on lane `lane-1`, optionally carrying a project.
    fn steer_db(workspace_id: Option<&str>) -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open db");
        let mut task = make_test_task();
        task.status = TaskStatus::Running;
        task.completed_at = None;
        task.workspace_id = workspace_id.map(str::to_string);
        TaskRepository::new(&db).create(&task).expect("create task");
        (dir, db)
    }

    /// A live workflow: a registered inbox for `task-1`, at the given cap.
    fn steerable(cap: usize) -> (Arc<SharedContext>, EventBus, Arc<SteeringInbox>) {
        let shared = Arc::new(SharedContext::new());
        let bus = EventBus::default();
        let inbox = Arc::new(SteeringInbox::new(cap));
        shared.register_steering_inbox("task-1", inbox.clone());
        (shared, bus, inbox)
    }

    fn steer_request(message: &str) -> SteerTaskRequest {
        SteerTaskRequest {
            message: message.to_string(),
            workspace_path: None,
        }
    }

    /// Split a `Response` into its status and its JSON body.
    async fn split(response: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("read the response body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    /// The happy path: the message lands in the run's own inbox, the lane comes
    /// from `task.source_lane` (the caller addresses a *run*, not a lane), and
    /// the acknowledgement is the queue depth — not a promise it was read.
    #[tokio::test]
    async fn steering_a_running_task_queues_the_message_and_answers_with_the_depth() {
        let (_dir, db) = steer_db(None);
        let (shared, bus, inbox) = steerable(16);
        let mut rx = bus.subscribe();

        let (status, body) = split(steer_task(
            &db,
            &shared,
            &bus,
            &RoutingConfig::default(),
            STEER_OWNER,
            "task-1",
            steer_request("  focus on the tests  "),
        ))
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["task_id"], "task-1");
        assert_eq!(body["accepted"], true);
        assert_eq!(body["inbox_depth"], 1);
        assert_eq!(body["lane_key"], "lane-1");

        // The text is trimmed, and the identity is the owner's, so a leftover
        // message re-enters the front door as this user.
        let drained = inbox.drain_all();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].text, "focus on the tests");
        assert_eq!(
            drained[0].principal,
            openalpaca_core::security::policy::Principal::User {
                global_id: STEER_OWNER.to_string(),
            }
        );

        // The rail's own event, from the shared push helper — no new family.
        let mut steered = 0;
        while let Ok(event) = rx.try_recv() {
            if let SystemEvent::WorkflowSteered {
                task_id, lane_key, ..
            } = event
            {
                assert_eq!(task_id, "task-1");
                assert_eq!(lane_key, "lane-1");
                steered += 1;
            }
        }
        assert_eq!(steered, 1, "exactly one WorkflowSteered");
    }

    /// The optional `workspace_path` defaults to the run's own project, so an
    /// `unprocessed_steering` leftover re-enters scoped where the run was.
    #[tokio::test]
    async fn the_message_inherits_the_runs_project_unless_the_caller_names_one() {
        let (_dir, db) = steer_db(Some("/Users/dev/openalpaca"));
        let (shared, bus, inbox) = steerable(16);

        let _ = steer_task(
            &db,
            &shared,
            &bus,
            &RoutingConfig::default(),
            STEER_OWNER,
            "task-1",
            steer_request("inherit"),
        );
        assert_eq!(
            inbox.drain_all()[0].workspace_path.as_deref(),
            Some("/Users/dev/openalpaca"),
        );

        let _ = steer_task(
            &db,
            &shared,
            &bus,
            &RoutingConfig::default(),
            STEER_OWNER,
            "task-1",
            SteerTaskRequest {
                message: "override".to_string(),
                workspace_path: Some("/tmp/other".to_string()),
            },
        );
        assert_eq!(
            inbox.drain_all()[0].workspace_path.as_deref(),
            Some("/tmp/other"),
        );
    }

    /// A backlogged inbox is a `409`, not a dropped message: the caller is told
    /// the queue is full so it can offer the follow-up instead.
    #[tokio::test]
    async fn a_full_inbox_is_a_409_steering_inbox_full() {
        let (_dir, db) = steer_db(None);
        let (shared, bus, inbox) = steerable(1);
        inbox
            .push(SteeringMsg {
                text: "earlier".to_string(),
                request_id: Uuid::new_v4(),
                principal: openalpaca_core::security::policy::Principal::System,
                scope: openalpaca_core::security::policy::Scope::Global,
                workspace_path: None,
                received_at: Utc::now(),
            })
            .expect("seed the queue");

        let (status, body) = split(steer_task(
            &db,
            &shared,
            &bus,
            &RoutingConfig::default(),
            STEER_OWNER,
            "task-1",
            steer_request("one more"),
        ))
        .await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "STEERING_INBOX_FULL");
        // Nothing was queued past the cap.
        assert_eq!(inbox.drain_all().len(), 1);
    }

    /// Two ways a run stops being steerable — it never registered an inbox, or
    /// it finished and closed the one it had — and both are the same `409`.
    #[tokio::test]
    async fn a_run_with_no_live_inbox_is_a_409_task_not_steerable() {
        let (_dir, db) = steer_db(None);

        // Never registered: the row exists, the workflow does not.
        let shared = Arc::new(SharedContext::new());
        let bus = EventBus::default();
        let (status, body) = split(steer_task(
            &db,
            &shared,
            &bus,
            &RoutingConfig::default(),
            STEER_OWNER,
            "task-1",
            steer_request("too early"),
        ))
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "TASK_NOT_STEERABLE");

        // Detached: the workflow finished and closed its inbox.
        let (shared, bus, inbox) = steerable(16);
        inbox.close_and_drain();
        let (status, body) = split(steer_task(
            &db,
            &shared,
            &bus,
            &RoutingConfig::default(),
            STEER_OWNER,
            "task-1",
            steer_request("too late"),
        ))
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "TASK_NOT_STEERABLE");
    }

    /// The rollback switch answers for the whole route, before it looks at
    /// anything else: with the rail off there is nothing to inject into.
    #[tokio::test]
    async fn steering_disabled_is_a_503_for_every_request() {
        let (_dir, db) = steer_db(None);
        let (shared, bus, inbox) = steerable(16);
        let routing = RoutingConfig {
            steering_enabled: false,
            ..RoutingConfig::default()
        };

        let (status, body) = split(steer_task(
            &db, &shared, &bus, &routing, STEER_OWNER, "task-1",
            steer_request("go"),
        ))
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], "STEERING_DISABLED");
        assert!(inbox.is_empty(), "nothing is queued while the rail is off");

        // Not a per-task answer: an unknown run gets the same 503.
        let (status, _) = split(steer_task(
            &db, &shared, &bus, &routing, STEER_OWNER, "no-such-task",
            steer_request("go"),
        ))
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    /// An unknown run and another owner's run are the same `404` — a run this
    /// caller cannot see is a run that does not exist.
    #[tokio::test]
    async fn an_unknown_or_foreign_run_is_a_404() {
        let (_dir, db) = steer_db(None);
        let (shared, bus, inbox) = steerable(16);

        let (status, body) = split(steer_task(
            &db,
            &shared,
            &bus,
            &RoutingConfig::default(),
            STEER_OWNER,
            "no-such-task",
            steer_request("go"),
        ))
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "NOT_FOUND");

        let (status, body) = split(steer_task(
            &db,
            &shared,
            &bus,
            &RoutingConfig::default(),
            "someone-else",
            "task-1",
            steer_request("go"),
        ))
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "NOT_FOUND");
        assert_eq!(
            body["error"]["message"], "Task not found",
            "a foreign run must not read differently from a missing one"
        );
        assert!(inbox.is_empty());
    }

    /// An empty (or whitespace-only) message is a `400`: injecting it would
    /// spend a round on nothing.
    #[tokio::test]
    async fn an_empty_message_is_a_400() {
        let (_dir, db) = steer_db(None);
        let (shared, bus, inbox) = steerable(16);

        for message in ["", "   \n\t "] {
            let (status, body) = split(steer_task(
                &db,
                &shared,
                &bus,
                &RoutingConfig::default(),
                STEER_OWNER,
                "task-1",
                steer_request(message),
            ))
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"]["code"], "EMPTY_MESSAGE");
        }
        assert!(inbox.is_empty());
    }

    // ── `steerable` on a task row (R40) ───────────────────────────

    /// A run this owner started, still running, with a live inbox.
    fn live_run() -> Task {
        let mut task = make_test_task();
        task.status = TaskStatus::Running;
        task.completed_at = None;
        task
    }

    /// The hint every task row carries, so a client can disable `Steer`
    /// instead of discovering the answer by sending. All three predicates:
    /// the run is this owner's, it is not terminal, and it has a live inbox.
    #[test]
    fn a_task_row_says_whether_the_run_can_be_steered() {
        let (shared, _bus, inbox) = steerable(16);

        // All three hold → steerable.
        assert!(is_steerable(&live_run(), &shared, STEER_OWNER));

        // Another channel's run: `created_by` is `"<provider>:<id>"`, never
        // this daemon's local user.
        let mut foreign = live_run();
        foreign.created_by = "telegram:4242".to_string();
        assert!(!is_steerable(&foreign, &shared, STEER_OWNER));

        // Terminal in the DB, even while the inbox has not been closed yet.
        let mut finished = live_run();
        finished.status = TaskStatus::Completed;
        assert!(!is_steerable(&finished, &shared, STEER_OWNER));

        // No inbox at all (queued, or a daemon generation that is gone).
        let bare = Arc::new(SharedContext::new());
        assert!(!is_steerable(&live_run(), &bare, STEER_OWNER));

        // The inbox it had, closed on the way out.
        inbox.close_and_drain();
        assert!(!is_steerable(&live_run(), &shared, STEER_OWNER));
    }

    /// The hint is served by both task shapes — flattened on a list row,
    /// beside `task` on the detail — and is always present, so `false` is
    /// distinguishable from an older daemon that does not know the field.
    #[test]
    fn both_task_shapes_serve_the_steerable_hint() {
        let summary = serde_json::to_value(TaskSummaryResponse {
            task: live_run(),
            outcome: None,
            cost_usd: 0.0,
            subagent_count: 0,
            steerable: true,
        })
        .unwrap();
        assert_eq!(summary["steerable"], true);

        let detail = serde_json::to_value(TaskResponse {
            task: live_run(),
            outcome: None,
            steerable: false,
        })
        .unwrap();
        assert_eq!(detail["steerable"], false);
    }

    /// The anti-drift assertion: a row that says `steerable: false` is a row
    /// the steer route refuses, and one that says `true` is a row it accepts —
    /// both sides read the same predicates.
    #[tokio::test]
    async fn the_steerable_hint_matches_what_the_steer_route_does() {
        // Owned, running, live inbox → the hint says yes and the route agrees.
        let (_dir, db) = steer_db(None);
        let (shared, bus, inbox) = steerable(16);
        let task = TaskRepository::new(&db)
            .get("task-1")
            .unwrap()
            .expect("the run exists");
        assert!(is_steerable(&task, &shared, STEER_OWNER));
        let (status, _) = split(steer_task(
            &db,
            &shared,
            &bus,
            &RoutingConfig::default(),
            STEER_OWNER,
            "task-1",
            steer_request("go"),
        ))
        .await;
        assert_eq!(status, StatusCode::OK);

        // Another owner: the hint says no, and the route answers 404 — the
        // same answer it gives for a run that does not exist.
        assert!(!is_steerable(&task, &shared, "someone-else"));
        let (status, _) = split(steer_task(
            &db,
            &shared,
            &bus,
            &RoutingConfig::default(),
            "someone-else",
            "task-1",
            steer_request("go"),
        ))
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Terminal row, inbox still open (the window between `cancel` marking
        // the row and the workflow closing its inbox): the hint says no, and
        // the route refuses rather than queueing into a run that is over.
        let mut cancelled = task.clone();
        cancelled.status = TaskStatus::Cancelled;
        cancelled.completed_at = Some(Utc::now());
        TaskRepository::new(&db)
            .update_status(&cancelled.id, TaskStatus::Cancelled)
            .expect("mark the run cancelled");
        assert!(!is_steerable(&cancelled, &shared, STEER_OWNER));
        let (status, body) = split(steer_task(
            &db,
            &shared,
            &bus,
            &RoutingConfig::default(),
            STEER_OWNER,
            "task-1",
            steer_request("too late"),
        ))
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "TASK_NOT_STEERABLE");
        // Nothing was queued past the refusal — only the accepted push above.
        assert_eq!(inbox.drain_all().len(), 1);
    }

    // ── Re-run and start (GAP-06) ─────────────────────────────────

    use openalpaca_core::agent::template::parse_agent_markdown;
    use openalpaca_core::lane::LaneManager;
    use openalpaca_core::middleware::prompt::SystemPersona;
    use openalpaca_core::orchestrator::{Orchestrator, skill_catalog, skill_router};
    use openalpaca_core::runner::LoopConfig;
    use openalpaca_core::security::gate::SecurityGate;
    use openalpaca_core::security::sandbox::SandboxManager;
    use openalpaca_core::tools::ToolRegistry;
    use openalpaca_llm::{
        ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, LlmRouter, ProviderType,
        Usage,
    };

    const LAUNCH_OWNER: &str = "user-1";

    /// A provider that answers anything in one line. Present so the dispatch
    /// takes the same path production does; the run itself never gets far.
    struct StubLlm;

    #[async_trait::async_trait]
    impl LlmProvider for StubLlm {
        fn name(&self) -> &str {
            "stub"
        }
        fn supports_tools(&self) -> bool {
            false
        }
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
            Ok(ChatResponse {
                content: "done".to_string(),
                tool_calls: vec![],
                model: "stub-model".to_string(),
                usage: Usage::default(),
                finish_reason: FinishReason::Stop,
                thinking: None,
                parts: None,
            })
        }
    }

    /// An orchestrator over `db`, with a lead-agent template unless
    /// `with_lead_agent` is false (which is how the `503` is reached).
    fn launch_orchestrator(db: &Database, with_lead_agent: bool) -> Orchestrator {
        let ctx = Arc::new(SharedContext::new());
        if with_lead_agent {
            let template = parse_agent_markdown(
                "---\nid: \"lead_agent\"\nname: \"Lead Agent\"\n\
                 description: \"leads runs\"\nsingleton: true\n\
                 capabilities:\n  - \"orchestration\"\n---\n\nLead the run.\n",
            )
            .expect("the fixture template parses");
            ctx.agent_registry.register_template(template);
        }
        let bus = EventBus::default();
        let registry = Arc::new(ToolRegistry::default());
        let gate = Arc::new(SecurityGate::new(Arc::new(SandboxManager::with_defaults(
            registry.clone(),
            bus.clone(),
        ))));
        Orchestrator::new(
            ctx,
            Arc::new(LaneManager::new()),
            bus,
            SystemPersona::default(),
            Some(Arc::new(LlmRouter::single_provider(
                Arc::new(StubLlm),
                ProviderType::Anthropic,
                "stub-model".to_string(),
            ))),
            LoopConfig::default(),
            gate,
            registry,
            Some(db.clone()),
            None,
            Arc::new(skill_catalog::SkillCatalog::new()),
            Arc::new(skill_router::SkillRouter::new(0.65, 0.45)),
            Arc::new(arc_swap::ArcSwap::from_pointee(
                openalpaca_core::daemon_config::DaemonConfig::default(),
            )),
        )
    }

    /// A temp database holding one run owned by [`LAUNCH_OWNER`].
    fn launch_db(status: TaskStatus, description: Option<&str>) -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open db");
        let mut task = make_test_task();
        task.status = status;
        task.description = description.map(str::to_string);
        task.source_lane = "user-1:gui".to_string();
        task.completed_at = status.is_terminal().then(Utc::now);
        TaskRepository::new(&db).create(&task).expect("create task");
        (dir, db)
    }

    /// The verb's whole point: a re-run is a **new** run, and the response says
    /// which one it came from — the id the caller sent.
    #[tokio::test]
    async fn re_running_a_finished_run_is_a_201_carrying_a_new_id() {
        let (_dir, db) = launch_db(TaskStatus::Completed, Some("write the changelog"));
        let orchestrator = launch_orchestrator(&db, true);

        let response = rerun_task(&orchestrator, &db, LAUNCH_OWNER, "task-1");
        let (status, body) = split(response).await;

        assert_eq!(status, StatusCode::CREATED);
        assert_ne!(
            body["task_id"], "task-1",
            "201 means a run that is not this one"
        );
        assert_eq!(body["source_task_id"], "task-1");
        assert_eq!(body["title"], "Test task");
        assert!(body["status"].is_string());

        // The link is stored, not just answered: it outlives the response.
        let copy = TaskRepository::new(&db)
            .get(body["task_id"].as_str().unwrap())
            .unwrap()
            .expect("the new row");
        assert_eq!(copy.source_task_id.as_deref(), Some("task-1"));
    }

    /// A run still in flight already has an agent on this goal.
    #[tokio::test]
    async fn re_running_a_live_run_is_a_409_task_not_terminal() {
        for status in [TaskStatus::Queued, TaskStatus::Running, TaskStatus::Paused] {
            let (_dir, db) = launch_db(status, Some("write the changelog"));
            let orchestrator = launch_orchestrator(&db, true);

            let (code, body) = split(rerun_task(&orchestrator, &db, LAUNCH_OWNER, "task-1")).await;
            assert_eq!(code, StatusCode::CONFLICT, "status {status:?}");
            assert_eq!(body["error"]["code"], "TASK_NOT_TERMINAL");
            assert!(
                body["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains(status.as_str()),
                "the refusal names the state it refused",
            );
            // Nothing was dispatched.
            assert_eq!(TaskRepository::new(&db).list_recent(10).unwrap().len(), 1);
        }
    }

    /// `422`, not `400`: the request is well-formed and names a real run — the
    /// stored row is what has nothing to give a lead agent.
    #[tokio::test]
    async fn re_running_a_row_with_no_goal_is_a_422_task_not_rerunnable() {
        for description in [None, Some("   \n ")] {
            let (_dir, db) = launch_db(TaskStatus::Completed, description);
            let orchestrator = launch_orchestrator(&db, true);

            let (code, body) = split(rerun_task(&orchestrator, &db, LAUNCH_OWNER, "task-1")).await;
            assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(body["error"]["code"], "TASK_NOT_RERUNNABLE");
        }
    }

    /// Owner-scoped, on the line `steer` drew (R40): a run this caller cannot
    /// see reads exactly like one that does not exist.
    #[tokio::test]
    async fn re_running_an_unknown_or_foreign_run_is_a_404() {
        let (_dir, db) = launch_db(TaskStatus::Completed, Some("write the changelog"));
        let orchestrator = launch_orchestrator(&db, true);

        let (code, body) = split(rerun_task(&orchestrator, &db, LAUNCH_OWNER, "no-such-run")).await;
        assert_eq!(code, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "NOT_FOUND");

        let (code, foreign) = split(rerun_task(&orchestrator, &db, "someone-else", "task-1")).await;
        assert_eq!(code, StatusCode::NOT_FOUND);
        assert_eq!(
            foreign["error"], body["error"],
            "a foreign run must not read differently from a missing one",
        );
        assert_eq!(TaskRepository::new(&db).list_recent(10).unwrap().len(), 1);
    }

    /// Every template that could lead a run is busy — capacity, not a bug, so
    /// `503` and a message worth retrying on.
    #[tokio::test]
    async fn a_launch_with_no_lead_agent_available_is_a_503() {
        let (_dir, db) = launch_db(TaskStatus::Completed, Some("write the changelog"));
        let orchestrator = launch_orchestrator(&db, false);

        let (code, body) = split(rerun_task(&orchestrator, &db, LAUNCH_OWNER, "task-1")).await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], "DISPATCH_FAILED");
    }

    /// D5 — `start` answers with the id the caller sent, in the same
    /// `{task_id, status}` shape the other three actions use.
    #[tokio::test]
    async fn starting_a_queued_run_is_a_200_with_the_same_id() {
        let (_dir, db) = launch_db(TaskStatus::Queued, Some("write the changelog"));
        let orchestrator = launch_orchestrator(&db, true);

        let (code, body) = split(start_task(&orchestrator, &db, LAUNCH_OWNER, "task-1")).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body["task_id"], "task-1", "D5: the id does not change");
        assert!(matches!(
            body["status"].as_str(),
            Some("queued") | Some("running")
        ));
        // One row before, one row after — and it is the same row.
        assert_eq!(TaskRepository::new(&db).list_recent(10).unwrap().len(), 1);
    }

    /// The second `start` on one id loses. Both calls are made before anything
    /// is awaited, so this is the real race the claim exists for, not a
    /// sequence the runtime happened to order.
    #[tokio::test]
    async fn starting_the_same_run_twice_is_a_409_the_second_time() {
        let (_dir, db) = launch_db(TaskStatus::Queued, Some("write the changelog"));
        let orchestrator = launch_orchestrator(&db, true);

        let first = start_task(&orchestrator, &db, LAUNCH_OWNER, "task-1");
        let second = start_task(&orchestrator, &db, LAUNCH_OWNER, "task-1");
        assert_eq!(first.status(), StatusCode::OK);

        let (code, body) = split(second).await;
        assert_eq!(code, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "TASK_ALREADY_RUNNING");
        assert_eq!(TaskRepository::new(&db).list_recent(10).unwrap().len(), 1);
    }

    /// A title-only row — `POST /v1/tasks` allows one — has no goal to
    /// dispatch. Same `422`, its own code: nothing was ever run to re-run.
    #[tokio::test]
    async fn starting_a_row_with_no_goal_is_a_422_task_not_dispatchable() {
        let (_dir, db) = launch_db(TaskStatus::Queued, None);
        let orchestrator = launch_orchestrator(&db, true);

        let (code, body) = split(start_task(&orchestrator, &db, LAUNCH_OWNER, "task-1")).await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"]["code"], "TASK_NOT_DISPATCHABLE");
    }

    /// R43 — `start` re-launches a row **in place**, so a finished run is not
    /// startable: doing it would clear the summary, outcome and artifact count
    /// that run produced. `409`, and its own code word, because the caller can
    /// still `rerun` — which keeps this row and answers with a new id.
    #[tokio::test]
    async fn starting_a_finished_run_is_a_409_task_not_startable() {
        for status in [
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Cancelled,
        ] {
            let (_dir, db) = launch_db(status, Some("write the changelog"));
            TaskRepository::new(&db)
                .set_result("task-1", "the first run's answer")
                .expect("store a result worth protecting");
            let before = TaskRepository::new(&db).get("task-1").unwrap().unwrap();
            let orchestrator = launch_orchestrator(&db, true);

            let (code, body) = split(start_task(&orchestrator, &db, LAUNCH_OWNER, "task-1")).await;
            assert_eq!(code, StatusCode::CONFLICT, "status {status:?}");
            assert_eq!(body["error"]["code"], "TASK_NOT_STARTABLE");
            assert!(
                body["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("re-run"),
                "the refusal has to name the verb that does work",
            );

            // The finished run's row is exactly as it was.
            let after = TaskRepository::new(&db).get("task-1").unwrap().unwrap();
            assert_eq!(after.status, before.status);
            assert_eq!(after.result_summary, before.result_summary);
            assert_eq!(after.completed_at, before.completed_at);
            assert_eq!(after.state_version, before.state_version);
            assert_eq!(TaskRepository::new(&db).list_recent(10).unwrap().len(), 1);
        }
    }

    /// `pause` does not cancel the run's token, so a paused row is still live
    /// and `start` answers the running refusal rather than R43's. Asserted
    /// because it holds by construction, and construction changes.
    #[tokio::test]
    async fn starting_a_paused_run_is_a_409_task_already_running() {
        let (_dir, db) = launch_db(TaskStatus::Paused, Some("write the changelog"));
        let orchestrator = launch_orchestrator(&db, true);
        orchestrator.shared_context.register_cancellation_token(
            "task-1",
            tokio_util::sync::CancellationToken::new(),
        );

        let (code, body) = split(start_task(&orchestrator, &db, LAUNCH_OWNER, "task-1")).await;
        assert_eq!(code, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "TASK_ALREADY_RUNNING");
    }

    #[tokio::test]
    async fn starting_an_unknown_or_foreign_run_is_a_404() {
        let (_dir, db) = launch_db(TaskStatus::Queued, Some("write the changelog"));
        let orchestrator = launch_orchestrator(&db, true);

        let (code, body) = split(start_task(&orchestrator, &db, LAUNCH_OWNER, "no-such-run")).await;
        assert_eq!(code, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "NOT_FOUND");

        let (code, _) = split(start_task(&orchestrator, &db, "someone-else", "task-1")).await;
        assert_eq!(code, StatusCode::NOT_FOUND);
    }

    /// The refusal a client sees for a typo has to list the verb the route
    /// actually grew, or `start` looks unimplemented.
    #[test]
    fn the_unknown_action_refusal_lists_start() {
        let (status, Json(body)) = unknown_action("explode");
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let message = body["error"].as_str().expect("a message");
        assert!(message.contains("explode"));
        for word in ["cancel", "pause", "resume", START_ACTION] {
            assert!(message.contains(word), "the valid set must list {word}");
        }
    }
}
