use super::Orchestrator;
use crate::memory::scope_context::MemoryScopeContext;
use crate::memory::task_extraction::{PersistResult, persist_memory_item};
use openalpaca_storage::models::memory::{MemoryKind, MemoryScope, MemorySource};
use openalpaca_storage::repository::MemoryRepository;

impl Orchestrator {
    /// Handle "remember X" commands by storing in Memory v2 as a Preference.
    /// Checks for semantically similar existing memories and supersedes if found.
    ///
    /// Supports `--workspace` flag to scope the memory to the current project workspace.
    /// Default (no flag) stores as Global scope.
    pub(super) async fn handle_remember_command(
        &self,
        content: &str,
        owner_id: Option<&str>,
        scope_ctx: &MemoryScopeContext,
    ) -> Result<String, String> {
        let oid = owner_id.ok_or_else(|| "Cannot store memory without an owner_id".to_string())?;

        if let Some(ref db) = self.db {
            let repo = MemoryRepository::new(db);
            let dcfg = self.daemon_config.load();
            let supersession_threshold = dcfg.orchestrator.memory.supersession_distance_threshold;
            let jaccard_threshold = dcfg.orchestrator.memory.fts_jaccard_threshold;

            // Parse --workspace flag from content
            let (text, scope, scope_id) = parse_remember_scope(content, scope_ctx);

            let result = persist_memory_item(
                &repo,
                &self.embedder,
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
                PersistResult::Superseded { old_content, .. } => {
                    Ok(format!(
                        "Got it, I've updated my memory{} (was: \"{}\"): {}",
                        scope_label,
                        old_content.chars().take(50).collect::<String>(),
                        text
                    ))
                }
                PersistResult::Inserted(_) => {
                    Ok(format!("Got it, I'll remember that{}: {}", scope_label, text))
                }
                PersistResult::Duplicate => {
                    Ok("I already have that noted.".to_string())
                }
                PersistResult::Error(e) => {
                    Err(format!("Failed to store memory: {}", e))
                }
            }
        } else {
            Err("Memory system is not available.".to_string())
        }
    }

    /// Handle "forget X" commands by searching and removing from Memory v2.
    pub(super) async fn handle_forget_command(
        &self,
        content: &str,
        owner_id: Option<&str>,
    ) -> Result<String, String> {
        let oid = owner_id.ok_or_else(|| "Cannot search memory without an owner_id".to_string())?;

        if let Some(ref db) = self.db {
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
mod tests {
    use super::*;

    #[test]
    fn test_parse_remember_scope_default_global() {
        let ctx = MemoryScopeContext::global_only();
        let (text, scope, id) = parse_remember_scope("I like dark mode", &ctx);
        assert_eq!(text, "I like dark mode");
        assert_eq!(scope, MemoryScope::Global);
        assert_eq!(id, "");
    }

    #[test]
    fn test_parse_remember_scope_workspace_flag() {
        let ctx = MemoryScopeContext::new(Some("/home/user/project".to_string()));
        let (text, scope, id) =
            parse_remember_scope("--workspace this project uses SQLite", &ctx);
        assert_eq!(text, "this project uses SQLite");
        assert_eq!(scope, MemoryScope::Workspace);
        assert_eq!(id, "/home/user/project");
    }

    #[test]
    fn test_parse_remember_scope_workspace_flag_at_end() {
        let ctx = MemoryScopeContext::new(Some("/ws".to_string()));
        let (text, scope, id) =
            parse_remember_scope("this project uses SQLite --workspace", &ctx);
        assert_eq!(text, "this project uses SQLite");
        assert_eq!(scope, MemoryScope::Workspace);
        assert_eq!(id, "/ws");
    }

    #[test]
    fn test_parse_remember_scope_workspace_no_detection() {
        let ctx = MemoryScopeContext::global_only();
        let (text, scope, id) =
            parse_remember_scope("--workspace this project uses SQLite", &ctx);
        assert_eq!(text, "this project uses SQLite");
        // Falls back to Global when no workspace detected
        assert_eq!(scope, MemoryScope::Global);
        assert_eq!(id, "");
    }
}
