mod file_ops;
mod helpers;
mod memory_search;
mod shell_execute;
mod summarize;
mod text_generate;
mod update_identity;
mod update_soul;
mod update_user;
mod web_fetch;
mod web_search;

use crate::bus::EventBus;
use crate::daemon_config::DaemonConfig;
use arc_swap::ArcSwap;
use openalpaca_llm::ToolDefinition;
use std::path::PathBuf;
use std::sync::Arc;

use self::file_ops::{file_read_tool, file_write_tool};
use self::memory_search::memory_search_tool;
use self::shell_execute::shell_execute_tool;
use self::summarize::summarize_tool;
use self::text_generate::text_generate_tool;
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
pub fn builtin_tools(
    db: Option<openalpaca_storage::Database>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
) -> Vec<RegisteredTool> {
    let dc = daemon_config
        .clone()
        .unwrap_or_else(|| Arc::new(ArcSwap::from_pointee(DaemonConfig::default())));

    let mut tools = vec![
        web_search_tool(dc),
        web_fetch_tool(),
        summarize_tool(),
        text_generate_tool(),
        file_read_tool(),
        file_write_tool(),
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
) -> Vec<RegisteredTool> {
    let mut tools = builtin_tools(db, embedder, daemon_config);
    tools.push(update_soul_tool(soul_ctx));
    tools
}

/// Return all built-in tools, including `update_soul`, `update_user`, and `update_identity`.
pub fn builtin_tools_with_persona_context(
    db: Option<openalpaca_storage::Database>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    soul_ctx: SoulToolContext,
    user_ctx: UserToolContext,
    identity_ctx: IdentityToolContext,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
) -> Vec<RegisteredTool> {
    let mut tools = builtin_tools(db, embedder, daemon_config);
    tools.push(update_soul_tool(soul_ctx));
    tools.push(update_user_tool(user_ctx));
    tools.push(update_identity_tool(identity_ctx));
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
                    }
                },
                "required": ["key", "content"]
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_tools_count_without_db() {
        let tools = builtin_tools(None, None, None);
        assert_eq!(tools.len(), 7);
    }

    #[test]
    fn test_builtin_tools_count_with_db() {
        let dir = tempfile::tempdir().unwrap();
        let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
        let dc = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
        let tools = builtin_tools(Some(db), None, Some(dc));
        assert_eq!(tools.len(), 8);
    }

    #[test]
    fn test_all_tools_have_valid_definitions() {
        for tool in builtin_tools(None, None, None) {
            assert!(!tool.definition.name.is_empty());
            assert!(!tool.definition.description.is_empty());
            assert!(tool.definition.parameters.is_object());
        }
    }

    #[test]
    fn test_builtin_tools_with_soul_context_includes_update_soul() {
        use crate::bus::EventBus;

        let dir = tempfile::tempdir().unwrap();
        let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
        let ctx = SoulToolContext {
            soul_path: dir.path().join("SOUL.md"),
            backup_dir: dir.path().join("backups"),
            bus: EventBus::new(16),
            max_backups: None,
        };
        let dc = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
        let tools = builtin_tools_with_soul_context(Some(db), None, ctx, Some(dc));
        assert_eq!(tools.len(), 9, "Should have 9 tools (8 base + update_soul)");
        assert!(
            tools.iter().any(|t| t.definition.name == "update_soul"),
            "update_soul tool must be present"
        );
    }
}
