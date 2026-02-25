//! Shared helpers for workspace path validation, file path resolution,
//! protected-file detection, and backup management.

use std::path::{Path, PathBuf};

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

/// Resolve a relative path within the workspace and verify it doesn't escape
/// the workspace boundary via symlinks or normalization.
///
/// `workspace_root` is the explicit workspace directory (captured once at
/// startup). This avoids depending on the process-global `current_dir()`,
/// which can change or be unavailable.
///
/// Returns the canonicalized absolute path on success.
pub(super) fn resolve_workspace_path(
    relative_path: &str,
    workspace_root: &Path,
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

    Ok(canonical)
}

/// Like `resolve_workspace_path` but for write targets where the file may not
/// yet exist. Canonicalizes the parent directory and verifies it is within
/// the workspace, then appends the file name.
pub(super) fn resolve_workspace_path_for_write(
    relative_path: &str,
    workspace_root: &Path,
) -> Result<PathBuf, String> {
    validate_workspace_path(relative_path)?;

    let full_path = workspace_root.join(relative_path);

    // For new files, canonicalize the parent directory
    let parent = full_path
        .parent()
        .ok_or_else(|| "Invalid path: no parent directory".to_string())?;

    let canonical_root = workspace_root
        .canonicalize()
        .map_err(|e| format!("Workspace root canonicalization failed: {}", e))?;

    // If parent exists, canonicalize it; otherwise fall back to the joined path
    // (parent dirs will be created by the caller)
    if parent.exists() {
        let canonical_parent = parent.canonicalize().map_err(|e| {
            format!(
                "Parent directory resolution failed for '{}': {}",
                relative_path, e
            )
        })?;

        if !canonical_parent.starts_with(&canonical_root) {
            return Err(format!(
                "Path '{}' resolves outside the workspace boundary",
                relative_path
            ));
        }

        // Re-append the file name to the canonical parent
        let file_name = full_path
            .file_name()
            .ok_or_else(|| "Invalid path: no file name".to_string())?;
        Ok(canonical_parent.join(file_name))
    } else {
        // Parent doesn't exist yet — the caller will create it.
        // Since validate_workspace_path already rejected .. components,
        // and the parent hasn't been created yet (so no symlinks to resolve),
        // we can use the non-canonical path. Double-check it starts with the root.
        if !full_path.starts_with(workspace_root) {
            return Err(format!(
                "Path '{}' resolves outside the workspace boundary",
                relative_path
            ));
        }
        Ok(full_path)
    }
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
