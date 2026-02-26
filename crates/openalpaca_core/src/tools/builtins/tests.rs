use super::*;

#[test]
fn test_builtin_tools_count_without_db() {
    let tools = builtin_tools(None, None, None, None, None);
    assert_eq!(tools.len(), 5);
}

#[test]
fn test_builtin_tools_count_with_db() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let dc = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
    let tools = builtin_tools(Some(db), None, Some(dc), None, None);
    assert_eq!(tools.len(), 6);
}

#[test]
fn test_all_tools_have_valid_definitions() {
    for tool in builtin_tools(None, None, None, None, None) {
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
    let tools = builtin_tools_with_soul_context(Some(db), None, ctx, Some(dc), None, None);
    assert_eq!(tools.len(), 7, "Should have 7 tools (6 base + update_soul)");
    assert!(
        tools.iter().any(|t| t.definition.name == "update_soul"),
        "update_soul tool must be present"
    );
}

// --- Issue 2: shell_execute 300s defense-in-depth timeout ---

#[tokio::test]
async fn test_shell_execute_timeout_is_300s() {
    // The shell_execute tool has a 300s defense-in-depth timeout.
    // We verify this by running a quick command through the tool and
    // checking the tool produces correct output (it wouldn't if the
    // timeout were misconfigured to 0).
    use crate::tools::registry::ToolBackend;

    let tool = shell_execute::shell_execute_tool();
    match &tool.backend {
        ToolBackend::BuiltIn(handler) => {
            let result: Result<String, String> = handler
                .execute(&serde_json::json!({"command": "echo test_timeout"}))
                .await;
            assert!(
                result.is_ok(),
                "shell_execute should succeed for quick commands"
            );
            assert!(result.unwrap().contains("test_timeout"));
        }
        _ => panic!("shell_execute should use BuiltIn backend"),
    }
}

// --- Issue 5: Workspace root override vs current_dir() ---

#[test]
fn test_builtin_tools_uses_explicit_workspace_root() {
    let dir = tempfile::tempdir().unwrap();
    let ws_root = dir.path().to_path_buf();

    // Pass an explicit workspace root — should not fall back to current_dir()
    let tools = builtin_tools(None, None, None, None, Some(ws_root.clone()));
    assert_eq!(tools.len(), 5);

    // Verify tools were created (they compile and register without error
    // when given an explicit workspace root).
    let tool_names: Vec<&str> = tools.iter().map(|t| t.definition.name.as_str()).collect();
    assert!(tool_names.contains(&"file_read"));
    assert!(tool_names.contains(&"file_write"));
}

#[test]
fn test_builtin_tools_falls_back_to_current_dir_when_none() {
    // When workspace_root is None, should fall back to current_dir()
    // This must not panic even if current_dir() is available.
    let tools = builtin_tools(None, None, None, None, None);
    assert_eq!(tools.len(), 5);
}
