//! Working memory for tasks: structured state persisted as JSON in task.state_json.

use crate::orchestrator::task_planner::TaskDag;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Workspace types ──────────────────────────────────────────────────

/// The type of a workspace entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum WorkspaceEntryType {
    #[default]
    Text,
    Artifact,
    Summary,
    Context,
}

/// A single entry in the shared workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub key: String,
    pub content: String,
    pub author_agent_id: String,
    pub entry_type: WorkspaceEntryType,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The shared workspace for a task — all agents can read/write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskWorkspace {
    pub entries: Vec<WorkspaceEntry>,
    pub max_entries: usize,
    pub max_entry_size: usize,
}

impl Default for TaskWorkspace {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            max_entries: 50,
            max_entry_size: 32768,
        }
    }
}

impl TaskWorkspace {
    /// Read a single entry by key, or return all entries if key is empty.
    pub fn read(&self, key: &str) -> Vec<&WorkspaceEntry> {
        if key.is_empty() {
            self.entries.iter().collect()
        } else {
            self.entries.iter().filter(|e| e.key == key).collect()
        }
    }

    /// List all keys with their types (for discovery).
    pub fn list_keys(&self) -> Vec<(&str, &WorkspaceEntryType)> {
        self.entries
            .iter()
            .map(|e| (e.key.as_str(), &e.entry_type))
            .collect()
    }

    /// Write (upsert) an entry. Returns Ok on success, Err if limits exceeded.
    pub fn write(
        &mut self,
        key: &str,
        content: &str,
        author_agent_id: &str,
        entry_type: WorkspaceEntryType,
        protected_keys: &[String],
    ) -> Result<(), String> {
        if content.len() > self.max_entry_size {
            tracing::warn!(
                "Workspace entry '{}' truncated from {} to {} chars",
                key,
                content.len(),
                self.max_entry_size
            );
        }
        let capped_content: String = content.chars().take(self.max_entry_size).collect();
        let now = Utc::now();

        // Upsert: update if key exists, insert otherwise
        if let Some(existing) = self.entries.iter_mut().find(|e| e.key == key) {
            existing.content = capped_content;
            existing.author_agent_id = author_agent_id.to_string();
            existing.entry_type = entry_type;
            existing.updated_at = now;
            return Ok(());
        }

        if self.entries.len() >= self.max_entries {
            // Evict the oldest non-protected entry (by updated_at) to make room
            if let Some(oldest_idx) = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| !protected_keys.contains(&e.key))
                .min_by_key(|(_, e)| e.updated_at)
                .map(|(i, _)| i)
            {
                let evicted_key = self.entries[oldest_idx].key.clone();
                self.entries.remove(oldest_idx);
                tracing::debug!(
                    "Workspace full — evicted oldest entry '{}' to make room for '{}'",
                    evicted_key,
                    key
                );
            } else {
                return Err(format!(
                    "Workspace full ({} entries) and all are protected — cannot write '{}'",
                    self.max_entries, key
                ));
            }
        }

        self.entries.push(WorkspaceEntry {
            key: key.to_string(),
            content: capped_content,
            author_agent_id: author_agent_id.to_string(),
            entry_type,
            created_at: now,
            updated_at: now,
        });
        Ok(())
    }

    /// Format workspace contents as a context string for agent prompts.
    pub fn format_for_prompt(&self, keys: &[String]) -> String {
        let relevant: Vec<&WorkspaceEntry> = if keys.is_empty() {
            self.entries.iter().collect()
        } else {
            self.entries
                .iter()
                .filter(|e| keys.contains(&e.key))
                .collect()
        };

        if relevant.is_empty() {
            return String::new();
        }

        let mut out = String::from("## Shared Workspace\n\n");
        for entry in relevant {
            let preview: String = entry.content.chars().take(2000).collect();
            out.push_str(&format!(
                "### [{}] (by {}, type: {:?})\n{}\n\n",
                entry.key, entry.author_agent_id, entry.entry_type, preview
            ));
        }
        out
    }
}

// ── Task state ───────────────────────────────────────────────────────

/// The structured working memory for a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskState {
    pub objective: String,
    pub steps: Vec<StepState>,
    pub constraints: TaskConstraints,
    #[serde(default)]
    pub workspace: TaskWorkspace,
    #[serde(default)]
    pub dag: Option<TaskDag>,
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
            workspace: TaskWorkspace::default(),
            dag: None,
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
        match serde_json::to_string(self) {
            Ok(json) => json,
            Err(e) => {
                tracing::error!("Failed to serialize TaskState: {}", e);
                "{}".to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_assignments() -> Vec<(String, String, String)> {
        vec![
            (
                "a1".to_string(),
                "Agent A1".to_string(),
                "Researcher".to_string(),
            ),
            (
                "a2".to_string(),
                "Agent A2".to_string(),
                "Writer".to_string(),
            ),
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
        assert_eq!(
            state.steps[0].result_summary.as_deref(),
            Some("Done successfully")
        );
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
        assert_eq!(
            state.steps[0].result_summary.as_deref(),
            Some("Something went wrong")
        );
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
        assert_eq!(
            deserialized.steps[0].result_summary.as_deref(),
            Some("Step 1 done")
        );
        assert_eq!(deserialized.steps[1].status, "pending");
    }

    // ── Workspace tests ──────────────────────────────────────────────

    #[test]
    fn test_workspace_write_and_read() {
        let mut ws = TaskWorkspace::default();
        ws.write("key1", "hello", "agent_a", WorkspaceEntryType::Text, &[])
            .unwrap();
        ws.write("key2", "world", "agent_b", WorkspaceEntryType::Summary, &[])
            .unwrap();

        let all = ws.read("");
        assert_eq!(all.len(), 2);

        let one = ws.read("key1");
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].content, "hello");
        assert_eq!(one[0].author_agent_id, "agent_a");
    }

    #[test]
    fn test_workspace_upsert() {
        let mut ws = TaskWorkspace::default();
        ws.write("key1", "v1", "agent_a", WorkspaceEntryType::Text, &[])
            .unwrap();
        ws.write("key1", "v2", "agent_b", WorkspaceEntryType::Artifact, &[])
            .unwrap();

        assert_eq!(ws.entries.len(), 1);
        assert_eq!(ws.entries[0].content, "v2");
        assert_eq!(ws.entries[0].author_agent_id, "agent_b");
        assert_eq!(ws.entries[0].entry_type, WorkspaceEntryType::Artifact);
    }

    #[test]
    fn test_workspace_caps_content() {
        let mut ws = TaskWorkspace {
            max_entry_size: 10,
            ..Default::default()
        };
        ws.write(
            "key1",
            "abcdefghijklmnop",
            "agent_a",
            WorkspaceEntryType::Text,
            &[],
        )
        .unwrap();
        assert_eq!(ws.entries[0].content.len(), 10);
    }

    #[test]
    fn test_workspace_max_entries_evicts_oldest() {
        let mut ws = TaskWorkspace {
            max_entries: 2,
            ..Default::default()
        };
        ws.write("k1", "a", "agent", WorkspaceEntryType::Text, &[])
            .unwrap();
        ws.write("k2", "b", "agent", WorkspaceEntryType::Text, &[])
            .unwrap();
        // Third write should evict the oldest entry ("k1") instead of failing
        let result = ws.write("k3", "c", "agent", WorkspaceEntryType::Text, &[]);
        assert!(result.is_ok());
        assert_eq!(ws.entries.len(), 2);
        // k1 should be evicted, k2 and k3 should remain
        assert!(ws.read("k1").is_empty());
        assert!(!ws.read("k2").is_empty());
        assert!(!ws.read("k3").is_empty());
    }

    #[test]
    fn test_workspace_list_keys() {
        let mut ws = TaskWorkspace::default();
        ws.write("research", "data", "agent_a", WorkspaceEntryType::Text, &[])
            .unwrap();
        ws.write(
            "outline",
            "structure",
            "agent_b",
            WorkspaceEntryType::Summary,
            &[],
        )
        .unwrap();

        let keys = ws.list_keys();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].0, "research");
        assert_eq!(keys[1].0, "outline");
    }

    #[test]
    fn test_workspace_format_for_prompt() {
        let mut ws = TaskWorkspace::default();
        ws.write(
            "research",
            "AI data",
            "agent_a",
            WorkspaceEntryType::Text,
            &[],
        )
        .unwrap();
        ws.write(
            "outline",
            "sections",
            "agent_b",
            WorkspaceEntryType::Summary,
            &[],
        )
        .unwrap();

        // Format all
        let all = ws.format_for_prompt(&[]);
        assert!(all.contains("research"));
        assert!(all.contains("outline"));

        // Format filtered
        let filtered = ws.format_for_prompt(&["research".to_string()]);
        assert!(filtered.contains("research"));
        assert!(!filtered.contains("outline"));

        // Format empty workspace
        let empty_ws = TaskWorkspace::default();
        assert!(empty_ws.format_for_prompt(&[]).is_empty());
    }

    #[test]
    fn test_workspace_roundtrip_in_task_state() {
        let mut state = TaskState::initial("Test workspace", &make_assignments());
        state
            .workspace
            .write("key1", "data", "agent", WorkspaceEntryType::Text, &[])
            .unwrap();

        let json = state.to_json();
        let deserialized: TaskState = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.workspace.entries.len(), 1);
        assert_eq!(deserialized.workspace.entries[0].key, "key1");
        assert_eq!(deserialized.workspace.entries[0].content, "data");
    }

    #[test]
    fn test_workspace_eviction_respects_protected_keys() {
        let mut ws = TaskWorkspace {
            max_entries: 2,
            ..Default::default()
        };
        ws.write("k1", "a", "agent", WorkspaceEntryType::Text, &[])
            .unwrap();
        ws.write("k2", "b", "agent", WorkspaceEntryType::Text, &[])
            .unwrap();
        // k1 is protected, so k2 (the oldest unprotected) should be evicted
        let result = ws.write(
            "k3",
            "c",
            "agent",
            WorkspaceEntryType::Text,
            &["k1".to_string()],
        );
        assert!(result.is_ok());
        assert!(!ws.read("k1").is_empty()); // protected — kept
        assert!(ws.read("k2").is_empty()); // evicted
        assert!(!ws.read("k3").is_empty()); // written
    }

    #[test]
    fn test_workspace_eviction_all_protected_returns_error() {
        let mut ws = TaskWorkspace {
            max_entries: 2,
            ..Default::default()
        };
        ws.write("k1", "a", "agent", WorkspaceEntryType::Text, &[])
            .unwrap();
        ws.write("k2", "b", "agent", WorkspaceEntryType::Text, &[])
            .unwrap();
        // Both existing keys are protected — write should fail
        let result = ws.write(
            "k3",
            "c",
            "agent",
            WorkspaceEntryType::Text,
            &["k1".to_string(), "k2".to_string()],
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("all are protected"));
    }

    #[test]
    fn test_backward_compat_no_workspace_field() {
        // Simulate old TaskState JSON without workspace field
        let old_json = r#"{
            "objective": "test",
            "steps": [],
            "constraints": {"max_agents": 1, "pipeline_sequential": true},
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z"
        }"#;
        let state: TaskState = serde_json::from_str(old_json).unwrap();
        assert_eq!(state.objective, "test");
        assert!(state.workspace.entries.is_empty());
        assert_eq!(state.workspace.max_entries, 50);
    }
}
