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
