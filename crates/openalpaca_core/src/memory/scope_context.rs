//! Workspace scope context: carries workspace identity through the request lifecycle.
//!
//! Created once in `handle_message()` and threaded to all memory read/write call sites.
//! Provides helpers for determining write scope and building cascading search scope lists.
//!
//! Two *different* workspace values travel here, and conflating them is the
//! defect ruling R22 fixed:
//!
//! - [`workspace_id`](MemoryScopeContext::workspace_id) scopes **memory**. It
//!   falls back to the daemon's current directory when the request carried no
//!   workspace, which is right for memory and wrong for anything that writes
//!   files.
//! - [`request_workspace_root`](MemoryScopeContext::request_workspace_root)
//!   scopes **content the request owns** (artifacts). It is set only when a
//!   client actually sent a workspace path, and is `None` for connector lanes,
//!   scheduled skills and every other turn without one.

use openalpaca_storage::models::memory::MemoryScope;

/// Carries workspace scope context for a request.
#[derive(Debug, Clone, Default)]
pub struct MemoryScopeContext {
    /// Detected workspace root path as scope_id. `None` if no workspace detected
    /// (e.g., running outside a project directory, or from Telegram).
    ///
    /// Memory scoping only: on a request that carried no workspace this is
    /// derived from the daemon's CWD, so it is **not** evidence that the turn
    /// belongs to a project.
    pub workspace_id: Option<String>,
    /// The project root the *request* supplied — `x-workspace-path` on
    /// `/v1/chat`, `workspace_path` on `/v1/command` — resolved to its root.
    /// `None` whenever no client sent one; never CWD-derived (R22).
    ///
    /// This is the only field that may place a file on disk.
    pub request_workspace_root: Option<String>,
}

impl MemoryScopeContext {
    /// Create a scope context with an optional workspace for **memory scoping**.
    ///
    /// `request_workspace_root` stays `None`: a caller that has a
    /// request-supplied workspace builds the context with
    /// [`MemoryScopeContext::for_request`] instead.
    pub fn new(workspace_id: Option<String>) -> Self {
        Self {
            workspace_id,
            request_workspace_root: None,
        }
    }

    /// The workspace context of an incoming turn (R22) — the one place that
    /// knows the rule.
    ///
    /// `workspace_path` is what the client sent, if anything. Memory scoping
    /// falls back to the daemon's CWD so a developer's CLI turn still lands in
    /// that project's memory; artifact placement does not, so
    /// `request_workspace_root` is `Some` only on the client-sent branch.
    pub fn for_request(workspace_path: Option<&str>) -> Self {
        match workspace_path {
            Some(path) => {
                let root =
                    crate::memory::workspace::resolve_workspace_id(std::path::Path::new(path));
                Self {
                    workspace_id: root.clone(),
                    request_workspace_root: root,
                }
            }
            None => {
                tracing::debug!("No workspace_path in request, falling back to daemon CWD");
                let workspace_id = std::env::current_dir()
                    .ok()
                    .and_then(|d| crate::memory::workspace::resolve_workspace_id(&d));
                Self {
                    workspace_id,
                    request_workspace_root: None,
                }
            }
        }
    }

    /// Recover the request's workspace context from a `ToolContext` that was
    /// built from one.
    pub fn from_tool_context(ctx: &crate::tools::registry::ToolContext) -> Self {
        Self {
            workspace_id: ctx.workspace_id.clone(),
            request_workspace_root: ctx.request_workspace_root.clone(),
        }
    }

    /// Create a scope context with no workspace (Global-only).
    pub fn global_only() -> Self {
        Self::default()
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

    /// R22, at the request path's own rule (`handlers.rs` calls exactly this).
    /// A turn with no client workspace — every connector lane and every
    /// scheduled skill — still gets a CWD-derived id for memory, and must get
    /// **no** request root, whatever the daemon's current directory looks like.
    /// This test process runs inside a `.git` checkout, so the CWD walk does
    /// find a root: the point is that it stays out of the second field.
    #[test]
    fn for_request_without_a_workspace_carries_no_request_root() {
        let ctx = MemoryScopeContext::for_request(None);
        assert_eq!(
            ctx.request_workspace_root, None,
            "the daemon CWD must never become a request workspace root"
        );
        assert_eq!(
            ctx.workspace_id,
            std::env::current_dir()
                .ok()
                .and_then(|d| crate::memory::workspace::resolve_workspace_id(&d)),
            "memory scoping keeps its CWD fallback"
        );
    }

    #[test]
    fn for_request_with_a_workspace_carries_both() {
        let project = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(project.path().join(".git")).unwrap();
        let root = project.path().canonicalize().unwrap();

        let ctx = MemoryScopeContext::for_request(project.path().to_str());
        assert_eq!(ctx.workspace_id.as_deref(), root.to_str());
        assert_eq!(ctx.request_workspace_root.as_deref(), root.to_str());
    }
}
