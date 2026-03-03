mod file_ops;
mod helpers;
mod memory_search;
mod send_file;
mod send_message;
mod shell_execute;
// Stub tools — not registered (always returned "not implemented").
// Kept for potential future implementation.
#[allow(dead_code)]
mod summarize;
#[allow(dead_code)]
mod text_generate;
mod update_identity;
mod update_soul;
mod update_user;
mod web_fetch;
mod web_search;

use crate::bus::EventBus;
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
use self::update_identity::update_identity_tool;
use self::update_soul::update_soul_tool;
use self::update_user::update_user_tool;
use self::web_fetch::web_fetch_tool;
use self::web_search::web_search_tool;

use super::registry::RegisteredTool;

/// Context required by the `update_soul` tool at runtime.
#[derive(Clone)]
pub struct SoulToolContext {
    /// Absolute path to the active `SOUL.md` file.
    pub soul_path: PathBuf,
    /// Directory for timestamped backups.
    pub backup_dir: PathBuf,
    /// Event bus for publishing `SoulUpdated` events.
    pub bus: EventBus,
    /// Maximum number of backups to keep. `None` = keep all (MVP default).
    pub max_backups: Option<usize>,
}

/// Context required by the `update_user` tool at runtime.
#[derive(Clone)]
pub struct UserToolContext {
    /// Absolute path to the active `USER.md` file.
    pub user_path: PathBuf,
    /// Directory for timestamped backups.
    pub backup_dir: PathBuf,
    /// Event bus for publishing `UserProfileUpdated` events.
    pub bus: EventBus,
    /// Maximum number of backups to keep.
    pub max_backups: Option<usize>,
}

/// Context required by the `update_identity` tool at runtime.
#[derive(Clone)]
pub struct IdentityToolContext {
    /// Absolute path to the active `IDENTITY.md` file.
    pub identity_path: PathBuf,
    /// Directory for timestamped backups.
    pub backup_dir: PathBuf,
    /// Event bus for publishing `IdentityUpdated` events.
    pub bus: EventBus,
    /// Maximum number of backups to keep.
    pub max_backups: Option<usize>,
}

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

/// Return all built-in tools, including the `update_soul` tool which requires
/// additional context (file paths, event bus).
pub fn builtin_tools_with_soul_context(
    db: Option<openalpaca_storage::Database>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    soul_ctx: SoulToolContext,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
    web_search_config: Option<Arc<ArcSwap<WebSearchConfig>>>,
    workspace_root: Option<PathBuf>,
) -> Vec<RegisteredTool> {
    let mut tools = builtin_tools(db, embedder, daemon_config, web_search_config, workspace_root);
    tools.push(update_soul_tool(soul_ctx));
    tools
}

/// Return all built-in tools, including `update_soul`, `update_user`, `update_identity`,
/// and optionally `send_message`.
#[allow(clippy::too_many_arguments)]
pub fn builtin_tools_with_persona_context(
    db: Option<openalpaca_storage::Database>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    soul_ctx: SoulToolContext,
    user_ctx: UserToolContext,
    identity_ctx: IdentityToolContext,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
    web_search_config: Option<Arc<ArcSwap<WebSearchConfig>>>,
    workspace_root: Option<PathBuf>,
    connector_send_provider: Option<ConnectorSendLock>,
) -> Vec<RegisteredTool> {
    let mut tools = builtin_tools(db, embedder, daemon_config, web_search_config, workspace_root);
    tools.push(update_soul_tool(soul_ctx));
    tools.push(update_user_tool(user_ctx));
    tools.push(update_identity_tool(identity_ctx));
    if let Some(provider) = connector_send_provider {
        tools.push(send_message::send_message_tool(provider.clone()));
        tools.push(send_file::send_file_tool(provider));
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
            description:
                "Read entries from the shared task workspace. If key is empty, returns all entries."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "key": {
                        "type": "string",
                        "description": "The workspace entry key to read. Leave empty to list all entries."
                    }
                },
                "required": []
            }),
            strict: None,
            input_examples: None,
        },
        ToolDefinition {
            name: "workspace_write".to_string(),
            description: "Write an entry to the shared task workspace for other agents to read."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "key": {
                        "type": "string",
                        "description": "A descriptive key for this entry (e.g. 'research_results', 'outline', 'draft_v1')"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to store (max 32KB)"
                    },
                    "entry_type": {
                        "type": "string",
                        "enum": ["text", "artifact", "summary", "context"],
                        "description": "The type of entry. Default: text"
                    },
                    "file_asset_id": {
                        "type": "string",
                        "description": "Optional file asset ID to associate with this entry. When set, enables file delivery to external channels (e.g. Telegram)."
                    }
                },
                "required": ["key", "content"]
            }),
            strict: None,
            input_examples: None,
        },
    ]
}

#[cfg(test)]
mod tests;
