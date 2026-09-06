//! Task management endpoints
//!
//! POST /v1/tasks           -> create a new task
//! GET  /v1/tasks           -> list tasks (query: created_by, status, limit)
//! GET  /v1/tasks/{id}      -> get a single task + agent runs
//! GET  /v1/tasks/{id}/timeline -> the run's swimlanes (GAP-09)
//! POST /v1/tasks/{id}/action -> perform action (cancel, pause, resume)
//!
//! Neither task shape carries agent runs any more: the legacy
//! `assigned_agents` / `assignments` payload (read from `agent_task_history`)
//! was deleted with Phase 4's P8, and the timeline route serves the same runs
//! from `subagent_span` — in flight as well as finished.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use std::sync::Arc;
use uuid::Uuid;

use openalpaca_core::events::SystemEvent;
use openalpaca_core::orchestrator::{TaskActionError, apply_task_action, parse_outcome};
use openalpaca_core::security::confirmation::ConfirmationBroker;
use openalpaca_storage::{
    Database, LlmUsageRepository, SPAN_DETAIL_INTERRUPTED, SubagentSpanRepository, Task,
    TaskRepository, TaskStatus,
};

use super::tasks_types::*;
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
                    TaskSummaryResponse {
                        task: t,
                        outcome,
                        cost_usd,
                        subagent_count,
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
            (
                StatusCode::OK,
                Json(
                    serde_json::to_value(TaskResponse { task, outcome })
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
) -> impl IntoResponse {
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
        Err(TaskActionError::UnknownAction) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("Unknown action: '{}'. Valid: cancel, pause, resume", request.action)
            })),
        ),
        Err(TaskActionError::Db(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let resp = TaskResponse { task, outcome };

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
        };
        let v = serde_json::to_value(&summary).unwrap();
        assert_eq!(v["workspace_id"], "/Users/dev/openalpaca");

        // GET /v1/tasks/{id} — the nested `task` object.
        let single = TaskResponse {
            task,
            outcome: None,
        };
        let v = serde_json::to_value(&single).unwrap();
        assert_eq!(v["task"]["workspace_id"], "/Users/dev/openalpaca");

        // A run with no project reports an explicit null, not a missing key.
        let v = serde_json::to_value(make_test_task()).unwrap();
        assert!(v.get("workspace_id").is_some());
        assert!(v["workspace_id"].is_null());
    }
}
