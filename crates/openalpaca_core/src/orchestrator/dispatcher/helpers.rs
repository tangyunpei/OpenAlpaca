//! Free helper functions used by the task dispatcher and its submodules.

use arc_swap::ArcSwap;
use crate::daemon_config::DaemonConfig;
use crate::memory::task_extraction::{TaskExtractionParams, extract_task_memories};
use openalpaca_llm::LlmRouter;
use openalpaca_storage::Database;
use openalpaca_storage::repository::MemoryRepository;
use std::sync::Arc;

/// Retrieve relevant user memories as a formatted block for agent prompts.
/// Mirrors the retrieval pattern used in `handle_simple_query()`.
pub(crate) async fn retrieve_memory_block(
    db: &Database,
    embedder: Option<&Arc<dyn openalpaca_llm::Embedder>>,
    owner_id: &str,
    query: &str,
    top_k: usize,
) -> Option<String> {
    let repo = MemoryRepository::new(db);
    let query_embedding = if let Some(embedder) = embedder {
        embedder
            .embed(&[query])
            .await
            .ok()
            .and_then(|v| v.into_iter().next())
    } else {
        None
    };
    let memories = repo
        .search_hybrid(
            owner_id,
            query,
            query_embedding.as_deref(),
            top_k,
            None,
            None,
            None,
        )
        .unwrap_or_default();

    if memories.is_empty() {
        return None;
    }

    // Track access for importance decay
    let ids: Vec<i64> = memories.iter().map(|m| m.id).collect();
    if let Err(e) = repo.touch_accessed(&ids) {
        tracing::warn!("Failed to track memory access: {e}");
    }

    let mut block = String::from("### RETRIEVED MEMORY ###\n");
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
        block.push_str(&entry);
    }
    Some(block)
}

/// Spawn a background task to extract memories from a completed task output.
/// Fire-and-forget: does not block the caller. Only runs for successful tasks.
pub(crate) fn spawn_task_memory_extraction(
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
    };
    let db = db.clone();
    let router = router.clone();
    let embedder = embedder.clone();
    let daemon_config = daemon_config.clone();

    tokio::spawn(async move {
        extract_task_memories(params, db, router, embedder, daemon_config).await;
    });
}

/// Generate a concise task title from a description by stripping filler prefixes
/// and truncating to a reasonable length.
pub(crate) fn generate_title(description: &str) -> String {
    let lower = description.to_lowercase();
    // Strip filler prefixes
    let cleaned = lower
        .trim_start_matches("can you ")
        .trim_start_matches("could you ")
        .trim_start_matches("please ")
        .trim_start_matches("help me ")
        .trim_start_matches("i need to ")
        .trim_start_matches("i want to ");
    // Capitalize first letter
    let mut chars = cleaned.chars();
    let title: String = match chars.next() {
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
        None => description.to_string(),
    };
    // Take first 8 words or 50 chars
    let words: Vec<&str> = title.split_whitespace().take(8).collect();
    let result = words.join(" ");
    if result.len() > 50 {
        format!("{}...", &result[..47])
    } else if words.len() == 8 && title.split_whitespace().count() > 8 {
        format!("{}...", result)
    } else {
        result
    }
}

/// Format a task result for display in the chat conversation.
pub(crate) fn format_task_result(title: &str, summary: &str, is_success: bool) -> String {
    if is_success {
        format!("**Task completed: {}**\n\n{}", title, summary)
    } else {
        format!("**Task failed: {}**\n\n{}", title, summary)
    }
}
