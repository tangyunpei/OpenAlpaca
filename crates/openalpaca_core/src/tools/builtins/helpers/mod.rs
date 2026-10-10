//! Shared helpers for workspace path validation, file path resolution,
//! protected-file detection, and backup management.

use std::path::{Path, PathBuf};

use crate::tools::registry::ToolContext;
use crate::tools::working_folder::{WorkingFolder, working_folder};

/// Maximum file size for file_read (10 MB).
pub(super) const MAX_FILE_READ_SIZE: u64 = 10 * 1024 * 1024;

/// Validate that a path is safe for workspace-scoped file operations.
/// Rejects absolute paths and paths containing `..` components.
pub(super) fn validate_workspace_path(path: &str) -> Result<(), String> {
    let path_buf = PathBuf::from(path);
    if path_buf.is_absolute() {
        return Err(
            "Absolute paths are not allowed. Use relative paths within the workspace.".to_string(),
        );
    }
    for component in path_buf.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err("Path traversal ('..') is not allowed.".to_string());
        }
    }
    Ok(())
}

/// What R32 adds to confinement (storage design §11): the rules a folder
/// carries beyond "relative, no `..`, resolved under the root".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FolderRules {
    /// The folder is a project (ST-10): a component named `.git` is refused
    /// for writing — git keeps that folder, and reading it stays allowed
    /// (ST-16) — and one named `.openalpaca` for reading and for writing, an
    /// older OpenAlpaca's folder nothing reads any more (ST-9). Any
    /// component, compared without case, of the path relative to the
    /// canonical root **after** resolution, so a link into either folder is
    /// judged by where it lands.
    Project,
    /// No rule beyond confinement: a conversation's scratch folder, and until
    /// S3 and S4a every caller whose root is the daemon's starting directory.
    Plain,
}

impl FolderRules {
    /// R32's rules for a working folder: [`FolderRules::Project`] in a
    /// project, [`FolderRules::Plain`] in a conversation's scratch folder.
    pub(super) fn of(folder: &WorkingFolder) -> Self {
        if folder.is_project() {
            FolderRules::Project
        } else {
            FolderRules::Plain
        }
    }

    /// R32 over `resolved`, the path a resolver landed on. The resolvers
    /// refused anything outside `canonical_root` first, so the fallback below
    /// only keeps this fail-closed.
    fn check(
        self,
        relative_path: &str,
        resolved: &Path,
        canonical_root: &Path,
        access: StoreAccess,
    ) -> Result<(), String> {
        if self == FolderRules::Plain {
            return Ok(());
        }
        let Ok(inside) = resolved.strip_prefix(canonical_root) else {
            return Err(format!(
                "Path '{relative_path}' resolves outside the workspace boundary"
            ));
        };
        let has = |name: &str| {
            inside.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::Normal(part) if part.eq_ignore_ascii_case(name)
                )
            })
        };
        if has(".openalpaca") {
            return Err(format!(
                "'{relative_path}' is refused: a .openalpaca folder inside a project is an older \
                 OpenAlpaca's, and nothing in it is read or written any more."
            ));
        }
        if access == StoreAccess::Write && has(".git") {
            return Err(format!(
                "Writing '{relative_path}' is refused: it is inside the project's .git folder, \
                 which git keeps. Run git through shell_execute instead."
            ));
        }
        Ok(())
    }
}

/// R26 (storage design §10): the call's working folder, ready to act in — a
/// scratch folder is made on first use (`WorkingFolder::ensure`) — and R32's
/// rules for it. `Err` is the refusal's own sentence (R26's no context, R27's
/// missing project, R44's neither project nor conversation): nothing was
/// read, made or run.
pub(super) fn acting_folder(ctx: &ToolContext) -> Result<(PathBuf, FolderRules), String> {
    let folder = working_folder(ctx).map_err(|refusal| refusal.to_string())?;
    let root = folder
        .ensure()
        .map_err(|refusal| refusal.to_string())?
        .to_path_buf();
    Ok((root, FolderRules::of(&folder)))
}

/// Resolve a relative path within the workspace and verify it doesn't escape
/// the workspace boundary via symlinks or normalization.
///
/// `workspace_root` is the explicit workspace directory (captured once at
/// startup). This avoids depending on the process-global `current_dir()`,
/// which can change or be unavailable.
///
/// Returns the canonicalized absolute path on success — every component, the
/// last included, resolved (R31's read side) — after R32's read rule when
/// `rules` is [`FolderRules::Project`].
pub(super) fn resolve_workspace_path(
    relative_path: &str,
    workspace_root: &Path,
    rules: FolderRules,
) -> Result<PathBuf, String> {
    validate_workspace_path(relative_path)?;

    let full_path = workspace_root.join(relative_path);

    // Canonicalize to resolve symlinks. For reads the file must exist;
    // for writes the parent must exist (caller handles dir creation).
    let canonical = full_path
        .canonicalize()
        .map_err(|e| format!("Path resolution failed for '{}': {}", relative_path, e))?;

    let canonical_root = workspace_root
        .canonicalize()
        .map_err(|e| format!("Workspace root canonicalization failed: {}", e))?;

    if !canonical.starts_with(&canonical_root) {
        return Err(format!(
            "Path '{}' resolves outside the workspace boundary",
            relative_path
        ));
    }
    rules.check(relative_path, &canonical, &canonical_root, StoreAccess::Read)?;

    Ok(canonical)
}

/// Like `resolve_workspace_path` but for write targets where the file, and
/// any of the directories above it, may not exist yet (the caller creates
/// them).
///
/// Resolves the **nearest existing ancestor** of the parent directory — the
/// parent itself when it exists — and re-appends the missing directories and
/// the file name, so the path returned is where `create_dir_all` and the
/// write will actually land. A link on the way is therefore followed here,
/// not only later by the caller: `link -> state` plus `link/newdir/x.json`
/// resolves to `<root>/state/newdir/x.json`, which the boundary check below
/// and the caller's store check (`is_store_owned_resolved`) both see, and
/// `link -> /anywhere` plus a new directory resolves outside the root and is
/// refused (S3-R1, plan §7a). The missing directories cannot redirect
/// anything: they do not exist, and `validate_workspace_path` refused `..`.
///
/// "Exists" means a directory entry is there — `symlink_metadata`, not
/// `exists`, which answers no for a dangling link and would let the walk
/// step past it; a dangling link then fails to canonicalize and the write
/// is refused. The leaf is resolved the same way (R31, storage design §11):
/// an entry already standing there — a file, a folder, a link — is
/// canonicalized whole and must stay under the root, as
/// [`resolve_workspace_path`] does for a read, so a link at the leaf that
/// leaves the root is refused instead of written through, and the path
/// returned for a link inside the root is its target. Then R32's write rule
/// when `rules` is [`FolderRules::Project`].
pub(super) fn resolve_workspace_path_for_write(
    relative_path: &str,
    workspace_root: &Path,
    rules: FolderRules,
) -> Result<PathBuf, String> {
    validate_workspace_path(relative_path)?;

    let full_path = workspace_root.join(relative_path);
    let parent = full_path
        .parent()
        .ok_or_else(|| "Invalid path: no parent directory".to_string())?;
    let file_name = full_path
        .file_name()
        .ok_or_else(|| "Invalid path: no file name".to_string())?;

    let canonical_root = workspace_root
        .canonicalize()
        .map_err(|e| format!("Workspace root canonicalization failed: {}", e))?;

    let resolution_failed = |e: std::io::Error| {
        format!(
            "Parent directory resolution failed for '{}': {}",
            relative_path, e
        )
    };
    let mut existing = parent;
    loop {
        match std::fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                existing = existing
                    .parent()
                    .ok_or_else(|| "Invalid path: no existing ancestor".to_string())?;
            }
            Err(e) => return Err(resolution_failed(e)),
        }
    }
    let missing = parent
        .strip_prefix(existing)
        .map_err(|_| "Invalid path: ancestor is not a prefix".to_string())?;
    let resolved_parent = existing
        .canonicalize()
        .map_err(resolution_failed)?
        .join(missing);

    if !resolved_parent.starts_with(&canonical_root) {
        return Err(format!(
            "Path '{}' resolves outside the workspace boundary",
            relative_path
        ));
    }
    // R31: the leaf is resolved like every other component. Nothing there:
    // the leaf is new and cannot redirect anything.
    let leaf = resolved_parent.join(file_name);
    let leaf_failed =
        |e: std::io::Error| format!("Path resolution failed for '{relative_path}': {e}");
    let target = match std::fs::symlink_metadata(&leaf) {
        Ok(_) => leaf.canonicalize().map_err(leaf_failed)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => leaf,
        Err(e) => return Err(leaf_failed(e)),
    };
    if !target.starts_with(&canonical_root) {
        return Err(format!(
            "Path '{relative_path}' resolves outside the workspace boundary"
        ));
    }
    rules.check(relative_path, &target, &canonical_root, StoreAccess::Write)?;
    Ok(target)
}

/// Check if a path refers to SOUL.md (case-insensitive filename check).
pub(super) fn is_soul_path(path: &str) -> bool {
    std::path::Path::new(path)
        .file_name()
        .and_then(|f| f.to_str())
        .map(|f| f.eq_ignore_ascii_case("soul.md"))
        .unwrap_or(false)
}

/// Check if a path refers to USER.md (case-insensitive filename check).
pub(super) fn is_user_path(path: &str) -> bool {
    std::path::Path::new(path)
        .file_name()
        .and_then(|f| f.to_str())
        .map(|f| f.eq_ignore_ascii_case("user.md"))
        .unwrap_or(false)
}

/// Which way a builtin file tool is touching a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StoreAccess {
    Read,
    Write,
}

/// The store's own files, which the generic file tools refuse (memory
/// redesign, design §2; plan 3.2). The builtin registry captures the
/// daemon's CWD as its workspace root and both launchers start the daemon
/// in the home store, so a relative path reaches the store directly:
///
/// - any path whose first component is `state` — the control snapshots,
///   the database, the discovery file — is refused on every access
///   (`file_read` would disclose the owner-id mapping);
/// - a memory partition's control files — a `.history/` component, a
///   `MEMORY.md` and a `.gitignore` directly under a `memory/` directory —
///   are refused on write only, so a hand-dropped note stays possible and a
///   read of a kept revision stays possible.
///
/// Returns what was matched, for the refusal message. Names are compared
/// case-insensitively: the platforms with a case-insensitive file system
/// would reach the same file either way.
pub(super) fn is_store_owned_path(path: &str, access: StoreAccess) -> Option<&'static str> {
    let components: Vec<&str> = Path::new(path)
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect();
    if components
        .first()
        .is_some_and(|first| first.eq_ignore_ascii_case("state"))
    {
        return Some("the store's `state/` directory");
    }
    if access == StoreAccess::Read {
        return None;
    }
    if components
        .iter()
        .any(|part| part.eq_ignore_ascii_case(".history"))
    {
        return Some("a memory partition's `.history/`");
    }
    match components.as_slice() {
        [.., name] if name.eq_ignore_ascii_case("MEMORY.md") => {
            Some("a memory partition's `MEMORY.md`")
        }
        [.., dir, name]
            if dir.eq_ignore_ascii_case("memory") && name.eq_ignore_ascii_case(".gitignore") =>
        {
            Some("a memory partition's `memory/.gitignore`")
        }
        _ => None,
    }
}

/// [`is_store_owned_path`] over the path a resolution landed on, relative
/// to the workspace root. The argument check runs first, on the string;
/// this one runs after `resolve_workspace_path[_for_write]`, because a
/// symlink inside the workspace that points at `state/` (or at a
/// partition's control files) canonicalizes under the root and would
/// otherwise pass. A resolved path that is not under the canonical root —
/// the resolvers refuse that already — matches nothing here.
pub(super) fn is_store_owned_resolved(
    resolved: &Path,
    workspace_root: &Path,
    access: StoreAccess,
) -> Option<&'static str> {
    let root = workspace_root.canonicalize().ok()?;
    let relative = resolved.strip_prefix(&root).ok()?;
    is_store_owned_path(&relative.to_string_lossy(), access)
}

/// Check if a path refers to IDENTITY.md (case-insensitive filename check).
pub(super) fn is_identity_path(path: &str) -> bool {
    std::path::Path::new(path)
        .file_name()
        .and_then(|f| f.to_str())
        .map(|f| f.eq_ignore_ascii_case("identity.md"))
        .unwrap_or(false)
}

/// Generate a unique backup path with nanosecond timestamp and UUID suffix.
///
/// Format: `<PREFIX>.20260211T153042.123456789Z.<uuid8>.md`
///
/// The 8-character UUID suffix guarantees uniqueness without filesystem
/// checks, eliminating the TOCTOU race present in the old exists()-based
/// approach. Lexicographic sort still orders by timestamp first (the UUID
/// suffix only breaks ties within the same nanosecond).
///
/// `prefix` is the document type (e.g. "SOUL", "USER", "IDENTITY").
pub(super) fn unique_backup_path(backup_dir: &std::path::Path, prefix: &str) -> PathBuf {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S.%9fZ");
    let uuid_suffix = &uuid::Uuid::new_v4().to_string()[..8];
    let name = format!("{}.{}.{}.md", prefix, ts, uuid_suffix);
    backup_dir.join(name)
}

/// Prune old backups in `backup_dir`, keeping at most `max` files.
///
/// Only considers files matching `<PREFIX>.*\.md`. Lexicographic sorting
/// orders them oldest-first. We remove the oldest entries that exceed
/// the retention limit. Errors are logged but never propagated — pruning
/// failure must not break the update flow.
///
/// `prefix` is the document type (e.g. "SOUL", "USER", "IDENTITY").
pub(super) async fn prune_backups(backup_dir: &std::path::Path, max: usize, prefix: &str) {
    let prefix_dot = format!("{}.", prefix);
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(backup_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.starts_with(&prefix_dot) && n.ends_with(".md"))
                    .unwrap_or(false)
            })
            .map(|e| e.path())
            .collect(),
        Err(_) => return,
    };

    if entries.len() <= max {
        return;
    }

    // Sort alphabetically → oldest timestamps first
    entries.sort();

    let to_remove = entries.len() - max;
    for path in entries.into_iter().take(to_remove) {
        if let Err(e) = tokio::fs::remove_file(&path).await {
            tracing::warn!("Failed to prune backup {}: {}", path.display(), e);
        }
    }
}

#[cfg(test)]
mod tests;
