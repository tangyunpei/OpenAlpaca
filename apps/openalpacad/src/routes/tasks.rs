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
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use openalpaca_core::context::TaskEntryStatus;
use openalpaca_core::events::SystemEvent;
use openalpaca_core::lane::TaskLaneStatus;
use openalpaca_storage::{Task, TaskRepository, TaskStatus};

use crate::AppState;

// ── Request / Response Types ──────────────────────────────────────

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignments: Option<Vec<openalpaca_storage::TaskAgentAssignment>>,
}

// ── Handlers ──────────────────────────────────────────────────────

/// POST /v1/tasks
pub async fn create_task_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateTaskRequest>,
) -> impl IntoResponse {
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
                        Json(serde_json::json!({ "error": format!("Invalid status: {}", status_str) })),
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
            let enriched: Vec<serde_json::Value> = tasks.iter().map(|t| {
                let assignments = repo.get_assignments(&t.id).unwrap_or_default();
                let agents: Vec<serde_json::Value> = assignments.iter().map(|a| {
                    serde_json::json!({
                        "agent_id": a.agent_id,
                        "role": a.role,
                        "status": a.status.as_str()
                    })
                }).collect();
                let mut v = serde_json::to_value(t).unwrap();
                v.as_object_mut().unwrap().insert("assigned_agents".to_string(), serde_json::json!(agents));
                v
            }).collect();
            (StatusCode::OK, Json(serde_json::to_value(enriched).unwrap()))
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
            (
                StatusCode::OK,
                Json(
                    serde_json::to_value(TaskResponse {
                        task,
                        assignments: Some(assignments),
                    })
                    .unwrap(),
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
    let repo = TaskRepository::new(&state.db);

    // Fetch current task
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

    // Validate state transition
    let new_status = match request.action.as_str() {
        "cancel" => {
            if task.status.is_terminal() {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": format!("Cannot cancel a task in '{}' state", task.status)
                    })),
                );
            }
            TaskStatus::Cancelled
        }
        "pause" => {
            if task.status != TaskStatus::Running {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": format!("Can only pause a running task, current state: '{}'", task.status)
                    })),
                );
            }
            TaskStatus::Paused
        }
        "resume" => {
            if task.status != TaskStatus::Paused {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": format!("Can only resume a paused task, current state: '{}'", task.status)
                    })),
                );
            }
            TaskStatus::Running
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("Unknown action: '{}'. Valid: cancel, pause, resume", request.action)
                })),
            );
        }
    };

    // 1. Update DB
    if let Err(e) = repo.update_status(&id, new_status) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        );
    }

    // 2. Update in-memory registry
    let entry_status = match new_status {
        TaskStatus::Queued => TaskEntryStatus::Queued,
        TaskStatus::Running => TaskEntryStatus::Running,
        TaskStatus::Completed => TaskEntryStatus::Completed,
        TaskStatus::Failed => TaskEntryStatus::Failed,
        TaskStatus::Cancelled => TaskEntryStatus::Cancelled,
        TaskStatus::Paused => TaskEntryStatus::Paused,
    };
    state
        .gateway
        .shared_context
        .task_registry
        .update_status(&id, entry_status);

    // 3. Update task lane
    if let Some(lane) = state.gateway.lane_manager.get_task_lane(&id) {
        let lane_status = match new_status {
            TaskStatus::Queued => TaskLaneStatus::Queued,
            TaskStatus::Running => TaskLaneStatus::Running,
            TaskStatus::Completed => TaskLaneStatus::Completed,
            TaskStatus::Failed => TaskLaneStatus::Failed,
            TaskStatus::Cancelled => TaskLaneStatus::Cancelled,
            TaskStatus::Paused => TaskLaneStatus::Paused,
        };
        lane.set_status(lane_status);
    }

    // 4. Emit event
    let now = Utc::now();
    let event = match new_status {
        TaskStatus::Cancelled => SystemEvent::TaskUpdated {
            task_id: id.clone(),
            status: "cancelled".to_string(),
            progress_current: None,
            progress_total: None,
            timestamp: now,
        },
        _ => SystemEvent::TaskUpdated {
            task_id: id.clone(),
            status: new_status.as_str().to_string(),
            progress_current: None,
            progress_total: None,
            timestamp: now,
        },
    };
    let _ = state.gateway.bus.publish(event);

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "task_id": id,
            "status": new_status.as_str()
        })),
    )
}
