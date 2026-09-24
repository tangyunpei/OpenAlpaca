//! Task-output memory extraction, started by the dispatcher when a run
//! finishes (design §6, MD-2; plan 3.4).

use crate::daemon_config::DaemonConfig;
use crate::memory::service::MemoryService;
use crate::memory::task_extraction::{
    TaskExtractionDeps, TaskExtractionParams, extract_task_memories,
};
use crate::orchestrator::BackgroundJobs;
use arc_swap::ArcSwap;
use openalpaca_llm::LlmRouter;
use openalpaca_storage::Database;
use std::path::PathBuf;
use std::sync::Arc;

/// Starts a finished run's extraction as a tracked job on `background`:
/// cancellable at shutdown, attributed to the run, never fire-and-forget.
/// A failed or cancelled run teaches nothing, so nothing is started for
/// one; nor without a memory service (a daemon with no store).
#[allow(clippy::too_many_arguments)]
pub(in crate::orchestrator) fn spawn_task_memory_extraction(
    background: &BackgroundJobs,
    memory_service: Option<Arc<MemoryService>>,
    db: &Database,
    router: &Arc<LlmRouter>,
    daemon_config: &Arc<ArcSwap<DaemonConfig>>,
    owner_id: &str,
    task_id: &str,
    task_description: &str,
    task_output: &str,
    source_path: &str,
    success: bool,
    project_root: Option<String>,
) {
    if !success {
        return;
    }
    let Some(memory_service) = memory_service else {
        return;
    };
    if !daemon_config.load().orchestrator.costs.task_extract_enabled {
        return;
    }

    let params = TaskExtractionParams {
        owner_id: owner_id.to_string(),
        task_id: task_id.to_string(),
        task_description: task_description.to_string(),
        task_output: task_output.to_string(),
        source_path: source_path.to_string(),
        project_root: project_root.map(PathBuf::from),
        success,
    };
    let deps = TaskExtractionDeps {
        db: db.clone(),
        router: router.clone(),
        daemon_config: daemon_config.clone(),
        memory_service,
        cancel: background.cancel_token(),
    };
    background.spawn(async move {
        extract_task_memories(&deps, params).await;
    });
}
