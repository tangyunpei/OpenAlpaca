use arc_swap::ArcSwap;
use async_trait::async_trait;
use crate::daemon_config::DaemonConfig;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

struct MemorySearchTool {
    db: openalpaca_storage::Database,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
}

#[async_trait]
impl BuiltInTool for MemorySearchTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: query".to_string())?;

        let limit = arguments.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

        let owner_id = arguments
            .get("owner_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing owner_id (should be injected by executor)".to_string())?;

        // Generate query embedding if embedder available
        let query_embedding = if let Some(ref embedder) = self.embedder {
            embedder
                .embed(&[query])
                .await
                .ok()
                .and_then(|v| v.into_iter().next())
        } else {
            None
        };

        let repo = openalpaca_storage::repository::MemoryRepository::new(&self.db);
        let memories = repo
            .search_hybrid(
                owner_id,
                query,
                query_embedding.as_deref(),
                limit,
                None,
                None,
                None,
            )
            .map_err(|e| format!("Memory search failed: {}", e))?;

        // Track access for importance decay + boost
        if !memories.is_empty() {
            let ids: Vec<i64> = memories.iter().map(|m| m.id).collect();
            let access_boost = self.daemon_config.load().orchestrator.memory.decay.access_boost;
            if let Err(e) = repo.touch_accessed(&ids, access_boost) {
                tracing::warn!("Failed to track memory access: {e}");
            }
        }

        let results: Vec<serde_json::Value> = memories
            .iter()
            .map(|m| {
                serde_json::json!({
                    "id": m.id,
                    "kind": m.kind.as_str(),
                    "scope": m.scope.as_str(),
                    "content": m.content,
                    "importance": m.importance,
                    "created_at": m.created_at,
                })
            })
            .collect();

        serde_json::to_string(&results).map_err(|e| format!("JSON serialization failed: {}", e))
    }
}

pub(super) fn memory_search_tool(
    db: openalpaca_storage::Database,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "memory_search".to_string(),
            description: "Search the user's memory for relevant facts, preferences, and knowledge. Use this when you need to recall something the user told you previously.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query to find relevant memories"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 5)"
                    }
                },
                "required": ["query"]
            }),
        },
        backend: ToolBackend::BuiltIn(Arc::new(MemorySearchTool { db, embedder, daemon_config })),
    }
}
