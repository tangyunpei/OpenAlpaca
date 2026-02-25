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
    let (text, scope, id) = parse_remember_scope("--workspace this project uses SQLite", &ctx);
    assert_eq!(text, "this project uses SQLite");
    assert_eq!(scope, MemoryScope::Workspace);
    assert_eq!(id, "/home/user/project");
}

#[test]
fn test_parse_remember_scope_workspace_flag_at_end() {
    let ctx = MemoryScopeContext::new(Some("/ws".to_string()));
    let (text, scope, id) = parse_remember_scope("this project uses SQLite --workspace", &ctx);
    assert_eq!(text, "this project uses SQLite");
    assert_eq!(scope, MemoryScope::Workspace);
    assert_eq!(id, "/ws");
}

#[test]
fn test_parse_remember_scope_workspace_no_detection() {
    let ctx = MemoryScopeContext::global_only();
    let (text, scope, id) = parse_remember_scope("--workspace this project uses SQLite", &ctx);
    assert_eq!(text, "this project uses SQLite");
    // Falls back to Global when no workspace detected
    assert_eq!(scope, MemoryScope::Global);
    assert_eq!(id, "");
}
