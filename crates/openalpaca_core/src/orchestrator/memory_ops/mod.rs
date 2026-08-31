use crate::daemon_config::DaemonConfig;
use crate::memory::scope_context::MemoryScopeContext;
use crate::memory::task_extraction::{PersistResult, persist_memory_item};
use arc_swap::ArcSwap;
use openalpaca_storage::Database;
use openalpaca_storage::models::memory::{MemoryKind, MemoryScope, MemorySource};
use openalpaca_storage::repository::MemoryRepository;
use std::sync::Arc;

/// Store a "remember X" item in Memory v2 as a Preference (shared core).
///
/// Checks for semantically similar existing memories and supersedes if found.
/// Supports a `--workspace` flag in `content` to scope the memory to the
/// current project workspace; default (no flag) stores as Global scope.
///
/// Used by the `memory_store` builtin tool (Routing V2).
pub(crate) async fn store_memory(
    db: Option<&Database>,
    embedder: &Option<Arc<dyn openalpaca_llm::Embedder>>,
    daemon_config: &Arc<ArcSwap<DaemonConfig>>,
    content: &str,
    owner_id: Option<&str>,
    scope_ctx: &MemoryScopeContext,
) -> Result<String, String> {
    let oid = owner_id.ok_or_else(|| "Cannot store memory without an owner_id".to_string())?;

    if let Some(db) = db {
        let repo = MemoryRepository::new(db);
        let dcfg = daemon_config.load();
        let supersession_threshold = dcfg.orchestrator.memory.supersession_distance_threshold;
        let jaccard_threshold = dcfg.orchestrator.memory.fts_jaccard_threshold;

        // Parse --workspace flag from content
        let (text, scope, scope_id) = parse_remember_scope(content, scope_ctx);

        let result = persist_memory_item(
            &repo,
            embedder,
            oid,
            &text,
            MemoryKind::Preference,
            scope,
            &scope_id,
            MemorySource::Conversation,
            0.9,
            1.0,
            None,
            supersession_threshold,
            jaccard_threshold,
        )
        .await;

        let scope_label = match scope {
            MemoryScope::Global => "",
            MemoryScope::Workspace => " (workspace)",
            MemoryScope::Lane => " (lane)",
        };

        match result {
            PersistResult::Superseded { old_content, .. } => Ok(format!(
                "Got it, I've updated my memory{} (was: \"{}\"): {}",
                scope_label,
                old_content.chars().take(50).collect::<String>(),
                text
            )),
            PersistResult::Inserted(_) => Ok(format!(
                "Got it, I'll remember that{}: {}",
                scope_label, text
            )),
            PersistResult::Duplicate => Ok("I already have that noted.".to_string()),
            PersistResult::Error(e) => Err(format!("Failed to store memory: {}", e)),
        }
    } else {
        Err("Memory system is not available.".to_string())
    }
}

/// Search for and delete the best-matching Preference memory (shared core).
///
/// Used by the `memory_forget` builtin tool (Routing V2).
pub(crate) fn forget_memory(
    db: Option<&Database>,
    content: &str,
    owner_id: Option<&str>,
) -> Result<String, String> {
    let oid = owner_id.ok_or_else(|| "Cannot search memory without an owner_id".to_string())?;

    if let Some(db) = db {
        let repo = MemoryRepository::new(db);

        // Search for matching memories
        let memories = repo
            .search_fts(
                oid,
                content,
                5,
                Some(openalpaca_storage::models::memory::MemoryKind::Preference),
                None,
                None,
            )
            .map_err(|e| format!("Failed to search memory: {}", e))?;

        if memories.is_empty() {
            return Ok(format!("I don't have any memory matching: {}", content));
        }

        // Delete the best match (first result)
        let best_match = &memories[0];
        repo.delete(best_match.id)
            .map_err(|e| format!("Failed to delete memory: {}", e))?;

        Ok(format!(
            "Done, I've forgotten: {}",
            best_match.content.chars().take(100).collect::<String>()
        ))
    } else {
        Err("Memory system is not available.".to_string())
    }
}

/// Parse `--workspace` flag from remember command content.
///
/// Returns `(cleaned_content, scope, scope_id)`.
/// - `--workspace` → Workspace scope with current workspace_id (falls back to Global if no workspace detected)
/// - No flag → Global scope with empty scope_id
fn parse_remember_scope(
    text: &str,
    scope_ctx: &MemoryScopeContext,
) -> (String, MemoryScope, String) {
    if text.contains("--workspace") {
        let cleaned = text.replace("--workspace", "").trim().to_string();
        if let Some(ref ws) = scope_ctx.workspace_id {
            return (cleaned, MemoryScope::Workspace, ws.clone());
        }
        tracing::warn!(
            "--workspace flag used but no workspace detected; falling back to Global scope"
        );
        return (cleaned, MemoryScope::Global, String::new());
    }

    // Default: Global scope
    (text.to_string(), MemoryScope::Global, String::new())
}

#[cfg(test)]
mod tests;
