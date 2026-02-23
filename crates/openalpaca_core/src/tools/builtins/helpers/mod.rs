//! Shared helpers for workspace path validation, file path resolution,
//! protected-file detection, and backup management.

use std::path::PathBuf;

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
/// Returns the canonicalized absolute path on success.
pub(super) fn resolve_workspace_path(relative_path: &str) -> Result<PathBuf, String> {
    validate_workspace_path(relative_path)?;

    let workspace_root = std::env::current_dir()
        .map_err(|e| format!("Cannot determine working directory: {}", e))?;

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
pub(super) fn resolve_workspace_path_for_write(relative_path: &str) -> Result<PathBuf, String> {
    validate_workspace_path(relative_path)?;

    let workspace_root = std::env::current_dir()
        .map_err(|e| format!("Cannot determine working directory: {}", e))?;

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
        if !full_path.starts_with(&workspace_root) {
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

/// Generate a unique backup path with nanosecond timestamp + collision suffix.
///
/// Format: `SOUL.20260211T153042.123456789Z.md`
/// Collision: `SOUL.20260211T153042.123456789Z.1.md`, `.2.md`, ...
pub(super) fn unique_backup_path(backup_dir: &std::path::Path) -> PathBuf {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S.%9fZ");
    let base_name = format!("SOUL.{}.md", ts);
    let candidate = backup_dir.join(&base_name);
    if !candidate.exists() {
        return candidate;
    }
    for suffix in 1..1000 {
        let name = format!("SOUL.{}.{}.md", ts, suffix);
        let candidate = backup_dir.join(&name);
        if !candidate.exists() {
            return candidate;
        }
    }
    // Fallback: UUID (uuid crate already in openalpaca_core deps)
    backup_dir.join(format!("SOUL.{}.{}.md", ts, uuid::Uuid::new_v4()))
}

/// Prune old backups in `backup_dir`, keeping at most `max` files.
///
/// Backups use timestamped names (`SOUL.<ISO-timestamp>.md`), so lexicographic
/// sorting orders them oldest-first. We remove the oldest entries that exceed
/// the retention limit. Errors are logged but never propagated — pruning
/// failure must not break the update flow.
pub(super) async fn prune_backups(backup_dir: &std::path::Path, max: usize) {
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(backup_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.starts_with("SOUL.") && n.ends_with(".md"))
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
