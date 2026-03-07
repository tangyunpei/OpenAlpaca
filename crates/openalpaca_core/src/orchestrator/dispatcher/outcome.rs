//! Task outcome construction, terminal consistency checks, and finalization.

use crate::orchestrator::task_state::{TaskOutcome, TaskState};
use openalpaca_storage::OutcomeKind;

/// Maximum length for the summary stored in `result_summary` column.
pub(super) const MAX_SUMMARY_LENGTH: usize = 2000;

/// Persist a state update with retry (up to 3 attempts) to handle optimistic locking conflicts.
///
/// Returns `true` if the update was successfully persisted, `false` if all retries were
/// exhausted or a DB error occurred.
pub(super) async fn update_state_with_retry(
    db: &openalpaca_storage::Database,
    task_id: &str,
    mutate: impl Fn(&mut TaskState),
    context: &str,
) -> bool {
    const MAX_RETRIES: usize = 3;
    for attempt in 0..MAX_RETRIES {
        let repo = openalpaca_storage::repository::TaskRepository::new(db);
        let existing = match repo.get(task_id) {
            Ok(Some(t)) => t,
            _ => return false,
        };
        let sj = match existing.state_json.as_deref() {
            Some(s) => s,
            None => return false,
        };
        let mut state: TaskState = match serde_json::from_str(sj) {
            Ok(s) => s,
            Err(_) => return false,
        };
        mutate(&mut state);
        match repo.update_state(task_id, &state.to_json(), existing.state_version) {
            Ok(true) => return true,
            Ok(false) => {
                if attempt < MAX_RETRIES - 1 {
                    tracing::debug!(
                        "State update version conflict ({}) for task '{}' (attempt {}/{}), retrying",
                        context,
                        task_id,
                        attempt + 1,
                        MAX_RETRIES
                    );
                    // Linear backoff with pseudo-jitter (avoids rand dependency).
                    // Jitter from task_id hash reduces contention under DAG parallelism.
                    let pseudo_jitter = task_id.as_bytes().iter().map(|b| *b as u64).sum::<u64>() % 5;
                    tokio::time::sleep(std::time::Duration::from_millis(10 + (attempt as u64 * 5) + pseudo_jitter)).await;
                } else {
                    tracing::warn!(
                        "State update ({}) for task '{}' failed after {} retries — state may be stale",
                        context,
                        task_id,
                        MAX_RETRIES
                    );
                    return false;
                }
            }
            Err(e) => {
                tracing::warn!(
                    "State update ({}) failed for task '{}': {}",
                    context,
                    task_id,
                    e
                );
                return false;
            }
        }
    }
    false
}

/// Build a structured TaskOutcome from the current task state.
///
/// Reads the task's state_json from the DB (if available), uses it to collect
/// step summaries and artifact pointers, then classifies the outcome.
///
/// If state_json is unavailable (lead agent with no state, legacy tasks),
/// falls back to constructing a minimal outcome from the provided content.
pub(super) fn build_task_outcome(
    db: Option<&openalpaca_storage::Database>,
    task_id: &str,
    final_content: &str,
    success: bool,
) -> TaskOutcome {
    // Try to read state_json for rich outcome data
    if let Some(db) = db {
        let repo = openalpaca_storage::repository::TaskRepository::new(db);
        if let Ok(Some(task)) = repo.get(task_id) {
            if let Some(ref sj) = task.state_json {
                if let Ok(state) = serde_json::from_str::<TaskState>(sj) {
                    let fallback = if final_content.is_empty() {
                        if success { "Task completed." } else { "Task failed." }
                    } else {
                        final_content
                    };
                    let mut outcome = if state.dag.is_some() {
                        state.build_outcome_dag(fallback, None)
                    } else {
                        state.build_outcome(fallback, None)
                    };
                    if !success {
                        outcome.outcome_kind = OutcomeKind::Failed;
                        // Prepend error reason if it's not already in the summary
                        if !final_content.is_empty() && !outcome.summary.contains(final_content) {
                            outcome.summary =
                                format!("{}\n\n{}", final_content, outcome.summary);
                        }
                    }
                    return outcome;
                }
            }
        }
    }

    // Fallback: no state_json available, build minimal outcome from content
    let summary = if final_content.is_empty() {
        if success { "Task completed.".to_string() } else { "Task failed.".to_string() }
    } else {
        final_content.to_string()
    };

    if success {
        TaskOutcome {
            summary,
            outcome_kind: OutcomeKind::TextOnly,
            artifacts: Vec::new(),
            no_artifact_reason: Some("No artifacts were produced.".to_string()),
        }
    } else {
        TaskOutcome {
            summary,
            outcome_kind: OutcomeKind::Failed,
            artifacts: Vec::new(),
            no_artifact_reason: None,
        }
    }
}

/// Log warnings for inconsistent terminal task states.
///
/// This is observability-only (never blocks or fails). It catches cases like:
/// - Empty outcome summary at terminal time
/// - success=true but outcome_kind=Failed (or vice versa)
fn check_terminal_consistency(task_id: &str, success: bool, outcome: &TaskOutcome) {
    if outcome.summary.is_empty() {
        tracing::warn!(
            task_id,
            success,
            outcome_kind = %outcome.outcome_kind.as_str(),
            "Terminal task has empty outcome summary"
        );
    }
    if success && outcome.outcome_kind == OutcomeKind::Failed {
        tracing::warn!(
            task_id,
            "Task marked successful but outcome_kind=Failed — inconsistent state"
        );
    }
    if !success && outcome.outcome_kind != OutcomeKind::Failed {
        tracing::warn!(
            task_id,
            outcome_kind = %outcome.outcome_kind.as_str(),
            "Task marked failed but outcome_kind is not Failed — inconsistent state"
        );
    }
}

/// Finalize a task with a structured outcome.
///
/// This is the unified replacement for the ad-hoc assembly in each execution mode.
/// It:
/// 1. Builds the TaskOutcome (via `build_task_outcome`)
/// 2. Checks terminal consistency (log-only warnings)
/// 3. Persists the outcome to DB (outcome_json, outcome_kind, artifact_count)
/// 4. Delegates to `finalize_task` for status update, `result_summary`, and event emission
pub(super) fn finalize_task_with_outcome(
    ctx: &crate::context::SharedContext,
    bus: &crate::bus::EventBus,
    db: Option<&openalpaca_storage::Database>,
    task_id: &str,
    final_content: &str,
    success: bool,
) -> TaskOutcome {
    let outcome = build_task_outcome(db, task_id, final_content, success);

    // Observability: log warnings for inconsistent terminal states
    check_terminal_consistency(task_id, success, &outcome);

    // Persist structured outcome fields to DB (outcome_json, outcome_kind, artifact_count)
    if let Some(db) = db {
        let repo = openalpaca_storage::repository::TaskRepository::new(db);
        let outcome_json = match serde_json::to_string(&outcome) {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!(
                    "finalize_task_with_outcome: failed to serialize outcome for task '{}': {e}",
                    task_id
                );
                // Skip DB write — don't persist invalid/empty JSON
                String::new()
            }
        };
        if outcome_json.is_empty() {
            tracing::warn!(
                "finalize_task_with_outcome: skipping set_outcome for task '{}' due to empty outcome_json",
                task_id
            );
        } else if let Err(e) = repo.set_outcome(
            task_id,
            &outcome_json,
            outcome.outcome_kind,
            outcome.artifacts.len() as i32,
        ) {
            tracing::warn!(
                "finalize_task_with_outcome: failed to set outcome for task '{}': {e}",
                task_id
            );
        }
    }

    // Delegate status update + result_summary + event emission to existing finalize_task
    let truncated_summary: String = outcome.summary.chars().take(MAX_SUMMARY_LENGTH).collect();
    finalize_task(
        ctx,
        bus,
        db,
        task_id,
        &truncated_summary,
        success,
        Some(outcome.outcome_kind),
        Some(outcome.artifacts.len() as i32),
        Some(&outcome.summary),
    );

    outcome
}

/// Update task status in registry + DB + emit event for a completed or failed task.
#[allow(clippy::too_many_arguments)]
pub(super) fn finalize_task(
    ctx: &crate::context::SharedContext,
    bus: &crate::bus::EventBus,
    db: Option<&openalpaca_storage::Database>,
    task_id: &str,
    summary: &str,
    success: bool,
    outcome_kind: Option<OutcomeKind>,
    artifact_count: Option<i32>,
    outcome_summary: Option<&str>,
) {
    let now = chrono::Utc::now();
    if success {
        ctx.task_registry
            .update_status(task_id, crate::context::TaskEntryStatus::Completed);
        if let Some(db) = db {
            let repo = openalpaca_storage::repository::TaskRepository::new(db);
            if let Err(e) = repo.update_status(task_id, openalpaca_storage::TaskStatus::Completed) {
                tracing::warn!(
                    "finalize_task: failed to update status for task '{}': {e}",
                    task_id
                );
            }
            if let Err(e) = repo.set_result(task_id, summary) {
                tracing::warn!(
                    "finalize_task: failed to set result for task '{}': {e}",
                    task_id
                );
            }
        }
        bus.publish(crate::events::SystemEvent::TaskCompleted {
            task_id: task_id.to_string(),
            result_summary: Some(summary.to_string()),
            outcome_kind: outcome_kind.map(|k| k.as_str().to_string()),
            artifact_count,
            outcome_summary: outcome_summary.map(|s| s.chars().take(500).collect()),
            timestamp: now,
        });
    } else {
        ctx.task_registry
            .update_status(task_id, crate::context::TaskEntryStatus::Failed);
        if let Some(db) = db {
            let repo = openalpaca_storage::repository::TaskRepository::new(db);
            if let Err(e) = repo.update_status(task_id, openalpaca_storage::TaskStatus::Failed) {
                tracing::warn!(
                    "finalize_task: failed to update status for task '{}': {e}",
                    task_id
                );
            }
            if let Err(e) = repo.set_result(task_id, summary) {
                tracing::warn!(
                    "finalize_task: failed to set result for task '{}': {e}",
                    task_id
                );
            }
        }
        bus.publish(crate::events::SystemEvent::TaskFailed {
            task_id: task_id.to_string(),
            error: summary.to_string(),
            outcome_kind: outcome_kind.map(|k| k.as_str().to_string()),
            timestamp: now,
        });
    }
}

/// Persist a task result as a conversation message.
#[allow(clippy::too_many_arguments)]
pub(super) fn persist_conversation(
    db: &openalpaca_storage::Database,
    lane_key: &str,
    source: &str,
    content: String,
    model: Option<String>,
    tokens_in: i64,
    tokens_out: i64,
    runtime_secs: i64,
) {
    let conv_repo = openalpaca_storage::ConversationRepository::new(db);
    if let Err(e) = conv_repo.get_or_create_conversation(lane_key, source) {
        tracing::warn!(
            "persist_conversation: failed to get/create conversation for lane '{}': {e}",
            lane_key
        );
        return;
    }

    let msg = openalpaca_storage::ConversationMessage {
        id: 0,
        lane_key: lane_key.to_string(),
        role: "assistant".to_string(),
        content,
        source: Some(source.to_string()),
        model,
        tokens_in: Some(tokens_in),
        tokens_out: Some(tokens_out),
        duration_ms: Some(runtime_secs * 1000),
        created_at: String::new(),
        content_json: None,
        display_text: None,
    };

    match conv_repo.insert(&msg) {
        Ok(_) => {
            if let Err(e) = conv_repo.increment_message_count(lane_key) {
                tracing::warn!(
                    "persist_conversation: failed to increment message count for lane '{}': {e}",
                    lane_key
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                "persist_conversation: failed to insert assistant message for lane '{}': {e}",
                lane_key
            );
        }
    }
}
