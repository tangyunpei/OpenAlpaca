use super::Orchestrator;
use super::{db_task_to_json, task_entry_to_json};
use crate::context::TaskEntryStatus;
use crate::events::SystemEvent;
use crate::lane::TaskLaneStatus;
use chrono::Utc;
use openalpaca_storage::repository::TaskRepository;

impl Orchestrator {
    pub(super) fn handle_task_query(
        &self,
        task_id: Option<String>,
        created_by: &str,
    ) -> Result<String, String> {
        match task_id {
            Some(id) => {
                // Try DB first, fall back to in-memory registry
                if let Some(ref db) = self.db {
                    let repo = TaskRepository::new(db);
                    if let Ok(Some(task)) = repo.get(&id) {
                        return Ok(db_task_to_json(&task));
                    }
                }
                match self.shared_context.task_registry.get(&id) {
                    Some(entry) => Ok(task_entry_to_json(&entry)),
                    None => Ok(serde_json::json!({
                        "error": "not_found",
                        "message": format!("Task '{}' not found", id)
                    })
                    .to_string()),
                }
            }
            None => {
                // Try DB first, fall back to in-memory registry
                if let Some(ref db) = self.db {
                    let repo = TaskRepository::new(db);
                    if let Ok(mut tasks) = repo.list_active_by_creator(created_by, 20) {
                        let mut scope = "active";
                        if tasks.is_empty()
                            && let Ok(recent) = repo.list_by_creator(created_by, 20)
                        {
                            // If nothing is running, surface recent terminal tasks so
                            // "what was the result?" queries can still resolve.
                            tasks = recent
                                .into_iter()
                                .filter(|t| t.status.is_terminal())
                                .take(5)
                                .collect();
                            scope = "recent_terminal";
                        }

                        let task_list: Vec<serde_json::Value> = tasks
                            .iter()
                            .map(|t| {
                                let (outcome_summary, no_artifact_reason, artifacts) = t
                                    .outcome_json
                                    .as_deref()
                                    .and_then(|oj| {
                                        serde_json::from_str::<serde_json::Value>(oj).ok()
                                    })
                                    .map(|v| {
                                        (
                                            v.get("summary")
                                                .and_then(|s| s.as_str())
                                                .map(String::from),
                                            v.get("no_artifact_reason")
                                                .and_then(|s| s.as_str())
                                                .map(String::from),
                                            v.get("artifacts")
                                                .cloned()
                                                .unwrap_or(serde_json::json!([])),
                                        )
                                    })
                                    .unwrap_or((None, None, serde_json::json!([])));

                                serde_json::json!({
                                    "task_id": t.id,
                                    "title": t.title,
                                    "status": t.status.as_str(),
                                    "progress_current": t.progress_current,
                                    "progress_total": t.progress_total,
                                    "result_summary": t.result_summary,
                                    "created_at": t.created_at.to_rfc3339(),
                                    "completed_at": t.completed_at.map(|ts| ts.to_rfc3339()),
                                    "outcome_kind": t.outcome_kind.map(|k| k.as_str()),
                                    "artifact_count": t.artifact_count,
                                    "outcome_summary": outcome_summary,
                                    "no_artifact_reason": no_artifact_reason,
                                    "artifacts": artifacts,
                                })
                            })
                            .collect();
                        return Ok(serde_json::json!({
                            "tasks": task_list,
                            "count": task_list.len(),
                            "scope": scope,
                        })
                        .to_string());
                    }
                }
                let active = self.shared_context.task_registry.list_active();
                let tasks: Vec<serde_json::Value> = active
                    .iter()
                    .map(|e| {
                        let mut v = serde_json::json!({
                            "task_id": e.task_id,
                            "title": e.title,
                            "status": e.status.as_str(),
                            "progress_current": e.progress_current,
                            "progress_total": e.progress_total,
                        });
                        if let Some(ref dag) = e.dag_summary {
                            v.as_object_mut().unwrap().insert(
                                "dag_summary".to_string(),
                                serde_json::json!({
                                    "total_nodes": dag.total_nodes,
                                    "completed_nodes": dag.completed_nodes,
                                    "running_nodes": dag.running_nodes,
                                    "failed_nodes": dag.failed_nodes,
                                }),
                            );
                        }
                        v
                    })
                    .collect();
                Ok(serde_json::json!({
                    "tasks": tasks,
                    "count": tasks.len(),
                })
                .to_string())
            }
        }
    }

    pub(super) fn handle_task_control(
        &self,
        task_id: &str,
        action: &str,
    ) -> Result<String, String> {
        // Fetch current state
        let entry = self
            .shared_context
            .task_registry
            .get(task_id)
            .ok_or_else(|| format!("Task '{}' not found", task_id))?;

        // Validate state transition
        let new_status = match action {
            "cancel" => {
                if entry.status.is_terminal() {
                    return Err(format!(
                        "Cannot cancel task in '{}' state",
                        entry.status.as_str()
                    ));
                }
                TaskEntryStatus::Cancelled
            }
            "pause" => {
                if entry.status != TaskEntryStatus::Running {
                    return Err(format!(
                        "Can only pause a running task, current: '{}'",
                        entry.status.as_str()
                    ));
                }
                TaskEntryStatus::Paused
            }
            "resume" => {
                if entry.status != TaskEntryStatus::Paused {
                    return Err(format!(
                        "Can only resume a paused task, current: '{}'",
                        entry.status.as_str()
                    ));
                }
                TaskEntryStatus::Running
            }
            _ => return Err(format!("Unknown action: '{}'", action)),
        };

        // Update task registry
        self.shared_context
            .task_registry
            .update_status(task_id, new_status);

        // Trigger the CancellationToken so background execution tasks actually stop.
        // Without this, the tokio tasks (DAG, pipeline, lead agent) continue running
        // because they only check token.is_cancelled() in their event loops.
        if new_status == TaskEntryStatus::Cancelled {
            let cancelled = self.shared_context.cancel_task(task_id);
            if cancelled {
                tracing::info!("Triggered cancellation token for task '{}'", task_id);
            } else {
                tracing::warn!(
                    "No cancellation token found for task '{}' — task may have already finished",
                    task_id
                );
            }
        }

        // Update task lane if present
        if let Some(lane) = self.lane_manager.get_task_lane(task_id) {
            let lane_status = match new_status {
                TaskEntryStatus::Queued => TaskLaneStatus::Queued,
                TaskEntryStatus::Running => TaskLaneStatus::Running,
                TaskEntryStatus::Completed => TaskLaneStatus::Completed,
                TaskEntryStatus::Failed => TaskLaneStatus::Failed,
                TaskEntryStatus::Cancelled => TaskLaneStatus::Cancelled,
                TaskEntryStatus::Paused => TaskLaneStatus::Paused,
            };
            lane.set_status(lane_status);
        }

        // Emit TaskUpdated event
        self.bus.publish(SystemEvent::TaskUpdated {
            task_id: task_id.to_string(),
            status: new_status.as_str().to_string(),
            progress_current: None,
            progress_total: None,
            timestamp: Utc::now(),
        });

        Ok(serde_json::json!({
            "task_id": task_id,
            "action": action,
            "new_status": new_status.as_str(),
        })
        .to_string())
    }
}
