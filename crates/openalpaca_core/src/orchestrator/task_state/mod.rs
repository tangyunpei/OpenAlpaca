//! Working memory for tasks: structured state persisted as JSON in task.state_json.

use crate::orchestrator::task_planner::TaskDag;
use chrono::{DateTime, Utc};
use openalpaca_storage::OutcomeKind;
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
    /// Optional file asset ID for entries backed by uploaded/generated files.
    /// Enables artifact delivery to external channels (e.g. Telegram).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_asset_id: Option<String>,
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
            // Preserve existing file_asset_id (set separately via set_file_asset_id)
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
            file_asset_id: None,
        });
        Ok(())
    }

    /// Associate a file asset ID with an existing workspace entry.
    ///
    /// Called after a successful `write()` when the caller has a file asset
    /// to attach. This avoids changing the `write()` signature and its callers.
    pub fn set_file_asset_id(&mut self, key: &str, file_asset_id: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == key) {
            entry.file_asset_id = Some(file_asset_id.to_string());
        }
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

/// Structured outcome for a completed task.
/// Serialized to `task.outcome_json` for durable persistence.
///
/// Uses `OutcomeKind` from `openalpaca_storage` directly to maintain type-safety
/// across the storage/core boundary (core already depends on storage).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOutcome {
    /// Human-readable summary of the task result. Always non-empty.
    pub summary: String,
    /// Classified outcome kind. Typed enum shared with the storage layer.
    pub outcome_kind: OutcomeKind,
    /// Artifact pointers collected from all steps.
    pub artifacts: Vec<ArtifactPointer>,
    /// Explanation when no artifacts were produced (e.g. "Task was text-only analysis").
    pub no_artifact_reason: Option<String>,
}

/// A pointer to an artifact produced during task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactPointer {
    /// Workspace key or file path identifying the artifact.
    pub key: String,
    /// Human-readable label (e.g. "Research summary", "Generated report").
    pub label: String,
    /// The agent that produced this artifact.
    pub agent_id: String,
    /// Step order in the pipeline/DAG that produced this artifact.
    pub step_order: i32,
    /// Optional file asset ID for deliverable artifacts.
    /// When set, enables file delivery to external channels.
    #[serde(default)]
    pub file_asset_id: Option<String>,
}

impl StepState {
    /// Set the result summary for this step (capped at 500 chars).
    pub fn set_summary(&mut self, summary: &str) {
        self.result_summary = Some(summary.chars().take(500).collect());
    }

    /// Add an artifact pointer for this step.
    ///
    /// Stored as JSON `{"key":"...","label":"...","file_asset_id":"..."}` to avoid
    /// delimiter ambiguity (keys may contain `:` in file paths or workspace identifiers).
    pub fn add_artifact(&mut self, key: &str, label: &str, file_asset_id: Option<&str>) {
        let mut obj = serde_json::json!({"key": key, "label": label});
        if let Some(id) = file_asset_id {
            obj["file_asset_id"] = serde_json::Value::String(id.to_string());
        }
        self.artifact_pointers.push(obj.to_string());
    }

    /// Whether this step produced any artifacts.
    pub fn has_artifacts(&self) -> bool {
        !self.artifact_pointers.is_empty()
    }
}

/// Check if two agent identifiers refer to the same agent, accounting for the
/// non-singleton instance ID format `template_id::uuid`.
///
/// Returns true when:
/// - exact match (`"a1" == "a1"`)
/// - one is an instance of the other (`"a1::abc123" ~ "a1"` in either direction)
fn is_same_agent(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // a is instance of b: "researcher::abc123" starts with "researcher::"
    if a.starts_with(b)
        && a.as_bytes().get(b.len()) == Some(&b':')
        && a.as_bytes().get(b.len() + 1) == Some(&b':')
    {
        return true;
    }
    // b is instance of a: "researcher::abc123" starts with "researcher::"
    if b.starts_with(a)
        && b.as_bytes().get(a.len()) == Some(&b':')
        && b.as_bytes().get(a.len() + 1) == Some(&b':')
    {
        return true;
    }
    false
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

    /// Set a step's result summary (convenience wrapper).
    pub fn set_step_summary(&mut self, step_order: i32, summary: &str) {
        if let Some(step) = self.steps.iter_mut().find(|s| s.step_order == step_order) {
            step.set_summary(summary);
        }
        self.updated_at = Utc::now();
    }

    /// Add an artifact pointer to a specific step.
    pub fn add_step_artifact(&mut self, step_order: i32, key: &str, label: &str, file_asset_id: Option<&str>) {
        if let Some(step) = self.steps.iter_mut().find(|s| s.step_order == step_order) {
            step.add_artifact(key, label, file_asset_id);
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
            .filter(|e| is_same_agent(&e.author_agent_id, &agent_id))
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

    /// Collect all artifact pointers from all completed steps.
    /// Returns them in step_order, each annotated with agent_id and step_order.
    ///
    /// Only completed steps contribute artifacts. A partially-failed pipeline
    /// will collect artifacts from completed steps only.
    pub fn collect_artifacts(&self) -> Vec<ArtifactPointer> {
        let mut artifacts = Vec::new();
        for step in &self.steps {
            if step.status == "completed" {
                for raw_pointer in &step.artifact_pointers {
                    // Parse JSON format: {"key":"...","label":"...","file_asset_id":"..."}
                    // Falls back to using the raw string as both key and label
                    let (key, label, file_asset_id) = match serde_json::from_str::<serde_json::Value>(raw_pointer)
                    {
                        Ok(v) => (
                            v["key"].as_str().unwrap_or(raw_pointer).to_string(),
                            v["label"].as_str().unwrap_or(raw_pointer).to_string(),
                            v["file_asset_id"].as_str().map(|s| s.to_string()),
                        ),
                        Err(_) => (raw_pointer.clone(), raw_pointer.clone(), None),
                    };
                    artifacts.push(ArtifactPointer {
                        key,
                        label,
                        agent_id: step.agent_id.clone(),
                        step_order: step.step_order,
                        file_asset_id,
                    });
                }
            }
        }
        artifacts
    }

    /// Build a TaskOutcome from the current state.
    ///
    /// Classification rules:
    /// - If ALL steps failed: `OutcomeKind::Failed`
    /// - If artifacts exist AND text summary exists: `OutcomeKind::Mixed`
    /// - If artifacts exist but no text summary: `OutcomeKind::ArtifactOnly`
    /// - Otherwise (text only, or no steps completed): `OutcomeKind::TextOnly`
    ///
    /// `fallback_summary` is used when no step has a result_summary.
    pub fn build_outcome(
        &self,
        fallback_summary: &str,
        no_artifact_reason: Option<String>,
    ) -> TaskOutcome {
        let artifacts = self.collect_artifacts();
        let has_artifacts = !artifacts.is_empty();

        let has_summary = self.steps.iter().any(|s| {
            s.status == "completed" && s.result_summary.as_ref().is_some_and(|r| !r.is_empty())
        });

        let all_failed =
            !self.steps.is_empty() && self.steps.iter().all(|s| s.status == "failed");

        let outcome_kind = if all_failed {
            OutcomeKind::Failed
        } else if has_artifacts && has_summary {
            OutcomeKind::Mixed
        } else if has_artifacts {
            OutcomeKind::ArtifactOnly
        } else {
            OutcomeKind::TextOnly
        };

        // Build summary: concatenate completed step summaries, fall back to provided fallback.
        let step_summaries: Vec<String> = self
            .steps
            .iter()
            .filter(|s| s.status == "completed")
            .filter_map(|s| s.result_summary.clone())
            .filter(|s| !s.is_empty())
            .collect();

        let summary = if step_summaries.is_empty() {
            fallback_summary.to_string()
        } else {
            step_summaries.join("\n\n")
        };

        // Ensure summary is never empty
        let summary = if summary.is_empty() {
            "Task completed.".to_string()
        } else {
            summary
        };

        let no_artifact_reason = if !has_artifacts {
            no_artifact_reason.or_else(|| Some("No artifacts were produced.".to_string()))
        } else {
            None
        };

        TaskOutcome {
            summary,
            outcome_kind,
            artifacts,
            no_artifact_reason,
        }
    }

    /// Collect artifact pointers from workspace entries of type Artifact.
    /// Uses `step_order = -1` for workspace-sourced artifacts since they are
    /// not associated with a specific pipeline step or DAG node.
    pub fn collect_artifacts_from_workspace(&self) -> Vec<ArtifactPointer> {
        self.workspace
            .entries
            .iter()
            .filter(|e| e.entry_type == WorkspaceEntryType::Artifact)
            .map(|e| ArtifactPointer {
                key: e.key.clone(),
                label: e.key.clone(),
                agent_id: e.author_agent_id.clone(),
                step_order: -1,
                file_asset_id: e.file_asset_id.clone(),
            })
            .collect()
    }

    /// Build outcome from DAG node data (not steps).
    /// Falls back to `build_outcome()` if no DAG or empty nodes.
    pub fn build_outcome_dag(
        &self,
        fallback_summary: &str,
        no_artifact_reason: Option<String>,
    ) -> TaskOutcome {
        use crate::orchestrator::task_planner::DagNodeStatus;

        let dag = match &self.dag {
            Some(d) if !d.nodes.is_empty() => d,
            _ => return self.build_outcome(fallback_summary, no_artifact_reason),
        };

        // Collect artifacts: step-based + workspace-based, deduplicated
        let mut artifacts = self.collect_artifacts();
        let existing_keys: std::collections::HashSet<String> =
            artifacts.iter().map(|a| a.key.clone()).collect();
        for wa in self.collect_artifacts_from_workspace() {
            if !existing_keys.contains(&wa.key) {
                artifacts.push(wa);
            }
        }
        let has_artifacts = !artifacts.is_empty();

        // Classify from DAG node statuses
        let all_failed = dag.nodes.iter().all(|n| {
            matches!(n.status, DagNodeStatus::Failed | DagNodeStatus::Skipped)
        });

        // Build summary from completed node result_summaries
        let node_summaries: Vec<&str> = dag
            .nodes
            .iter()
            .filter(|n| n.status == DagNodeStatus::Completed)
            .filter_map(|n| n.result_summary.as_deref())
            .filter(|s| !s.is_empty())
            .collect();

        let summary = if node_summaries.is_empty() {
            if fallback_summary.is_empty() {
                "Task completed.".to_string()
            } else {
                fallback_summary.to_string()
            }
        } else if node_summaries.len() == 1 {
            node_summaries[0].to_string()
        } else {
            node_summaries
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{}. {}", i + 1, s))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let has_text_summary = !node_summaries.is_empty();
        let outcome_kind = if all_failed {
            OutcomeKind::Failed
        } else if has_artifacts && has_text_summary {
            OutcomeKind::Mixed
        } else if has_artifacts {
            OutcomeKind::ArtifactOnly
        } else {
            OutcomeKind::TextOnly
        };

        let no_artifact_reason = if !has_artifacts {
            no_artifact_reason.or_else(|| Some("No artifacts were produced.".to_string()))
        } else {
            None
        };

        TaskOutcome {
            summary,
            outcome_kind,
            artifacts,
            no_artifact_reason,
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

#[cfg(test)]
mod tests;
