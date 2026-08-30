//! Workflow-context block (Routing V2) — renders the lane's active
//! workflows as a compact prompt block for the workflow-aware main loop.
//!
//! Injected PER-TURN into the composed message list, deliberately NOT through
//! the compose engine's layers: Layer 3's fingerprint is keyed on query text
//! (the `memory_retrieval_hash` is zeroed on this path), so a repeated query
//! would serve a stale workflow status from the per-lane Tier-2 cache, and a
//! Layer 2 raw-block would thrash the global Tier-1 LRU on every status
//! change. See `handle_simple_query` for the injection point.

use crate::context::SharedContext;
use openalpaca_storage::Database;
use openalpaca_storage::repository::TaskRepository;

/// Render the lane's active workflows as a `<active_workflows>` prompt
/// block. Returns `None` when the lane has none (the common case). Only the
/// tool-mode main loop calls this, so legacy-path prompts are untouched even
/// though lane registration itself is unconditional.
///
/// Per workflow: task id, title, status (in-memory registry first, DB
/// fallback for post-restart entries), and the registry's progress counters
/// when present (the cheap "last progress line").
pub(crate) fn render_workflow_context_block(
    shared_context: &SharedContext,
    db: Option<&Database>,
    lane_key: &str,
) -> Option<String> {
    let task_ids = shared_context.workflows_for_lane(lane_key);
    if task_ids.is_empty() {
        return None;
    }

    let mut block = String::from(
        "<active_workflows>\nBackground workflows currently running on this conversation:\n",
    );
    for task_id in &task_ids {
        let line = match shared_context.task_registry.get(task_id) {
            Some(entry) => {
                let progress = match (entry.progress_current, entry.progress_total) {
                    (Some(cur), Some(total)) => format!(", progress {cur}/{total}"),
                    (Some(cur), None) => format!(", progress {cur}"),
                    _ => String::new(),
                };
                format!(
                    "- {} — \"{}\" ({}{})\n",
                    task_id,
                    entry.title,
                    entry.status.as_str(),
                    progress,
                )
            }
            None => {
                // Registry miss (e.g. post-restart) — one cheap DB read.
                let db_row = db.and_then(|db| TaskRepository::new(db).get(task_id).ok().flatten());
                match db_row {
                    Some(task) => format!(
                        "- {} — \"{}\" ({})\n",
                        task_id,
                        task.title,
                        task.status.as_str(),
                    ),
                    None => format!("- {} (status unknown)\n", task_id),
                }
            }
        };
        block.push_str(&line);
    }
    block.push_str(
        "If the user's message is a correction or new guidance for one of these, call \
         steer_workflow with its task id. Use queue_followup to queue work for after one \
         finishes, and task_status for details. Answer unrelated messages normally.\n\
         </active_workflows>",
    );
    Some(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::TaskEntryStatus;

    #[test]
    fn test_no_workflows_renders_nothing() {
        let ctx = SharedContext::new();
        assert_eq!(render_workflow_context_block(&ctx, None, "user1:cli"), None);
    }

    #[test]
    fn test_one_workflow_renders_id_title_status_progress() {
        let ctx = SharedContext::new();
        ctx.task_registry
            .register("task-1".to_string(), "Research task".to_string());
        ctx.task_registry
            .update_status("task-1", TaskEntryStatus::Running);
        ctx.task_registry.update_progress("task-1", 2, 5, None);
        ctx.register_workflow_for_lane("user1:cli", "task-1");

        let block = render_workflow_context_block(&ctx, None, "user1:cli")
            .expect("block should render for an active workflow");
        assert!(block.starts_with("<active_workflows>"), "{block}");
        assert!(block.ends_with("</active_workflows>"), "{block}");
        assert!(block.contains("task-1"), "{block}");
        assert!(block.contains("\"Research task\""), "{block}");
        assert!(block.contains("running"), "{block}");
        assert!(block.contains("progress 2/5"), "{block}");
        assert!(block.contains("steer_workflow"), "{block}");
        assert!(block.contains("queue_followup"), "{block}");

        // Other lanes see nothing.
        let other = render_workflow_context_block(&ctx, None, "user2:telegram");
        assert_eq!(other, None);
    }

    #[test]
    fn test_two_workflows_render_both_lines() {
        let ctx = SharedContext::new();
        ctx.task_registry
            .register("task-a".to_string(), "First".to_string());
        ctx.task_registry
            .register("task-b".to_string(), "Second".to_string());
        ctx.register_workflow_for_lane("user1:cli", "task-a");
        ctx.register_workflow_for_lane("user1:cli", "task-b");

        let block = render_workflow_context_block(&ctx, None, "user1:cli").unwrap();
        assert!(block.contains("task-a"), "{block}");
        assert!(block.contains("\"First\""), "{block}");
        assert!(block.contains("task-b"), "{block}");
        assert!(block.contains("\"Second\""), "{block}");
    }

    #[test]
    fn test_registry_miss_falls_back_to_db_then_unknown() {
        let ctx = SharedContext::new();
        ctx.register_workflow_for_lane("user1:cli", "ghost-task");

        // No DB → status unknown, still listed.
        let block = render_workflow_context_block(&ctx, None, "user1:cli").unwrap();
        assert!(block.contains("ghost-task (status unknown)"), "{block}");
    }
}
