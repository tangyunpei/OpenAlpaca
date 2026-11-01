//! Shared helpers for workspace path validation, file path resolution,
//! protected-file detection, and backup management.

use std::path::{Path, PathBuf};

use openalpaca_storage::store::projects;

use crate::tools::registry::ToolContext;
use crate::tools::working_folder::{WorkingFolder, WorkingFolderRefusal, working_folder};

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
    /// No rule beyond confinement: a conversation's scratch folder.
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

/// [`acting_folder`] for an approval card (R34): the folder named, never
/// made — a conversation that never acts gets no scratch folder, and a file
/// in a scratch folder not made yet does not exist either.
pub(super) fn named_folder(ctx: &ToolContext) -> Result<(PathBuf, FolderRules), String> {
    let folder = working_folder(ctx).map_err(|refusal| refusal.to_string())?;
    Ok((folder.path().to_path_buf(), FolderRules::of(&folder)))
}

/// Resolve a relative path within the workspace and verify it doesn't escape
/// the workspace boundary via symlinks or normalization.
///
/// `workspace_root` is the call's working folder — the project's root, or
/// the conversation's scratch folder (design §10, [`acting_folder`]) — never
/// the process-global `current_dir()`.
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
/// sees, and `link -> /anywhere` plus a new directory resolves outside the
/// root and is refused (S3-R1, plan §7a). The missing directories cannot
/// redirect anything: they do not exist, and `validate_workspace_path`
/// refused `..`.
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

/// The refusal of a path that leaves the working folder — the resolvers'
/// sentence, and the one an open of their target gives for a link found at
/// its leaf ([`open_leaf`]).
pub(super) fn outside_boundary(relative_path: &str) -> String {
    format!("Path '{relative_path}' resolves outside the workspace boundary")
}

/// Opens `target` — a resolver's answer for `relative_path` — without
/// following a link at its leaf (the lead's ruling R-T30, d1-confine-1):
/// `O_NOFOLLOW` on unix, as the memory writer's opens do (`fsx.rs`). The
/// resolvers canonicalized the target, so no link stands at its leaf when
/// they answer; a link found there when it is opened was put there since,
/// and following it could read or write a file outside the folder. That
/// open fails (`ELOOP`) and is refused with [`outside_boundary`]'s
/// sentence; any other failure is `other(e)`. A link at a *middle* folder
/// is still followed by the open — closing that needs a walk with `openat`
/// (recorded, not built). Elsewhere than unix the options are as given.
pub(super) async fn open_leaf(
    options: &mut tokio::fs::OpenOptions,
    target: &Path,
    relative_path: &str,
    other: impl FnOnce(std::io::Error) -> String,
) -> Result<tokio::fs::File, String> {
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    open_leaf_as_set(options, target, relative_path, other).await
}

/// [`open_leaf`] for a read that must not wait (residual X6): on unix the
/// open also carries `O_NONBLOCK`, so a named pipe's open returns at once
/// where it would wait for a writer that may never come, and a device's
/// open does not wait for its line. The caller then judges the opened file
/// by its own `fstat` — no window between a check and the open — and reads
/// only a regular file, whose reads `O_NONBLOCK` does not change.
pub(super) async fn open_leaf_without_waiting(
    options: &mut tokio::fs::OpenOptions,
    target: &Path,
    relative_path: &str,
    other: impl FnOnce(std::io::Error) -> String,
) -> Result<tokio::fs::File, String> {
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    open_leaf_as_set(options, target, relative_path, other).await
}

/// The open both [`open_leaf`]s share, with their flags already set.
async fn open_leaf_as_set(
    options: &tokio::fs::OpenOptions,
    target: &Path,
    relative_path: &str,
    other: impl FnOnce(std::io::Error) -> String,
) -> Result<tokio::fs::File, String> {
    match options.open(target).await {
        Ok(file) => Ok(file),
        Err(e) if is_link_at_leaf(&e) => Err(outside_boundary(relative_path)),
        Err(e) => Err(other(e)),
    }
}

/// The folders above `target` that do not exist yet, made below the
/// project's `root` one level at a time and never `root` itself (the lead's
/// ruling R-T30, d5-results-2; `store::create_below`'s rules): a level that
/// is a link or a file is refused, not followed, and a root that is gone —
/// moved away since the plan — makes nothing and is refused with the missing
/// project's sentence. `create_dir_all` re-made such a root, and the write
/// landed in a stub folder that then read as the project, present.
/// `target` is the write resolver's answer, so it lies under `root`; one
/// that does not is refused rather than made. Any other failure to make a
/// folder is `other`'s to word, as [`open_leaf`]'s is — the calling tool's
/// own hint goes there (N-TOOL-4).
pub(super) fn make_parents_below(
    relative_path: &str,
    root: &Path,
    target: &Path,
    other: impl FnOnce(std::io::Error) -> String,
) -> Result<(), String> {
    let below = target
        .parent()
        .and_then(|parent| parent.strip_prefix(root).ok())
        .ok_or_else(|| outside_boundary(relative_path))?;
    match openalpaca_storage::store::create_below(root, below) {
        Ok(_) => Ok(()),
        Err(_) if projects::root_is_missing(root) => Err(WorkingFolderRefusal::ProjectMissing {
            root: root.to_string_lossy().into_owned(),
        }
        .to_string()),
        Err(e) => Err(other(e)),
    }
}

/// Whether an `O_NOFOLLOW` open failed because a link stands at the leaf.
fn is_link_at_leaf(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        e.raw_os_error() == Some(libc::ELOOP)
    }
    #[cfg(not(unix))]
    {
        let _ = e;
        false
    }
}

/// The file a write or an edit changes, relative to the project's `root`,
/// when the path the model wrote reaches it through a link inside the
/// project (the lead's ruling R-T31, d1-confine-2, d3-approvals-3): the
/// write resolver's `target` is then not `root.join(relative_path)`, and
/// the card names it, so a person approves the file that changes. `None`
/// when the path names its file itself — `./` and doubled separators
/// included (`Path`'s comparison drops them), and a spelling the volume
/// takes for the same file (another letter case, another Unicode form),
/// which resolves to the file's own spelling but goes through no link.
pub(super) fn linked_target(relative_path: &str, root: &Path, target: &Path) -> Option<String> {
    if target == root.join(relative_path) || !through_a_link(relative_path, root) {
        return None;
    }
    Some(
        openalpaca_storage::store::relative_to(root, target)
            .unwrap_or_else(|_| target.display().to_string()),
    )
}

/// Whether a component of `relative_path` under `root` that exists is a
/// link — the walk stops at the first that does not exist.
fn through_a_link(relative_path: &str, root: &Path) -> bool {
    let mut at = root.to_path_buf();
    for component in Path::new(relative_path).components() {
        let std::path::Component::Normal(name) = component else {
            continue;
        };
        at.push(name);
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

/// Which way a builtin file tool is touching a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StoreAccess {
    Read,
    Write,
}

/// The store's own files, which `workspace_write` refuses as an entry key: a
/// key names the entry and, spilled, the artifact's path (memory redesign,
/// design §2; plan 3.2). The file tools and `send` no longer ask (storage
/// design §11, R33): they act in a project or a scratch folder, where none
/// of these files is reachable.
///
/// - any path whose first component is `state` — the control snapshots,
///   the database, the discovery file — is refused on every access;
/// - a memory partition's control files — a `.history/` component, a
///   `MEMORY.md` and a `.gitignore` directly under a `memory/` directory —
///   are refused on write only.
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
