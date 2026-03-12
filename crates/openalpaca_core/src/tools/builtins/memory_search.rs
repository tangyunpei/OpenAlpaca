use crate::daemon_config::DaemonConfig;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use arc_swap::ArcSwap;
use async_trait::async_trait;
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

        // Clamp limit to prevent excessively large SQL queries and potential
        // u64→i64 overflow when passed to SQL LIMIT clauses downstream.
        const MAX_MEMORY_SEARCH_LIMIT: u64 = 100;
        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .min(MAX_MEMORY_SEARCH_LIMIT) as usize;

        let owner_id = arguments
            .get("owner_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing owner_id (should be injected by executor)".to_string())?;

        let workspace_id = arguments
            .get("workspace_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

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

        // Use cascade search with workspace scope context when available
        let scope_ctx = crate::memory::scope_context::MemoryScopeContext::new(workspace_id);
        let cascade_scopes = scope_ctx.cascade_scopes();
        let memories = repo
            .search_hybrid_cascade(
                owner_id,
                query,
                query_embedding.as_deref(),
                limit,
                None,
                &cascade_scopes,
            )
            .map_err(|e| format!("Memory search failed: {}", e))?;

        // Track access for importance decay + boost
        if !memories.is_empty() {
            let ids: Vec<i64> = memories.iter().map(|m| m.id).collect();
            let access_boost = self
                .daemon_config
                .load()
                .orchestrator
                .memory
                .decay
                .access_boost;
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
            description: "Search the user's long-term memory store for relevant facts, \
                preferences, past conversations, and learned knowledge. Uses hybrid \
                search combining full-text search and vector similarity. Returns up to \
                'limit' results (default 5, max 100), each with content, relevance \
                score, and metadata. Use this to recall user preferences, past \
                decisions, or contextual information before acting."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural language search query to find relevant memories (e.g., 'user's preferred language', 'last discussed project')"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 5, max: 100)"
                    }
                },
                "required": ["query"]
            }),
            strict: Some(true),
            input_examples: Some(vec![
                serde_json::json!({"query": "user preferences for code style"}),
                serde_json::json!({"query": "previous conversation about authentication", "limit": 10}),
            ]),
        },
        backend: ToolBackend::BuiltIn(Arc::new(MemorySearchTool { db, embedder, daemon_config })),
        provides_capabilities: vec!["memory_read".into()],
    }
}
