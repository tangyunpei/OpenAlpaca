//! Task management endpoints
//!
//! POST /v1/tasks           -> create a new task
//! GET  /v1/tasks           -> list tasks (query: created_by, status, limit)
//! GET  /v1/tasks/{id}      -> get a single task + assignments
//! POST /v1/tasks/{id}/action -> perform action (cancel, pause, resume)

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
use openalpaca_storage::{Task, TaskRepository, TaskStatus};

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
            let repo = TaskRepository::new(&state.db);
            let enriched: Vec<serde_json::Value> = tasks
                .iter()
                .map(|t| {
                    let assignments = repo.get_assignments(&t.id).unwrap_or_default();
                    let agents: Vec<serde_json::Value> = assignments
                        .iter()
                        .map(|a| {
                            serde_json::json!({
                                "agent_id": a.agent_id,
                                "role": a.role,
                                "status": a.status.as_str()
                            })
                        })
                        .collect();
                    match serde_json::to_value(t) {
                        Ok(mut v) => {
                            if let Some(obj) = v.as_object_mut() {
                                obj.insert(
                                    "assigned_agents".to_string(),
                                    serde_json::json!(agents),
                                );
                                if let Some(parsed) = parse_outcome(t) {
                                    if let Ok(outcome_val) = serde_json::to_value(parsed) {
                                        obj.insert("outcome".to_string(), outcome_val);
                                    }
                                }
                            }
                            v
                        }
                        Err(_) => {
                            serde_json::json!({ "id": t.id, "error": "serialization_failed" })
                        }
                    }
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::to_value(enriched).unwrap_or_else(|_| serde_json::json!([]))),
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
            let assignments = repo.get_assignments(&id).unwrap_or_default();
            let outcome = parse_outcome(&task);
            (
                StatusCode::OK,
                Json(
                    serde_json::to_value(TaskResponse {
                        task,
                        assignments: Some(assignments),
                        outcome,
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

/// GET /v1/tasks/{id}/dag — returns structured DAG state with per-node status
pub async fn get_task_dag_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let repo = TaskRepository::new(&state.db);

    let task = match repo.get(&id) {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "Task not found" })),
            );
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            );
        }
    };

    let state_json = match &task.state_json {
        Some(sj) => sj,
        None => {
            return (
                StatusCode::OK,
                Json(serde_json::json!({ "task_id": id, "dag": null })),
            );
        }
    };

    let task_state: openalpaca_core::orchestrator::task_state::TaskState =
        match serde_json::from_str(state_json) {
            Ok(s) => s,
            Err(_) => {
                return (
                    StatusCode::OK,
                    Json(serde_json::json!({ "task_id": id, "dag": null })),
                );
            }
        };

    match task_state.dag {
        Some(dag) => {
            let nodes: Vec<serde_json::Value> = dag
                .nodes
                .iter()
                .map(|n| {
                    serde_json::json!({
                        "node_id": n.node_id,
                        "title": n.title,
                        "description": n.description,
                        "agent_id": n.agent_id,
                        "agent_name": n.agent_name,
                        "depends_on": n.depends_on,
                        "status": n.status,
                        "result_summary": n.result_summary,
                        "workspace_keys": n.workspace_keys,
                        "output_key": n.output_key,
                    })
                })
                .collect();

            let completed = dag.completed_count();
            let running = dag
                .nodes
                .iter()
                .filter(|n| {
                    n.status == openalpaca_core::orchestrator::task_planner::DagNodeStatus::Running
                })
                .count();
            let failed = dag
                .nodes
                .iter()
                .filter(|n| {
                    n.status == openalpaca_core::orchestrator::task_planner::DagNodeStatus::Failed
                })
                .count();

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "task_id": id,
                    "dag": {
                        "total_nodes": dag.nodes.len(),
                        "completed_nodes": completed,
                        "running_nodes": running,
                        "failed_nodes": failed,
                        "is_finished": dag.is_finished(),
                        "nodes": nodes,
                    }
                })),
            )
        }
        None => (
            StatusCode::OK,
            Json(serde_json::json!({ "task_id": id, "dag": null })),
        ),
    }
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
            assignments: None,
            outcome,
        };

        let v = serde_json::to_value(&resp).unwrap();
        // The task sub-object should not contain raw JSON fields
        assert!(v["task"].get("state_json").is_none());
        assert!(v["task"].get("outcome_json").is_none());
        // But the parsed outcome should be present at top level
        assert!(v.get("outcome").is_some());
        assert_eq!(v["outcome"]["outcome_summary"], "Test");
    }
}
