//! Task state, step state, and constraints.

use super::workspace::{TaskWorkspace, WorkspaceEntryType};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The structured working memory for a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskState {
    pub objective: String,
    pub steps: Vec<StepState>,
    pub constraints: TaskConstraints,
    #[serde(default)]
    pub workspace: TaskWorkspace,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Per-step state within a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepState {
    pub step_order: i32,
    pub agent_id: String,
    pub agent_name: String,
    pub role: String,
    pub status: String,
    pub result_summary: Option<String>,
    pub artifact_pointers: Vec<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Constraints governing task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskConstraints {}

impl StepState {
    /// Add an artifact pointer for this step.
    pub fn add_artifact(&mut self, key: &str, label: &str, file_asset_id: Option<&str>) {
        let mut obj = serde_json::json!({"key": key, "label": label});
        if let Some(id) = file_asset_id {
            obj["file_asset_id"] = serde_json::Value::String(id.to_string());
        }
        self.artifact_pointers.push(obj.to_string());
    }
}

impl TaskState {
    /// Create the initial state for a new task.
    pub fn initial(objective: &str, assignments: &[(String, String, String)]) -> Self {
        let now = Utc::now();
        let steps = assignments
            .iter()
            .enumerate()
            .map(|(i, (agent_id, agent_name, role))| StepState {
                step_order: i as i32,
                agent_id: agent_id.clone(),
                agent_name: agent_name.clone(),
                role: role.clone(),
                status: "pending".to_string(),
                result_summary: None,
                artifact_pointers: Vec::new(),
                started_at: None,
                completed_at: None,
            })
            .collect();

        TaskState {
            objective: objective.to_string(),
            steps,
            constraints: TaskConstraints {},
            workspace: TaskWorkspace::default(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Mark a step as running.
    pub fn mark_step_running(&mut self, step_order: i32) {
        if let Some(step) = self.steps.iter_mut().find(|s| s.step_order == step_order) {
            step.status = "running".to_string();
            step.started_at = Some(Utc::now());
        }
        self.updated_at = Utc::now();
    }

    /// Mark a step as completed with a summary (capped at 500 chars).
    pub fn mark_step_completed(&mut self, step_order: i32, summary: &str) {
        if let Some(step) = self.steps.iter_mut().find(|s| s.step_order == step_order) {
            step.status = "completed".to_string();
            step.result_summary = Some(summary.chars().take(500).collect());
            step.completed_at = Some(Utc::now());
        }
        self.updated_at = Utc::now();
    }

    /// Mark a step as failed with an error message (capped at 500 chars).
    pub fn mark_step_failed(&mut self, step_order: i32, error: &str) {
        if let Some(step) = self.steps.iter_mut().find(|s| s.step_order == step_order) {
            step.status = "failed".to_string();
            step.result_summary = Some(error.chars().take(500).collect());
            step.completed_at = Some(Utc::now());
        }
        self.updated_at = Utc::now();
    }

    /// Scan workspace for Artifact-type entries authored by the given step's agent
    /// and register them as artifact pointers on that step (avoiding duplicates).
    pub fn scan_workspace_artifacts(&mut self, step_order: i32) {
        let agent_id = match self.steps.iter().find(|s| s.step_order == step_order) {
            Some(s) => s.agent_id.clone(),
            None => return,
        };

        // Collect existing artifact keys to avoid duplicates
        let existing_keys: Vec<String> = self
            .steps
            .iter()
            .find(|s| s.step_order == step_order)
            .map(|s| {
                s.artifact_pointers
                    .iter()
                    .filter_map(|raw| {
                        serde_json::from_str::<serde_json::Value>(raw)
                            .ok()
                            .and_then(|v| v["key"].as_str().map(|s| s.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let new_artifacts: Vec<(String, String, Option<String>)> = self
            .workspace
            .entries
            .iter()
            .filter(|e| e.entry_type == WorkspaceEntryType::Artifact)
            .filter(|e| super::outcome::is_same_agent(&e.author_agent_id, &agent_id))
            .filter(|e| !existing_keys.contains(&e.key))
            .map(|e| (e.key.clone(), e.key.clone(), e.file_asset_id.clone()))
            .collect();

        if !new_artifacts.is_empty() {
            if let Some(step) = self.steps.iter_mut().find(|s| s.step_order == step_order) {
                for (key, label, file_asset_id) in new_artifacts {
                    step.add_artifact(&key, &label, file_asset_id.as_deref());
                }
            }
            self.updated_at = Utc::now();
        }
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> String {
        match serde_json::to_string(self) {
            Ok(json) => json,
            Err(e) => {
                tracing::error!("Failed to serialize TaskState: {}", e);
                "{}".to_string()
            }
        }
    }
}
