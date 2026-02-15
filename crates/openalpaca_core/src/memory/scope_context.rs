//! Memory scope context: carries workspace identity through the request lifecycle.
//!
//! Created once in `handle_message()` and threaded to all memory read/write call sites.
//! Provides helpers for determining write scope and building cascading search scope lists.

use openalpaca_storage::models::memory::MemoryScope;

/// Carries workspace scope context for memory operations throughout a request.
#[derive(Debug, Clone)]
pub struct MemoryScopeContext {
    /// Detected workspace root path as scope_id. `None` if no workspace detected
    /// (e.g., running outside a project directory, or from Telegram).
    pub workspace_id: Option<String>,
}

impl MemoryScopeContext {
    /// Create a scope context with an optional workspace.
    pub fn new(workspace_id: Option<String>) -> Self {
        Self { workspace_id }
    }

    /// Create a scope context with no workspace (Global-only).
    pub fn global_only() -> Self {
        Self {
            workspace_id: None,
        }
    }

    /// Determine the default scope and scope_id for a memory write.
    ///
    /// If a workspace is detected, returns `(Workspace, workspace_id)`.
    /// Otherwise returns `(Global, "")`.
    pub fn default_write_scope(&self) -> (MemoryScope, &str) {
        if let Some(ref ws) = self.workspace_id {
            (MemoryScope::Workspace, ws.as_str())
        } else {
            (MemoryScope::Global, "")
        }
    }

    /// Build a list of (scope, scope_id) pairs for cascading retrieval.
    ///
    /// Returns `[Global, Workspace?]` — Global is always included.
    /// If a workspace is detected, Workspace scope is appended.
    /// The caller should iterate in reverse for most-specific-first ordering.
    pub fn cascade_scopes(&self) -> Vec<(MemoryScope, Option<&str>)> {
        let mut scopes = vec![(MemoryScope::Global, None)];
        if let Some(ref ws) = self.workspace_id {
            scopes.push((MemoryScope::Workspace, Some(ws.as_str())));
        }
        scopes
    }

    /// Whether a workspace context is available.
    pub fn has_workspace(&self) -> bool {
        self.workspace_id.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cascade_scopes_with_workspace() {
        let ctx = MemoryScopeContext::new(Some("/home/user/project".to_string()));
        let scopes = ctx.cascade_scopes();
        assert_eq!(scopes.len(), 2);
        assert_eq!(scopes[0], (MemoryScope::Global, None));
        assert_eq!(
            scopes[1],
            (MemoryScope::Workspace, Some("/home/user/project"))
        );
    }

    #[test]
    fn test_cascade_scopes_without_workspace() {
        let ctx = MemoryScopeContext::global_only();
        let scopes = ctx.cascade_scopes();
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0], (MemoryScope::Global, None));
    }

    #[test]
    fn test_default_write_scope_with_workspace() {
        let ctx = MemoryScopeContext::new(Some("/ws".to_string()));
        let (scope, id) = ctx.default_write_scope();
        assert_eq!(scope, MemoryScope::Workspace);
        assert_eq!(id, "/ws");
    }

    #[test]
    fn test_default_write_scope_without_workspace() {
        let ctx = MemoryScopeContext::global_only();
        let (scope, id) = ctx.default_write_scope();
        assert_eq!(scope, MemoryScope::Global);
        assert_eq!(id, "");
    }

    #[test]
    fn test_has_workspace() {
        assert!(MemoryScopeContext::new(Some("/ws".to_string())).has_workspace());
        assert!(!MemoryScopeContext::global_only().has_workspace());
    }
}
