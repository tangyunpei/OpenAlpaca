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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignments: Option<Vec<openalpaca_storage::TaskAgentAssignment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ParsedOutcomeFields>,
}
