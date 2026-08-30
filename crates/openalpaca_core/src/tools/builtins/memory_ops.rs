//! `memory_store` / `memory_forget` — Routing V2 main-loop tools wrapping the
//! shared memory-command cores in `orchestrator::memory_ops`.
//!
//! Constructed per-request by the tool-mode main loop (the old
//! remember/forget intent handlers are thin wrappers over the same shared
//! functions, so both paths stay behaviorally identical). NOT registered in
//! the global registry — global registration would change
//! `registered_tool_names()` and thus the triage prompt on the legacy path.

use crate::daemon_config::DaemonConfig;
use crate::memory::scope_context::MemoryScopeContext;
use crate::orchestrator::{forget_memory, store_memory};
use crate::tools::registry::{BuiltInTool, ToolContext};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

// ── MemoryStoreTool ──────────────────────────────────────────────────

/// Stores a long-term memory (Preference) for the requesting user.
pub struct MemoryStoreTool {
    db: Option<openalpaca_storage::Database>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
}

impl MemoryStoreTool {
    pub fn new(
        db: Option<openalpaca_storage::Database>,
        embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
    ) -> Self {
        Self {
            db,
            embedder,
            daemon_config,
        }
    }
}

#[async_trait]
impl BuiltInTool for MemoryStoreTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("memory_store requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| "Missing required parameter: content".to_string())?;

        // Identity from context, not from arguments (anti-spoofing) — same
        // contract as `memory_search`.
        let scope_ctx = MemoryScopeContext::new(ctx.workspace_id.clone());
        store_memory(
            self.db.as_ref(),
            &self.embedder,
            &self.daemon_config,
            content,
            ctx.owner_id.as_deref(),
            &scope_ctx,
        )
        .await
    }
}

/// Build the tool definition for `memory_store`.
pub fn memory_store_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "memory_store".to_string(),
        description: "Store a fact or preference in the user's long-term memory so it \
                       persists across conversations. Use when the user asks you to \
                       remember something, or states a durable preference worth keeping. \
                       Semantically similar existing memories are superseded automatically."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The fact or preference to remember, stated concisely (append --workspace to scope it to the current project)"
                }
            },
            "required": ["content"]
        }),
        strict: Some(true),
        input_examples: Some(vec![
            serde_json::json!({"content": "The user prefers concise answers in Chinese"}),
        ]),
    }
}

// ── MemoryForgetTool ─────────────────────────────────────────────────

/// Deletes the best-matching long-term memory for the requesting user.
pub struct MemoryForgetTool {
    db: Option<openalpaca_storage::Database>,
}

impl MemoryForgetTool {
    pub fn new(db: Option<openalpaca_storage::Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl BuiltInTool for MemoryForgetTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("memory_forget requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| "Missing required parameter: query".to_string())?;

        forget_memory(self.db.as_ref(), query, ctx.owner_id.as_deref())
    }
}

/// Build the tool definition for `memory_forget` (see tests below for the
/// old-handler equivalence contract).
pub fn memory_forget_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "memory_forget".to_string(),
        description: "Delete a stored long-term memory. Searches the user's memories for \
                       the query and removes the best match. Use when the user asks you to \
                       forget something previously remembered."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What to forget — text matching the memory to remove"
                }
            },
            "required": ["query"]
        }),
        strict: Some(true),
        input_examples: Some(vec![serde_json::json!({"query": "preferred language"})]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_db(dir: &tempfile::TempDir) -> openalpaca_storage::Database {
        openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap()
    }

    fn owner_ctx(owner: &str) -> ToolContext {
        ToolContext {
            owner_id: Some(owner.to_string()),
            ..Default::default()
        }
    }

    fn default_config() -> Arc<ArcSwap<DaemonConfig>> {
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default()))
    }

    #[tokio::test]
    async fn test_memory_store_forget_roundtrip_matches_old_handler_contract() {
        let dir = tempfile::tempdir().unwrap();
        let db = make_db(&dir);
        let store = MemoryStoreTool::new(Some(db.clone()), None, default_config());
        let forget = MemoryForgetTool::new(Some(db.clone()));
        let ctx = owner_ctx("user1");

        // Store — same response the old remember handler produced.
        let stored = store
            .execute_with_context(
                &serde_json::json!({"content": "the user prefers dark mode"}),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(
            stored,
            "Got it, I'll remember that: the user prefers dark mode"
        );

        // Duplicate — old handler's dedup reply.
        let dup = store
            .execute_with_context(
                &serde_json::json!({"content": "the user prefers dark mode"}),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(dup, "I already have that noted.");

        // Forget — old forget handler's reply, and the row is really gone.
        let forgotten = forget
            .execute_with_context(&serde_json::json!({"query": "dark mode"}), &ctx)
            .await
            .unwrap();
        assert_eq!(
            forgotten,
            "Done, I've forgotten: the user prefers dark mode"
        );
        let gone = forget
            .execute_with_context(&serde_json::json!({"query": "dark mode"}), &ctx)
            .await
            .unwrap();
        assert_eq!(gone, "I don't have any memory matching: dark mode");
    }

    #[tokio::test]
    async fn test_memory_store_equals_shared_handler_core() {
        // The intent handlers are thin wrappers over the same shared
        // functions — a direct call must produce byte-identical output.
        let dir_tool = tempfile::tempdir().unwrap();
        let dir_core = tempfile::tempdir().unwrap();
        let (db_tool, db_core) = (make_db(&dir_tool), make_db(&dir_core));
        let cfg = default_config();

        let tool = MemoryStoreTool::new(Some(db_tool), None, cfg.clone());
        let via_tool = tool
            .execute_with_context(
                &serde_json::json!({"content": "release day is Friday"}),
                &owner_ctx("user1"),
            )
            .await
            .unwrap();
        let via_core = crate::orchestrator::store_memory(
            Some(&db_core),
            &None,
            &cfg,
            "release day is Friday",
            Some("user1"),
            &crate::memory::scope_context::MemoryScopeContext::global_only(),
        )
        .await
        .unwrap();
        assert_eq!(via_tool, via_core);
    }

    #[tokio::test]
    async fn test_memory_tools_require_owner_and_db() {
        let cfg = default_config();
        let store = MemoryStoreTool::new(None, None, cfg.clone());
        // No DB → the old handler's unavailability error.
        let err = store
            .execute_with_context(
                &serde_json::json!({"content": "x"}),
                &owner_ctx("user1"),
            )
            .await
            .unwrap_err();
        assert_eq!(err, "Memory system is not available.");

        // No owner → the old handler's owner error.
        let dir = tempfile::tempdir().unwrap();
        let db = make_db(&dir);
        let store = MemoryStoreTool::new(Some(db.clone()), None, cfg);
        let err = store
            .execute_with_context(&serde_json::json!({"content": "x"}), &ToolContext::default())
            .await
            .unwrap_err();
        assert_eq!(err, "Cannot store memory without an owner_id");

        let forget = MemoryForgetTool::new(Some(db));
        let err = forget
            .execute_with_context(&serde_json::json!({"query": "x"}), &ToolContext::default())
            .await
            .unwrap_err();
        assert_eq!(err, "Cannot search memory without an owner_id");
    }
}
