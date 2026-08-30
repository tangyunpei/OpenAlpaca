//! Memory retrieval and task memory extraction helpers for dispatcher prompts.

use crate::daemon_config::DaemonConfig;
use crate::memory::task_extraction::{TaskExtractionParams, extract_task_memories};
use arc_swap::ArcSwap;
use openalpaca_llm::LlmRouter;
use openalpaca_storage::Database;
use std::sync::Arc;

/// Spawn a background task to extract memories from a completed task output.
/// Fire-and-forget: does not block the caller. Only runs for successful tasks.
#[allow(clippy::too_many_arguments)]
pub(in crate::orchestrator) fn spawn_task_memory_extraction(
    db: &Database,
    router: &Arc<LlmRouter>,
    embedder: &Option<Arc<dyn openalpaca_llm::Embedder>>,
    daemon_config: &Arc<ArcSwap<DaemonConfig>>,
    owner_id: &str,
    task_id: &str,
    task_description: &str,
    task_output: &str,
    source_path: &str,
    success: bool,
    workspace_id: Option<String>,
) {
    if !success {
        return;
    }
    let dcfg = daemon_config.load();
    if !dcfg.orchestrator.costs.task_extract_enabled {
        return;
    }

    let params = TaskExtractionParams {
        owner_id: owner_id.to_string(),
        task_id: task_id.to_string(),
        task_description: task_description.to_string(),
        task_output: task_output.to_string(),
        source_path: source_path.to_string(),
        workspace_id,
    };
    let db = db.clone();
    let router = router.clone();
    let embedder = embedder.clone();
    let daemon_config = daemon_config.clone();

    let task_id_for_log = params.task_id.clone();
    let handle = tokio::spawn(async move {
        extract_task_memories(params, db, router, embedder, daemon_config).await;
    });
    // Separate lightweight task to catch panics from the extraction task
    tokio::spawn(async move {
        if let Err(e) = handle.await {
            tracing::warn!(
                "Fire-and-forget memory extraction failed for task '{}': {e}",
                task_id_for_log,
            );
        }
    });
}
