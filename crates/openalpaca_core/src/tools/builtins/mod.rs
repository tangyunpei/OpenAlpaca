mod file_ops;
mod helpers;
mod memory_search;
mod send;
mod shell_execute;
// Stub tools — not registered (always returned "not implemented").
// Kept for potential future implementation.
#[allow(dead_code)]
mod summarize;
#[allow(dead_code)]
mod text_generate;
mod update_persona;
mod web_fetch;
mod web_search;

use crate::daemon_config::DaemonConfig;
use crate::orchestrator::ConnectorSendProvider;
use arc_swap::ArcSwap;
use openalpaca_llm::{ToolDefinition, WebSearchConfig};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Shared lock type for the connector send provider, set post-construction.
pub type ConnectorSendLock = Arc<RwLock<Option<Arc<dyn ConnectorSendProvider>>>>;

use self::file_ops::{file_read_tool, file_write_tool};
use self::memory_search::memory_search_tool;
use self::shell_execute::shell_execute_tool;
use self::update_persona::update_persona_tool;
use self::web_fetch::web_fetch_tool;
use self::web_search::web_search_tool;

use super::registry::RegisteredTool;

/// Context required by the `update_persona` tool at runtime.
/// Re-export from the update_persona module.
pub use update_persona::PersonaToolContext;

/// Return all built-in tool definitions and implementations.
/// When `db` is provided, memory-backed tools (memory_search) are included.
/// When `embedder` is provided, memory_search uses hybrid (FTS + vector) search.
///
/// `workspace_root` is the explicit workspace directory for file tools.
/// If `None`, falls back to `std::env::current_dir()` (for backward
/// compatibility in tests and CLI).
pub fn builtin_tools(
    db: Option<openalpaca_storage::Database>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
    web_search_config: Option<Arc<ArcSwap<WebSearchConfig>>>,
    workspace_root: Option<PathBuf>,
) -> Vec<RegisteredTool> {
    let ws_cfg = web_search_config
        .unwrap_or_else(|| Arc::new(ArcSwap::from_pointee(WebSearchConfig::default())));

    let ws_root = workspace_root
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let mut tools = vec![
        web_search_tool(ws_cfg),
        web_fetch_tool(),
        file_read_tool(ws_root.clone()),
        file_write_tool(ws_root),
        shell_execute_tool(),
    ];
    if let (Some(db), Some(dc)) = (db, daemon_config) {
        tools.push(memory_search_tool(db, embedder, dc));
    }
    tools
}

/// Return all built-in tools, including `update_persona`,
/// and optionally `send`.
pub fn builtin_tools_with_persona_context(
    db: Option<openalpaca_storage::Database>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    persona_ctx: PersonaToolContext,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
    web_search_config: Option<Arc<ArcSwap<WebSearchConfig>>>,
    workspace_root: Option<PathBuf>,
    connector_send_provider: Option<ConnectorSendLock>,
) -> Vec<RegisteredTool> {
    let mut tools = builtin_tools(db, embedder, daemon_config, web_search_config, workspace_root);
    tools.push(update_persona_tool(persona_ctx));
    if let Some(provider) = connector_send_provider {
        tools.push(send::send_tool(provider));
    }
    tools
}

/// Return ToolDefinition entries for workspace tools.
/// These tools are handled by ContextualToolExecutor (not the registry),
/// so they only have definitions (no BuiltInTool backend).
pub fn workspace_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "workspace_read".to_string(),
            description: "Read entries from the shared task workspace used for inter-agent \
                collaboration. Provide a specific key to retrieve one entry, or omit key \
                to list all entries. Returns a JSON array of entries with key, content, \
                author agent ID, and entry type. Use this to read work products from \
                other agents in a multi-agent task."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "key": {
                        "type": "string",
                        "description": "The workspace entry key to read (e.g., 'research_results', 'draft_v1'). Omit to list all entries."
                    }
                },
                "required": []
            }),
            strict: Some(true),
            input_examples: None,
        },
        ToolDefinition {
            name: "workspace_write".to_string(),
            description: "Write an entry to the shared task workspace for inter-agent \
                collaboration. Content is limited to 32KB. Entry types: 'text' (default), \
                'artifact' (code/document output), 'summary' (condensed results), or \
                'context' (background information). Supports optimistic concurrency — \
                retries automatically on conflict from parallel agent writes. Other \
                agents can read your entries via workspace_read."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "key": {
                        "type": "string",
                        "description": "A descriptive key for this entry (e.g., 'research_results', 'outline', 'draft_v1'). Must be unique per task."
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to store as a UTF-8 string (max 32KB)"
                    },
                    "entry_type": {
                        "type": "string",
                        "enum": ["text", "artifact", "summary", "context"],
                        "description": "Entry type: 'text' (general), 'artifact' (code/document output), 'summary' (condensed results), 'context' (background info). Default: 'text'"
                    },
                    "file_asset_id": {
                        "type": "string",
                        "description": "Optional file asset ID to associate with this entry. When set, enables file delivery to external channels (e.g. Telegram)."
                    }
                },
                "required": ["key", "content"]
            }),
            strict: Some(true),
            input_examples: Some(vec![
                serde_json::json!({
                    "key": "research_results",
                    "content": "Found 3 relevant papers on the topic...",
                    "entry_type": "summary"
                }),
                serde_json::json!({
                    "key": "draft_v1",
                    "content": "# Introduction\n\nThis document outlines...",
                    "entry_type": "artifact"
                }),
            ]),
        },
    ]
}

#[cfg(test)]
mod tests;
