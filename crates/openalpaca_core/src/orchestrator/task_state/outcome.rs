//! TaskOutcome construction, artifact collection, and DAG-aware outcome building.

use super::state::TaskState;
use super::workspace::WorkspaceEntryType;
use openalpaca_storage::OutcomeKind;
use serde::{Deserialize, Serialize};

/// Structured outcome for a completed task.
/// Serialized to `task.outcome_json` for durable persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOutcome {
    /// Human-readable summary of the task result. Always non-empty.
    pub summary: String,
    /// Classified outcome kind. Typed enum shared with the storage layer.
    pub outcome_kind: OutcomeKind,
    /// Artifact pointers collected from all steps.
    pub artifacts: Vec<ArtifactPointer>,
    /// Explanation when no artifacts were produced.
    pub no_artifact_reason: Option<String>,
}

/// A pointer to an artifact produced during task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactPointer {
    /// Workspace key or file path identifying the artifact.
    pub key: String,
    /// Human-readable label.
    pub label: String,
    /// The agent that produced this artifact.
    pub agent_id: String,
    /// Step order in the pipeline/DAG that produced this artifact.
    pub step_order: i32,
    /// Optional file asset ID for deliverable artifacts.
    #[serde(default)]
    pub file_asset_id: Option<String>,
}

/// Check if two agent identifiers refer to the same agent, accounting for the
/// non-singleton instance ID format `template_id::uuid`.
pub(super) fn is_same_agent(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if a.starts_with(b)
        && a.as_bytes().get(b.len()) == Some(&b':')
        && a.as_bytes().get(b.len() + 1) == Some(&b':')
    {
        return true;
    }
    if b.starts_with(a)
        && b.as_bytes().get(a.len()) == Some(&b':')
        && b.as_bytes().get(a.len() + 1) == Some(&b':')
    {
        return true;
    }
    false
}

impl TaskState {
    /// Collect all artifact pointers from all completed steps.
    pub fn collect_artifacts(&self) -> Vec<ArtifactPointer> {
        let mut artifacts = Vec::new();
        for step in &self.steps {
            if step.status == "completed" {
                for raw_pointer in &step.artifact_pointers {
                    let (key, label, file_asset_id) =
                        match serde_json::from_str::<serde_json::Value>(raw_pointer) {
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

        let mut artifacts = self.collect_artifacts();
        let existing_keys: std::collections::HashSet<String> =
            artifacts.iter().map(|a| a.key.clone()).collect();
        for wa in self.collect_artifacts_from_workspace() {
            if !existing_keys.contains(&wa.key) {
                artifacts.push(wa);
            }
        }
        let has_artifacts = !artifacts.is_empty();

        let all_failed = dag.nodes.iter().all(|n| {
            matches!(n.status, DagNodeStatus::Failed | DagNodeStatus::Skipped)
        });

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
}
