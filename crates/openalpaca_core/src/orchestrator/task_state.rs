//! Working memory for tasks: structured state persisted as JSON in task.state_json.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The structured working memory for a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskState {
    pub objective: String,
    pub steps: Vec<StepState>,
    pub constraints: TaskConstraints,
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
pub struct TaskConstraints {
    pub max_agents: usize,
    pub pipeline_sequential: bool,
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
            constraints: TaskConstraints {
                max_agents: assignments.len(),
                pipeline_sequential: true,
            },
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

    /// Serialize to JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_assignments() -> Vec<(String, String, String)> {
        vec![
            ("a1".to_string(), "Agent A1".to_string(), "Researcher".to_string()),
            ("a2".to_string(), "Agent A2".to_string(), "Writer".to_string()),
        ]
    }

    #[test]
    fn test_initial_state() {
        let state = TaskState::initial("Test objective", &make_assignments());
        assert_eq!(state.objective, "Test objective");
        assert_eq!(state.steps.len(), 2);
        assert_eq!(state.steps[0].step_order, 0);
        assert_eq!(state.steps[0].agent_id, "a1");
        assert_eq!(state.steps[0].status, "pending");
        assert_eq!(state.steps[1].step_order, 1);
        assert_eq!(state.steps[1].agent_id, "a2");
        assert_eq!(state.constraints.max_agents, 2);
        assert!(state.constraints.pipeline_sequential);
    }

    #[test]
    fn test_mark_step_running() {
        let mut state = TaskState::initial("obj", &make_assignments());
        state.mark_step_running(0);
        assert_eq!(state.steps[0].status, "running");
        assert!(state.steps[0].started_at.is_some());
        assert_eq!(state.steps[1].status, "pending");
    }

    #[test]
    fn test_mark_step_completed() {
        let mut state = TaskState::initial("obj", &make_assignments());
        state.mark_step_running(0);
        state.mark_step_completed(0, "Done successfully");
        assert_eq!(state.steps[0].status, "completed");
        assert_eq!(state.steps[0].result_summary.as_deref(), Some("Done successfully"));
        assert!(state.steps[0].completed_at.is_some());
    }

    #[test]
    fn test_mark_step_completed_caps_summary() {
        let mut state = TaskState::initial("obj", &make_assignments());
        let long_summary = "x".repeat(600);
        state.mark_step_completed(0, &long_summary);
        assert_eq!(state.steps[0].result_summary.as_ref().unwrap().len(), 500);
    }

    #[test]
    fn test_mark_step_failed() {
        let mut state = TaskState::initial("obj", &make_assignments());
        state.mark_step_running(0);
        state.mark_step_failed(0, "Something went wrong");
        assert_eq!(state.steps[0].status, "failed");
        assert_eq!(state.steps[0].result_summary.as_deref(), Some("Something went wrong"));
        assert!(state.steps[0].completed_at.is_some());
    }

    #[test]
    fn test_mark_step_failed_caps_error() {
        let mut state = TaskState::initial("obj", &make_assignments());
        let long_error = "e".repeat(600);
        state.mark_step_failed(0, &long_error);
        assert_eq!(state.steps[0].result_summary.as_ref().unwrap().len(), 500);
    }

    #[test]
    fn test_to_json_roundtrip() {
        let mut state = TaskState::initial("Test roundtrip", &make_assignments());
        state.mark_step_running(0);
        state.mark_step_completed(0, "Step 1 done");

        let json = state.to_json();
        let deserialized: TaskState = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.objective, "Test roundtrip");
        assert_eq!(deserialized.steps.len(), 2);
        assert_eq!(deserialized.steps[0].status, "completed");
        assert_eq!(deserialized.steps[0].result_summary.as_deref(), Some("Step 1 done"));
        assert_eq!(deserialized.steps[1].status, "pending");
    }
}
