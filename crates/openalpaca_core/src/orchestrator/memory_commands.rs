//! Memory commands for the Orchestrator: remember and forget.

use crate::memory::task_extraction::{PersistResult, persist_memory_item};
use openalpaca_storage::repository::MemoryRepository;

use super::Orchestrator;

impl Orchestrator {
    /// Handle "remember X" commands by storing in Memory v2 as a Preference.
    /// Checks for semantically similar existing memories and supersedes if found.
    pub(crate) async fn handle_remember_command(
        &self,
        content: &str,
        owner_id: Option<&str>,
    ) -> Result<String, String> {
        let oid = owner_id.ok_or_else(|| "Cannot store memory without an owner_id".to_string())?;

        if let Some(ref db) = self.db {
            let repo = MemoryRepository::new(db);
            let dcfg = self.daemon_config.load();
            let supersession_threshold = dcfg.orchestrator.memory.supersession_distance_threshold;
            let jaccard_threshold = dcfg.orchestrator.memory.fts_jaccard_threshold;

            let result = persist_memory_item(
                &repo,
                &self.embedder,
                oid,
                content,
                openalpaca_storage::models::memory::MemoryKind::Preference,
                openalpaca_storage::models::memory::MemoryScope::Global,
                "",
                openalpaca_storage::models::memory::MemorySource::Conversation,
                0.9,
                1.0,
                None,
                supersession_threshold,
                jaccard_threshold,
            )
            .await;

            match result {
                PersistResult::Superseded { old_content, .. } => {
                    Ok(format!(
                        "Got it, I've updated my memory (was: \"{}\"): {}",
                        old_content.chars().take(50).collect::<String>(),
                        content
                    ))
                }
                PersistResult::Inserted(_) => {
                    Ok(format!("Got it, I'll remember that: {}", content))
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
    pub(crate) async fn handle_forget_command(
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
