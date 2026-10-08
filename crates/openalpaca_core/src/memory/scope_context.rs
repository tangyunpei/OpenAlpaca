//! Workspace scope context: carries workspace identity through the request lifecycle.
//!
//! Created once in `handle_message()` and threaded to the call sites that need
//! the request's workspace: the memory service resolves its own partition from
//! `request_workspace_root` (design §2), and artifact placement reads it too.
//!
//! Two fields travel here, because their consumers differ and conflating them
//! is the defect ruling R22 fixed:
//!
//! - [`workspace_id`](MemoryScopeContext::workspace_id) is what a turn's tools
//!   carry for **memory** scoping (`ToolContext::workspace_id`).
//! - [`request_workspace_root`](MemoryScopeContext::request_workspace_root)
//!   scopes **content the request owns** (artifacts) and is what a run
//!   records.
//!
//! Both are the root of the path a client sent, and `None` when it sent none —
//! connector lanes, scheduled skills and every other turn without one. The
//! daemon's current directory is never read (R22; the lead's R-S13: the
//! resolver takes the path a client sent, storage design §3), so on a context
//! built from a request the two are equal.
//!
//! The root comes from one resolver, `openalpaca_storage::store::projects::resolve`
//! (storage design §3): walking up from the path, the nearest folder that is a
//! registered project's root or holds `.git` — never the home store, a folder
//! inside it or a folder containing it (R13). `.openalpaca` is no marker. The
//! resolver keeps no cache and creates nothing.

use std::path::Path;

use openalpaca_storage::store::projects;

/// Carries workspace scope context for a request.
#[derive(Debug, Clone, Default)]
pub struct MemoryScopeContext {
    /// Detected workspace root path as scope_id. `None` if no workspace detected
    /// (e.g., a request that named none, or from Telegram).
    ///
    /// Memory scoping: the root the client's path resolved to — the same value
    /// as `request_workspace_root` on a context built from a request, and
    /// never derived from the daemon's current directory (R-S13).
    pub workspace_id: Option<String>,
    /// The project root the *request* supplied — `x-workspace-path` on
    /// `/v1/chat`, `workspace_path` on `/v1/command` — resolved to its root.
    /// `None` whenever no client sent one; never CWD-derived (R22), and never
    /// a root that is, lies inside or contains the home store (R13).
    ///
    /// This is the only field that may place a file on disk, and it is what a
    /// dispatched run records as its `task.workspace_id`.
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
    /// `workspace_path` is what the client sent, if anything, and both fields
    /// are the root it resolves to. A turn that sent none has no project on
    /// either field: nothing is read from the daemon's current directory (the
    /// lead's R-S13). The CLI always sends its working directory; the turns
    /// with none are the connector lanes, the scheduled skills and an app
    /// window with no project chosen.
    ///
    /// **The home store is not a workspace on either field** (R13). The
    /// resolver answers no project for the home store, for any folder inside
    /// it and for any folder containing it. Nothing that writes a file reads
    /// `workspace_id`: the task-output extraction at `dispatcher/lead_agent.rs`
    /// takes `request_workspace_root`, like `task.workspace_id`.
    ///
    /// Pure: it records nothing, so a read — the status poll, the Memory
    /// view — may call it freely.
    pub fn for_request(workspace_path: Option<&str>) -> Self {
        let root = workspace_path.and_then(|path| resolved_root(Path::new(path)));
        Self {
            workspace_id: root.clone(),
            request_workspace_root: root,
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

    /// Whether a workspace context is available.
    pub fn has_workspace(&self) -> bool {
        self.workspace_id.is_some()
    }
}

/// The project root [`projects::resolve`] finds for `path`, registered or
/// not, as the string the rows hold. An error reading `projects/` reads as no
/// project, logged at debug: the turn still runs, without one.
fn resolved_root(path: &Path) -> Option<String> {
    match projects::resolve(path) {
        Ok(found) => found.map(|r| r.root_string()),
        Err(e) => {
            tracing::debug!(
                path = %path.display(),
                "resolving a workspace path failed, so it names no project: {e:#}"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_workspace() {
        assert!(MemoryScopeContext::new(Some("/ws".to_string())).has_workspace());
        assert!(!MemoryScopeContext::global_only().has_workspace());
    }

    /// R22 and the lead's R-S13: a turn whose client sent no path has no
    /// project on either field, even when the process's current directory
    /// lies in a git repository — nothing is resolved from it. The test runs
    /// on its own working directory (cargo's package directory, inside the
    /// checkout) rather than a changed one: the current directory is
    /// process-global. The guard keeps a regressed fallback reading a temp
    /// home store, never the real one.
    #[test]
    fn a_turn_with_no_path_has_no_project_even_inside_a_git_repository() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home_store = tmp.path().canonicalize().unwrap().join(".openalpaca");
        std::fs::create_dir_all(&home_store).unwrap();
        let _guard = crate::test_util::HomeStoreGuard::set(&home_store);
        let cwd = std::env::current_dir().unwrap();
        assert!(
            cwd.ancestors().any(|dir| dir.join(".git").exists()),
            "precondition: this test runs inside a git checkout ({})",
            cwd.display()
        );

        let ctx = MemoryScopeContext::for_request(None);
        assert_eq!(
            ctx.request_workspace_root, None,
            "the daemon CWD must never become a request workspace root"
        );
        assert_eq!(ctx.workspace_id, None, "nor a memory workspace");
    }

    /// T24 re-review carry-over, now R13: a client path under `$HOME` with no
    /// closer project walks up to `$HOME`, which contains the home store, so
    /// it is no project — the request root is `None` and content takes the
    /// home store, as it does for a path naming the home store itself.
    #[test]
    fn a_path_resolving_to_the_home_store_is_not_a_project() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().canonicalize().unwrap().join("home");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(home.join(".openalpaca")).unwrap();
        let _guard = crate::test_util::HomeStoreGuard::set(&home.join(".openalpaca"));

        // A directory under `$HOME` carrying no marker of its own.
        let loose = home.join("Documents");
        std::fs::create_dir(&loose).unwrap();

        let ctx = MemoryScopeContext::for_request(loose.to_str());
        assert_eq!(
            ctx.request_workspace_root, None,
            "$HOME is not a project — placement must fall back to the home store"
        );
        // And it is not a memory workspace either: a `Workspace($HOME)` scope is
        // one nothing else reads (final review D3).
        assert_eq!(
            ctx.workspace_id, None,
            "$HOME is not a workspace for memory scoping either"
        );

        // The home store root itself is no project either (R13).
        let ctx = MemoryScopeContext::for_request(home.join(".openalpaca").to_str());
        assert_eq!(ctx.request_workspace_root, None);
        assert_eq!(ctx.workspace_id, None);
    }

    /// The fold is narrow: a real project under the home directory keeps its
    /// own store.
    #[test]
    fn a_real_project_under_home_is_still_a_project() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().canonicalize().unwrap().join("home");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(home.join(".openalpaca")).unwrap();
        let _guard = crate::test_util::HomeStoreGuard::set(&home.join(".openalpaca"));

        let project = home.join("code").join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir(project.join(".git")).unwrap();

        let ctx = MemoryScopeContext::for_request(project.to_str());
        assert_eq!(ctx.request_workspace_root.as_deref(), project.to_str());
    }

    /// R13 (storage design §3): a root that contains the home store is never
    /// a project. Before, only a root whose own store *was* the home store
    /// folded, so a repository the home store sat inside answered for every
    /// folder beside it.
    #[test]
    fn a_git_repository_that_contains_the_home_store_is_no_project() {
        let tmp = tempfile::TempDir::new().unwrap();
        let outer = tmp.path().canonicalize().unwrap();
        std::fs::create_dir(outer.join(".git")).unwrap();
        let home_store = outer.join("home").join(".openalpaca");
        std::fs::create_dir_all(&home_store).unwrap();
        let _guard = crate::test_util::HomeStoreGuard::set(&home_store);
        let beside = outer.join("notes");
        std::fs::create_dir(&beside).unwrap();

        let ctx = MemoryScopeContext::for_request(beside.to_str());
        assert_eq!(
            ctx.request_workspace_root, None,
            "the repository holds the home store"
        );
        assert_eq!(ctx.workspace_id, None);
    }

    /// ST-9 and storage design §3: an in-project `.openalpaca` is never read
    /// and marks nothing — a folder holding only one is no project.
    #[test]
    fn a_folder_holding_only_an_openalpaca_directory_is_no_project() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home_store = root.join("home").join(".openalpaca");
        std::fs::create_dir_all(&home_store).unwrap();
        let _guard = crate::test_util::HomeStoreGuard::set(&home_store);
        let old = root.join("work").join("old");
        std::fs::create_dir_all(old.join(".openalpaca")).unwrap();

        let ctx = MemoryScopeContext::for_request(old.to_str());
        assert_eq!(ctx.request_workspace_root, None);
        assert_eq!(ctx.workspace_id, None);
    }

    /// R22: a relative workspace path is never read against the daemon's
    /// working directory. Cargo runs this test from the package directory,
    /// inside the repository's checkout, where `src` exists.
    #[test]
    fn a_relative_workspace_path_is_no_project() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home_store = tmp.path().canonicalize().unwrap().join(".openalpaca");
        std::fs::create_dir_all(&home_store).unwrap();
        let _guard = crate::test_util::HomeStoreGuard::set(&home_store);
        assert!(Path::new("src").is_dir());

        let ctx = MemoryScopeContext::for_request(Some("src"));
        assert_eq!(ctx.request_workspace_root, None);
        assert_eq!(ctx.workspace_id, None);
    }

    /// Storage design §3's table: a folder with no git that was chosen is the
    /// project of every folder below it.
    #[test]
    fn a_registered_folder_with_no_git_is_the_project_of_its_subfolders() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home_store = root.join("home").join(".openalpaca");
        std::fs::create_dir_all(&home_store).unwrap();
        let _guard = crate::test_util::HomeStoreGuard::set(&home_store);
        let notes = root.join("work").join("notes");
        let below = notes.join("2026");
        std::fs::create_dir_all(&below).unwrap();
        projects::register(&notes).unwrap();

        let ctx = MemoryScopeContext::for_request(below.to_str());
        assert_eq!(ctx.request_workspace_root.as_deref(), notes.to_str());
        assert_eq!(ctx.workspace_id.as_deref(), notes.to_str());
    }

    #[test]
    fn for_request_with_a_workspace_carries_both() {
        let project = tempfile::TempDir::new().unwrap();
        let home = tempfile::TempDir::new().unwrap();
        let home_store = home.path().canonicalize().unwrap().join(".openalpaca");
        std::fs::create_dir_all(&home_store).unwrap();
        let _guard = crate::test_util::HomeStoreGuard::set(&home_store);
        std::fs::create_dir(project.path().join(".git")).unwrap();
        let root = project.path().canonicalize().unwrap();

        let ctx = MemoryScopeContext::for_request(project.path().to_str());
        assert_eq!(ctx.workspace_id.as_deref(), root.to_str());
        assert_eq!(ctx.request_workspace_root.as_deref(), root.to_str());
    }
}
