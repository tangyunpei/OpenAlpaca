//! State persistence, workspace updates, and node completion handling.

use super::*;

/// Load the full TaskState from the DB — used by Opt-7c to pre-load a shared
/// workspace snapshot once per batch instead of once per concurrent node.
pub(super) fn load_task_state(task_id: &str, db: &Option<Database>) -> Option<TaskState> {
    let db = db.as_ref()?;
    let repo = openalpaca_storage::repository::TaskRepository::new(db);
    let task = repo.get(task_id).ok()??;
    task.state_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
}

/// Load workspace context filtered by the node's workspace_keys.
pub(super) fn load_workspace_context(
    task_id: &str,
    db: &Option<Database>,
    workspace_keys: &[String],
) -> String {
    let Some(db) = db else {
        return String::new();
    };

    let repo = openalpaca_storage::repository::TaskRepository::new(db);
    let task = match repo.get(task_id) {
        Ok(Some(t)) => t,
        _ => return String::new(),
    };

    let state: TaskState = match task.state_json.as_deref() {
        Some(json) => match serde_json::from_str(json) {
            Ok(s) => s,
            Err(_) => return String::new(),
        },
        None => return String::new(),
    };

    state.workspace.format_for_prompt(workspace_keys)
}

/// Write a completed node's output to the workspace under its output_key.
/// Uses a retry loop (max 3 attempts) to handle optimistic locking conflicts
/// when concurrent nodes complete simultaneously.
pub(super) async fn write_node_output_to_workspace(
    dag: &TaskDag,
    node_result: &NodeResult,
    task_id: &str,
    db: &Option<Database>,
) {
    let Some(db) = db else { return };

    // Find the node's output_key
    let output_key = dag
        .nodes
        .iter()
        .find(|n| n.node_id == node_result.node_id)
        .and_then(|n| n.output_key.as_deref());

    let Some(key) = output_key else { return };
    if node_result.final_content.is_empty() {
        return;
    }

    const MAX_RETRIES: usize = 3;
    for attempt in 0..MAX_RETRIES {
        let repo = openalpaca_storage::repository::TaskRepository::new(db);
        let existing = match repo.get(task_id) {
            Ok(Some(t)) => t,
            _ => return,
        };
        let sj = match &existing.state_json {
            Some(s) => s,
            None => return,
        };
        let mut state: TaskState = match serde_json::from_str(sj) {
            Ok(s) => s,
            Err(_) => return,
        };

        // Collect workspace_keys from nodes that haven't completed yet
        let protected_keys: Vec<String> = dag
            .nodes
            .iter()
            .filter(|n| {
                matches!(
                    n.status,
                    DagNodeStatus::Pending | DagNodeStatus::Ready | DagNodeStatus::Running
                )
            })
            .flat_map(|n| n.workspace_keys.iter().cloned())
            .collect();

        if let Err(e) = state.workspace.write(
            key,
            &node_result.final_content,
            &node_result.agent_id,
            WorkspaceEntryType::Context,
            &protected_keys,
        ) {
            tracing::warn!(
                "Failed to write workspace entry '{}' for node '{}': {}",
                key,
                node_result.node_id,
                e
            );
            return;
        }

        match repo.update_state(task_id, &state.to_json(), existing.state_version) {
            Ok(true) => return, // success
            Ok(false) => {
                if attempt < MAX_RETRIES - 1 {
                    tracing::debug!(
                        "Workspace write version conflict for key '{}' node '{}' (attempt {}/{}), retrying",
                        key,
                        node_result.node_id,
                        attempt + 1,
                        MAX_RETRIES
                    );
                    // Brief async backoff to reduce collision probability
                    tokio::time::sleep(std::time::Duration::from_millis(10 * (1 << attempt))).await;
                } else {
                    tracing::warn!(
                        "Workspace write for key '{}' node '{}' failed after {} retries — data may be lost",
                        key,
                        node_result.node_id,
                        MAX_RETRIES
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to persist workspace entry '{}' for node '{}': {}",
                    key,
                    node_result.node_id,
                    e
                );
                return;
            }
        }
    }
}

/// Load a snapshot of the workspace from the task's persisted state.
pub(super) fn load_workspace_snapshot(task_id: &str, db: &Option<Database>) -> TaskWorkspace {
    let Some(db) = db else {
        return TaskWorkspace::default();
    };

    let repo = openalpaca_storage::repository::TaskRepository::new(db);
    let task = match repo.get(task_id) {
        Ok(Some(t)) => t,
        _ => return TaskWorkspace::default(),
    };

    match task.state_json.as_deref() {
        Some(json) => match serde_json::from_str::<TaskState>(json) {
            Ok(s) => s.workspace,
            Err(_) => TaskWorkspace::default(),
        },
        None => TaskWorkspace::default(),
    }
}

/// Persist the current DAG state back to the task's state_json.
/// Uses a retry loop (max 3 attempts) to handle optimistic locking conflicts
/// when concurrent nodes trigger DAG state updates simultaneously.
pub(super) async fn persist_dag_state(dag: &TaskDag, task_id: &str, db: &Option<Database>) {
    let Some(db) = db else { return };

    const MAX_RETRIES: usize = 3;
    for attempt in 0..MAX_RETRIES {
        let repo = openalpaca_storage::repository::TaskRepository::new(db);
        let existing = match repo.get(task_id) {
            Ok(Some(t)) => t,
            _ => return,
        };
        let sj = match &existing.state_json {
            Some(s) => s,
            None => return,
        };
        let mut state: TaskState = match serde_json::from_str(sj) {
            Ok(s) => s,
            Err(_) => return,
        };

        state.dag = Some(dag.clone());

        match repo.update_state(task_id, &state.to_json(), existing.state_version) {
            Ok(true) => return, // success
            Ok(false) => {
                if attempt < MAX_RETRIES - 1 {
                    tracing::debug!(
                        "DAG state persist version conflict for task '{}' (attempt {}/{}), retrying",
                        task_id,
                        attempt + 1,
                        MAX_RETRIES
                    );
                    // Brief async backoff to reduce collision probability
                    tokio::time::sleep(Duration::from_millis(10 * (1 << attempt))).await;
                } else {
                    tracing::warn!(
                        "DAG state persist for task '{}' failed after {} retries — state may be stale",
                        task_id,
                        MAX_RETRIES
                    );
                }
            }
            Err(e) => {
                tracing::warn!("Failed to persist DAG state for task '{}': {}", task_id, e);
                return;
            }
        }
    }
}
