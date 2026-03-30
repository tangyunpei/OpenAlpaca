//! Memory retrieval and task memory extraction helpers for dispatcher prompts.

use crate::daemon_config::DaemonConfig;
use crate::memory::scope_context::MemoryScopeContext;
use crate::memory::task_extraction::{TaskExtractionParams, extract_task_memories};
use arc_swap::ArcSwap;
use openalpaca_llm::LlmRouter;
use openalpaca_storage::Database;
use openalpaca_storage::repository::MemoryRepository;
use std::sync::Arc;

/// Retrieve relevant user memories as a formatted block for agent prompts.
/// Mirrors the retrieval pattern used in `handle_simple_query()`.
///
/// When `scope_ctx` is provided, uses cascading search (Workspace → Global).
/// When `None`, falls back to unscoped global search (backward compatibility for
/// pipeline and lead_agent contexts that don't yet carry scope context).
pub(in crate::orchestrator) async fn retrieve_memory_block(
    db: &Database,
    embedder: Option<&Arc<dyn openalpaca_llm::Embedder>>,
    owner_id: &str,
    query: &str,
    top_k: usize,
    scope_ctx: Option<&MemoryScopeContext>,
    access_boost: f64,
) -> Option<String> {
    let repo = MemoryRepository::new(db);
    let query_embedding = if let Some(embedder) = embedder {
        match embedder.embed(&[query]).await {
            Ok(v) => v.into_iter().next(),
            Err(e) => {
                tracing::warn!("Memory embedding failed, falling back to text-only search: {e}");
                None
            }
        }
    } else {
        None
    };
    let memories = if let Some(ctx) = scope_ctx {
        let cascade_scopes = ctx.cascade_scopes();
        match repo.search_hybrid_cascade(
            owner_id,
            query,
            query_embedding.as_deref(),
            top_k,
            None,
            &cascade_scopes,
        ) {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!("Memory cascade search failed: {e}");
                Vec::new()
            }
        }
    } else {
        match repo.search_hybrid(
            owner_id,
            query,
            query_embedding.as_deref(),
            top_k,
            None,
            None,
            None,
        ) {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!("Memory search failed: {e}");
                Vec::new()
            }
        }
    };

    if memories.is_empty() {
        return None;
    }

    // Track access for importance decay + boost
    let ids: Vec<i64> = memories.iter().map(|m| m.id).collect();
    if let Err(e) = repo.touch_accessed(&ids, access_boost) {
        tracing::warn!("Failed to track memory access: {e}");
    }

    let mut inner = String::new();
    let mut budget = 2000usize;
    for m in &memories {
        let entry = format!(
            "- [{}] {}\n",
            m.kind.as_str(),
            m.content.chars().take(300).collect::<String>()
        );
        if entry.len() > budget {
            break;
        }
        budget -= entry.len();
        inner.push_str(&entry);
    }
    Some(crate::orchestrator::wrap_untrusted_context(
        &inner,
        "retrieved_memory",
        "retrieved",
    ))
}

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
