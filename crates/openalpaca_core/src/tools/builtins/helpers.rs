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
    let canonical = full_path.canonicalize().map_err(|e| {
        format!(
            "Path resolution failed for '{}': {}",
            relative_path, e
        )
    })?;

    let canonical_root = workspace_root.canonicalize().map_err(|e| {
        format!("Workspace root canonicalization failed: {}", e)
    })?;

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

    let canonical_root = workspace_root.canonicalize().map_err(|e| {
        format!("Workspace root canonicalization failed: {}", e)
    })?;

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
mod tests {
    use super::*;

    #[test]
    fn test_validate_workspace_path_rejects_absolute() {
        assert!(validate_workspace_path("/etc/passwd").is_err());
    }

    #[test]
    fn test_validate_workspace_path_rejects_traversal() {
        assert!(validate_workspace_path("../secret").is_err());
        assert!(validate_workspace_path("foo/../../bar").is_err());
    }

    #[test]
    fn test_validate_workspace_path_accepts_relative() {
        assert!(validate_workspace_path("src/main.rs").is_ok());
        assert!(validate_workspace_path("README.md").is_ok());
    }

    #[test]
    fn test_is_soul_path_variants() {
        assert!(is_soul_path("SOUL.md"));
        assert!(is_soul_path("soul.md"));
        assert!(is_soul_path("Soul.MD"));
        assert!(is_soul_path("config/SOUL.md"));
        assert!(!is_soul_path("README.md"));
        assert!(!is_soul_path("SOULMATE.md"));
        assert!(!is_soul_path("my_soul.md"));
    }

    #[test]
    fn test_unique_backup_path_avoids_collision() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        // Get a path, create the file, then get another — they should differ
        let path1 = unique_backup_path(&backup_dir);
        std::fs::write(&path1, "first").unwrap();

        let path2 = unique_backup_path(&backup_dir);
        assert_ne!(path1, path2, "Second path should differ from first");
        assert!(!path2.exists(), "Second path should not exist yet");
    }

    #[tokio::test]
    async fn test_prune_backups_removes_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        // Create 7 backup files with sequential timestamps
        for i in 1..=7 {
            let name = format!("SOUL.20260101T00000{}Z.md", i);
            std::fs::write(backup_dir.join(&name), format!("backup {}", i)).unwrap();
        }

        prune_backups(&backup_dir, 2).await;

        let remaining: Vec<_> = std::fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        assert_eq!(
            remaining.len(),
            2,
            "Should keep only 2 backups: {:?}",
            remaining
        );
        // Most recent (6 and 7) should survive
        assert!(remaining.contains(&"SOUL.20260101T000006Z.md".to_string()));
        assert!(remaining.contains(&"SOUL.20260101T000007Z.md".to_string()));
    }

    #[tokio::test]
    async fn test_prune_backups_noop_when_under_max() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        std::fs::write(backup_dir.join("SOUL.20260101T000001Z.md"), "b1").unwrap();
        std::fs::write(backup_dir.join("SOUL.20260101T000002Z.md"), "b2").unwrap();

        prune_backups(&backup_dir, 5).await;

        let count = std::fs::read_dir(&backup_dir).unwrap().count();
        assert_eq!(count, 2, "Should not remove any backups when under max");
    }

    #[tokio::test]
    async fn test_prune_backups_with_mixed_filename_formats() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        // Old format (second-precision)
        std::fs::write(backup_dir.join("SOUL.20250101T000001Z.md"), "old1").unwrap();
        // New format (nanosecond-precision)
        std::fs::write(
            backup_dir.join("SOUL.20260101T120000.123456789Z.md"),
            "new1",
        )
        .unwrap();
        // Collision-suffixed
        std::fs::write(
            backup_dir.join("SOUL.20260101T120000.123456789Z.1.md"),
            "new2",
        )
        .unwrap();
        // Another new format
        std::fs::write(
            backup_dir.join("SOUL.20260201T000000.000000000Z.md"),
            "new3",
        )
        .unwrap();

        prune_backups(&backup_dir, 2).await;

        let mut remaining: Vec<String> = std::fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        remaining.sort();

        assert_eq!(
            remaining.len(),
            2,
            "Should keep only 2 backups, got: {:?}",
            remaining
        );
        // Lexicographic sort: ".1.md" sorts before ".md" (digit < letter),
        // so the collision-suffixed file is older. The two most recent are:
        assert!(remaining.contains(&"SOUL.20260101T120000.123456789Z.md".to_string()));
        assert!(remaining.contains(&"SOUL.20260201T000000.000000000Z.md".to_string()));
    }
}
