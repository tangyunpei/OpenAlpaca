//! Task management endpoints
//!
//! POST /v1/tasks           -> create a new task
//! GET  /v1/tasks           -> list tasks (query: created_by, status, limit)
//! GET  /v1/tasks/{id}      -> get a single task + agent runs
//! POST /v1/tasks/{id}/action -> perform action (cancel, pause, resume)
//!
//! The `assigned_agents` / `assignments` arrays are sourced from
//! `agent_task_history` (written by the dispatcher's `record_agent_history`),
//! not the dead-post-V2 `task_agent_assignment` table.

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
use openalpaca_storage::{Database, SubAgentRepository, Task, TaskRepository, TaskStatus};

use super::tasks_types::*;
use crate::AppState;

// ── Helpers ───────────────────────────────────────────────────────

/// Summarize the agent runs recorded for a task (from `agent_task_history`)
/// as the `assigned_agents` JSON array served by `GET /v1/tasks`.
fn agent_runs_summary(db: &Database, task_id: &str) -> Vec<serde_json::Value> {
    SubAgentRepository::new(db)
        .get_history_for_task(task_id)
        .unwrap_or_default()
        .iter()
        .map(|run| {
            serde_json::json!({
                "agent_id": run.agent_id,
                "role": run.role,
                "status": run.status,
                "runtime_seconds": run.runtime_seconds,
                "completed_at": run.completed_at,
            })
        })
        .collect()
}

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
            let summaries: Vec<TaskSummaryResponse> = tasks
                .into_iter()
                .map(|t| {
                    let assigned_agents = agent_runs_summary(&state.db, &t.id);
                    let outcome = parse_outcome(&t);
                    TaskSummaryResponse {
                        task: t,
                        assigned_agents,
                        outcome,
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
            let agents = SubAgentRepository::new(&state.db)
                .get_history_for_task(&id)
                .unwrap_or_default();
            let outcome = parse_outcome(&task);
            (
                StatusCode::OK,
                Json(
                    serde_json::to_value(TaskResponse {
                        task,
                        agents: Some(agents),
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

    fn make_agent_config(id: &str) -> openalpaca_storage::SubAgentConfig {
        openalpaca_storage::SubAgentConfig {
            id: id.to_string(),
            template_id: id.to_string(),
            name: id.to_string(),
            description: None,
            icon: None,
            status: "idle".to_string(),
            current_task_id: None,
            skills_json: "[]".to_string(),
            preset_json: "{}".to_string(),
            constraints_json: None,
            llm_config_json: None,
            persona: None,
            created_at: Utc::now(),
            updated_at: None,
        }
    }

    #[test]
    fn test_agent_runs_summary_from_seeded_history() {
        use openalpaca_storage::{AgentTaskHistory, Database};

        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        let task_repo = TaskRepository::new(&db);
        let mut task = make_test_task();
        task.id = "task-hist".to_string();
        task_repo.create(&task).unwrap();

        // No runs yet: the array is empty, not an error.
        assert!(agent_runs_summary(&db, "task-hist").is_empty());

        // Seed two agent runs (the shape record_agent_history writes).
        let sub_repo = SubAgentRepository::new(&db);
        sub_repo.upsert(&make_agent_config("researcher")).unwrap();
        sub_repo.upsert(&make_agent_config("writer")).unwrap();
        let base = Utc::now();
        sub_repo
            .add_history(&AgentTaskHistory {
                id: "h1".to_string(),
                agent_id: "researcher".to_string(),
                task_id: "task-hist".to_string(),
                role: "researcher".to_string(),
                status: "completed".to_string(),
                runtime_seconds: Some(12),
                completed_at: base,
            })
            .unwrap();
        sub_repo
            .add_history(&AgentTaskHistory {
                id: "h2".to_string(),
                agent_id: "writer".to_string(),
                task_id: "task-hist".to_string(),
                role: "writer".to_string(),
                status: "failed".to_string(),
                runtime_seconds: None,
                completed_at: base + chrono::Duration::seconds(30),
            })
            .unwrap();

        let agents = agent_runs_summary(&db, "task-hist");
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0]["agent_id"], "researcher");
        assert_eq!(agents[0]["status"], "completed");
        assert_eq!(agents[0]["runtime_seconds"], 12);
        assert_eq!(agents[1]["agent_id"], "writer");
        assert_eq!(agents[1]["status"], "failed");
        assert!(agents[1]["runtime_seconds"].is_null());
    }

    #[test]
    fn test_task_response_serializes_agent_runs_under_assignments_key() {
        use openalpaca_storage::AgentTaskHistory;

        let resp = TaskResponse {
            task: make_test_task(),
            agents: Some(vec![AgentTaskHistory {
                id: "h1".to_string(),
                agent_id: "researcher".to_string(),
                task_id: "task-1".to_string(),
                role: "researcher".to_string(),
                status: "completed".to_string(),
                runtime_seconds: Some(7),
                completed_at: Utc::now(),
            }]),
            outcome: None,
        };
        let v = serde_json::to_value(&resp).unwrap();
        // Legacy key kept for client compatibility (CLI/GUI parse "assignments").
        let runs = v["assignments"].as_array().expect("assignments array");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0]["agent_id"], "researcher");
        assert_eq!(runs[0]["status"], "completed");
        assert_eq!(runs[0]["runtime_seconds"], 7);
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
            agents: None,
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

    /// Reproduces `list_tasks_handler`'s pre-refactor shape: `serde_json::to_value(&task)`
    /// with `assigned_agents` (always) and `outcome` (only when it parses) inserted onto
    /// the object — the exact algorithm `TaskSummaryResponse` replaces. Used below to pin
    /// that the typed struct serializes to byte-for-byte the same shape.
    fn pre_refactor_shape(task: &Task, agents: Vec<serde_json::Value>) -> serde_json::Value {
        let mut v = serde_json::to_value(task).unwrap();
        if let Some(obj) = v.as_object_mut() {
            obj.insert("assigned_agents".to_string(), serde_json::json!(agents));
            let outcome_val =
                parse_outcome(task).and_then(|parsed| serde_json::to_value(parsed).ok());
            if let Some(outcome_val) = outcome_val {
                obj.insert("outcome".to_string(), outcome_val);
            }
        }
        v
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
        let agents = vec![serde_json::json!({
            "agent_id": "researcher",
            "role": "researcher",
            "status": "completed",
            "runtime_seconds": 12,
            "completed_at": task.completed_at,
        })];

        let expected = pre_refactor_shape(&task, agents.clone());
        let outcome = parse_outcome(&task);
        let summary = TaskSummaryResponse {
            task,
            assigned_agents: agents,
            outcome,
        };
        let actual = serde_json::to_value(&summary).unwrap();

        assert_eq!(actual, expected);
        // Sanity: the fields the old post-injection added are actually present,
        // so this test would fail if either one silently dropped out.
        assert!(actual.get("assigned_agents").is_some());
        assert!(actual.get("outcome").is_some());
        assert_eq!(actual["outcome"]["outcome_kind"], "text_only");
    }

    #[test]
    fn test_task_summary_response_matches_pre_refactor_shape_without_outcome() {
        // No outcome_kind/outcome_json set: parse_outcome returns None, and the
        // old code never inserted an "outcome" key in that case.
        let task = make_test_task();
        let agents: Vec<serde_json::Value> = Vec::new();

        let expected = pre_refactor_shape(&task, agents.clone());
        let outcome = parse_outcome(&task);
        assert!(outcome.is_none());
        let summary = TaskSummaryResponse {
            task,
            assigned_agents: agents,
            outcome,
        };
        let actual = serde_json::to_value(&summary).unwrap();

        assert_eq!(actual, expected);
        assert!(actual.get("outcome").is_none());
        // assigned_agents is still present, just empty — not omitted.
        assert_eq!(actual["assigned_agents"], serde_json::json!([]));
    }
}
