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
mod tests;
