use super::Orchestrator;
use super::{db_task_to_json, parse_outcome, task_entry_to_json};
use crate::bus::EventBus;
use crate::context::{SharedContext, TaskEntryStatus};
use crate::events::SystemEvent;
use crate::lane::{LaneManager, TaskLaneStatus};
use chrono::Utc;
use openalpaca_storage::repository::TaskRepository;
use openalpaca_storage::{Database, TaskStatus};

/// Error from [`apply_task_action`], carrying enough detail for each caller
/// to format its own response (chat message vs. HTTP status + body).
#[derive(Debug)]
pub enum TaskActionError {
    /// Task exists in neither the in-memory registry nor the database.
    NotFound,
    /// Cancel requested on a task already in a terminal state.
    CannotCancel { current: &'static str },
    /// Pause requested on a task that isn't running.
    CannotPause { current: &'static str },
    /// Resume requested on a task that isn't paused.
    CannotResume { current: &'static str },
    /// Action is not one of cancel / pause / resume.
    UnknownAction,
    /// Database read/write failure.
    Db(String),
}

fn entry_status_from_db(status: TaskStatus) -> TaskEntryStatus {
    match status {
        TaskStatus::Queued => TaskEntryStatus::Queued,
        TaskStatus::Running => TaskEntryStatus::Running,
        TaskStatus::Completed => TaskEntryStatus::Completed,
        TaskStatus::Failed => TaskEntryStatus::Failed,
        TaskStatus::Cancelled => TaskEntryStatus::Cancelled,
        TaskStatus::Paused => TaskEntryStatus::Paused,
    }
}

fn db_status_from_entry(status: TaskEntryStatus) -> TaskStatus {
    match status {
        TaskEntryStatus::Queued => TaskStatus::Queued,
        TaskEntryStatus::Running => TaskStatus::Running,
        TaskEntryStatus::Completed => TaskStatus::Completed,
        TaskEntryStatus::Failed => TaskStatus::Failed,
        TaskEntryStatus::Cancelled => TaskStatus::Cancelled,
        TaskEntryStatus::Paused => TaskStatus::Paused,
    }
}

/// Apply a task-control action (`cancel` / `pause` / `resume`), shared by the
/// orchestrator chat handler and the daemon HTTP route.
///
/// The task is resolved from the in-memory registry first, falling back to the
/// database, so tasks that only exist in SQLite (e.g. after a daemon restart)
/// remain controllable. On success this fires the task's CancellationToken
/// (cancel only), persists the status change when a DB row exists, syncs the
/// registry entry and task lane, publishes [`SystemEvent::TaskUpdated`], and
/// returns the new status.
pub fn apply_task_action(
    shared_context: &SharedContext,
    lane_manager: &LaneManager,
    bus: &EventBus,
    db: Option<&Database>,
    task_id: &str,
    action: &str,
) -> Result<TaskEntryStatus, TaskActionError> {
    // Resolve current state: in-memory registry first, DB fallback.
    let current = match shared_context.task_registry.get(task_id) {
        Some(entry) => entry.status,
        None => match db {
            Some(db) => {
                let task = TaskRepository::new(db)
                    .get(task_id)
                    .map_err(|e| TaskActionError::Db(e.to_string()))?
                    .ok_or(TaskActionError::NotFound)?;
                entry_status_from_db(task.status)
            }
            None => return Err(TaskActionError::NotFound),
        },
    };

    // Validate state transition
    let new_status = match action {
        "cancel" => {
            if current.is_terminal() {
                return Err(TaskActionError::CannotCancel {
                    current: current.as_str(),
                });
            }
            TaskEntryStatus::Cancelled
        }
        "pause" => {
            if current != TaskEntryStatus::Running {
                return Err(TaskActionError::CannotPause {
                    current: current.as_str(),
                });
            }
            TaskEntryStatus::Paused
        }
        "resume" => {
            if current != TaskEntryStatus::Paused {
                return Err(TaskActionError::CannotResume {
                    current: current.as_str(),
                });
            }
            TaskEntryStatus::Running
        }
        _ => return Err(TaskActionError::UnknownAction),
    };

    // Trigger the CancellationToken so background execution tasks actually stop.
    // Without this, the tokio tasks (DAG, pipeline, lead agent) continue running
    // because they only check token.is_cancelled() in their event loops.
    if new_status == TaskEntryStatus::Cancelled {
        let cancelled = shared_context.cancel_task(task_id);
        if cancelled {
            tracing::info!("Triggered cancellation token for task '{}'", task_id);
        } else {
            tracing::warn!(
                "No cancellation token found for task '{}' — task may have already finished",
                task_id
            );
        }
    }

    // Persist when a DB row exists (update_status is a no-op otherwise).
    if let Some(db) = db {
        TaskRepository::new(db)
            .update_status(task_id, db_status_from_entry(new_status))
            .map_err(|e| TaskActionError::Db(e.to_string()))?;
    }

    // Sync in-memory registry (no-op for DB-only tasks).
    shared_context
        .task_registry
        .update_status(task_id, new_status);

    // Update task lane if present
    if let Some(lane) = lane_manager.get_task_lane(task_id) {
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
    bus.publish(SystemEvent::TaskUpdated {
        task_id: task_id.to_string(),
        status: new_status.as_str().to_string(),
        progress_current: None,
        progress_total: None,
        timestamp: Utc::now(),
    });

    Ok(new_status)
}

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
                                let (outcome_summary, no_artifact_reason, artifacts) =
                                    parse_outcome(t)
                                        .map(|p| {
                                            (
                                                p.outcome_summary,
                                                p.no_artifact_reason,
                                                serde_json::json!(p.artifacts),
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
        let new_status = apply_task_action(
            &self.shared_context,
            &self.lane_manager,
            &self.bus,
            self.db.as_ref(),
            task_id,
            action,
        )
        .map_err(|e| match e {
            TaskActionError::NotFound => format!("Task '{}' not found", task_id),
            TaskActionError::CannotCancel { current } => {
                format!("Cannot cancel task in '{}' state", current)
            }
            TaskActionError::CannotPause { current } => {
                format!("Can only pause a running task, current: '{}'", current)
            }
            TaskActionError::CannotResume { current } => {
                format!("Can only resume a paused task, current: '{}'", current)
            }
            TaskActionError::UnknownAction => format!("Unknown action: '{}'", action),
            TaskActionError::Db(e) => format!("Database error: {}", e),
        })?;

        Ok(serde_json::json!({
            "task_id": task_id,
            "action": action,
            "new_status": new_status.as_str(),
        })
        .to_string())
    }

    /// Handle the deterministic `/steer <message>` prefix (Routing V2):
    /// guaranteed injection into the lane's sole running workflow, bypassing
    /// the model. Replies are deterministic; the reply is always `Ok` — a
    /// missing or ambiguous target is an answer, not a handler failure.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn handle_steer_prefix(
        &self,
        request_id: uuid::Uuid,
        text: &str,
        principal: &crate::security::policy::Principal,
        scope: &crate::security::policy::Scope,
        lane_key: &str,
        workspace_path: Option<String>,
    ) -> Result<String, String> {
        use crate::runner::steering::{SteeringMsg, SteeringPushError, push_steering};

        let text = text.trim();
        if text.is_empty() {
            return Ok("Usage: /steer <message> — inject a message into the running workflow.".to_string());
        }

        let workflows = self.shared_context.workflows_for_lane(lane_key);
        match workflows.as_slice() {
            [] => Ok("No running workflow on this conversation.".to_string()),
            [task_id] => {
                let title = self
                    .shared_context
                    .task_registry
                    .get(task_id)
                    .map(|e| e.title)
                    .unwrap_or_else(|| task_id.clone());
                let msg = SteeringMsg {
                    text: text.to_string(),
                    request_id,
                    principal: principal.clone(),
                    scope: scope.clone(),
                    workspace_path,
                    received_at: Utc::now(),
                };
                match push_steering(&self.shared_context, &self.bus, task_id, lane_key, msg) {
                    Ok(depth) => Ok(format!(
                        "Steering message queued for \"{}\" ({}). {} message{} waiting — the workflow picks it up at its next round.",
                        title,
                        task_id,
                        depth,
                        if depth == 1 { "" } else { "s" },
                    )),
                    Err(SteeringPushError::Full) => Ok(format!(
                        "The steering queue for \"{}\" ({}) is full — the workflow hasn't caught up with earlier messages yet. Wait for it to process the backlog, or use /cancel {} to stop it.",
                        title, task_id, task_id,
                    )),
                    Err(SteeringPushError::Closed) => {
                        Ok("No running workflow on this conversation.".to_string())
                    }
                }
            }
            many => {
                let list: Vec<String> = many
                    .iter()
                    .map(|id| {
                        let title = self
                            .shared_context
                            .task_registry
                            .get(id)
                            .map(|e| e.title)
                            .unwrap_or_default();
                        format!("- {} ({})", id, title)
                    })
                    .collect();
                Ok(format!(
                    "Multiple workflows are running on this conversation:\n{}\nWhich task should receive this message? Nothing was queued.",
                    list.join("\n"),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_storage::Task;

    fn make_db_task(id: &str, status: TaskStatus) -> Task {
        let now = Utc::now();
        Task {
            id: id.to_string(),
            title: "Test task".to_string(),
            description: None,
            status,
            priority: 0,
            progress_current: None,
            progress_total: None,
            result_summary: None,
            created_by: "user-1".to_string(),
            source_lane: "lane-1".to_string(),
            created_at: now,
            updated_at: now,
            completed_at: None,
            state_json: None,
            state_version: 0,
            outcome_json: None,
            outcome_kind: None,
            artifact_count: 0,
        }
    }

    fn make_env() -> (SharedContext, LaneManager, EventBus) {
        (SharedContext::new(), LaneManager::new(), EventBus::default())
    }

    #[test]
    fn cancel_db_only_task_persists_and_emits() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        let repo = TaskRepository::new(&db);
        repo.create(&make_db_task("t1", TaskStatus::Running)).unwrap();

        let (ctx, lanes, bus) = make_env();
        let mut rx = bus.subscribe();

        // No registry entry — resolution must fall back to the DB.
        let new_status =
            apply_task_action(&ctx, &lanes, &bus, Some(&db), "t1", "cancel").unwrap();
        assert_eq!(new_status, TaskEntryStatus::Cancelled);

        let task = repo.get("t1").unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Cancelled);

        match rx.try_recv().unwrap() {
            SystemEvent::TaskUpdated {
                task_id, status, ..
            } => {
                assert_eq!(task_id, "t1");
                assert_eq!(status, "cancelled");
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn invalid_transition_on_db_only_task() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        let repo = TaskRepository::new(&db);
        repo.create(&make_db_task("t1", TaskStatus::Completed))
            .unwrap();
        repo.create(&make_db_task("t2", TaskStatus::Queued)).unwrap();

        let (ctx, lanes, bus) = make_env();

        // Cancel a terminal task → rejected, DB untouched.
        match apply_task_action(&ctx, &lanes, &bus, Some(&db), "t1", "cancel") {
            Err(TaskActionError::CannotCancel { current }) => assert_eq!(current, "completed"),
            other => panic!("expected CannotCancel, got: {:?}", other),
        }
        assert_eq!(
            repo.get("t1").unwrap().unwrap().status,
            TaskStatus::Completed
        );

        // Pause a queued task → rejected.
        match apply_task_action(&ctx, &lanes, &bus, Some(&db), "t2", "pause") {
            Err(TaskActionError::CannotPause { current }) => assert_eq!(current, "queued"),
            other => panic!("expected CannotPause, got: {:?}", other),
        }
    }

    #[test]
    fn resume_db_only_paused_task() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        let repo = TaskRepository::new(&db);
        repo.create(&make_db_task("t1", TaskStatus::Paused)).unwrap();

        let (ctx, lanes, bus) = make_env();
        let new_status =
            apply_task_action(&ctx, &lanes, &bus, Some(&db), "t1", "resume").unwrap();
        assert_eq!(new_status, TaskEntryStatus::Running);
        assert_eq!(repo.get("t1").unwrap().unwrap().status, TaskStatus::Running);
    }

    #[test]
    fn unknown_task_and_unknown_action() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();

        let (ctx, lanes, bus) = make_env();
        assert!(matches!(
            apply_task_action(&ctx, &lanes, &bus, Some(&db), "nope", "cancel"),
            Err(TaskActionError::NotFound)
        ));
        assert!(matches!(
            apply_task_action(&ctx, &lanes, &bus, None, "nope", "cancel"),
            Err(TaskActionError::NotFound)
        ));

        ctx.task_registry
            .register("t1".to_string(), "Test task".to_string());
        assert!(matches!(
            apply_task_action(&ctx, &lanes, &bus, Some(&db), "t1", "explode"),
            Err(TaskActionError::UnknownAction)
        ));
    }

    #[test]
    fn registry_task_without_db_row_still_cancels() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();

        let (ctx, lanes, bus) = make_env();
        ctx.task_registry
            .register("t1".to_string(), "Test task".to_string());

        let new_status =
            apply_task_action(&ctx, &lanes, &bus, Some(&db), "t1", "cancel").unwrap();
        assert_eq!(new_status, TaskEntryStatus::Cancelled);
        assert_eq!(
            ctx.task_registry.get("t1").unwrap().status,
            TaskEntryStatus::Cancelled
        );
    }
}
