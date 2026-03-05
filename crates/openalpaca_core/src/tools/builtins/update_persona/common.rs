//! Shared logic for all persona document handlers.

use std::path::Path;

/// Compute SHA-256 hash of content.
pub(super) fn sha256(content: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

/// Backup the existing file if it exists. Returns the backup path string.
pub(super) async fn backup_if_exists(
    doc_path: &Path,
    backup_dir: &Path,
    prefix: &str,
) -> Result<Option<String>, String> {
    if !doc_path.exists() {
        return Ok(None);
    }
    tokio::fs::create_dir_all(backup_dir)
        .await
        .map_err(|e| format!("Failed to create backup directory: {}", e))?;
    let backup_path = super::super::helpers::unique_backup_path(backup_dir, prefix);
    tokio::fs::copy(doc_path, &backup_path)
        .await
        .map_err(|e| format!("Failed to create backup: {}", e))?;
    Ok(Some(backup_path.display().to_string()))
}

/// Prune old backups if retention limit is configured.
pub(super) async fn prune_backups(backup_dir: &Path, max_backups: Option<usize>, prefix: &str) {
    if let Some(max) = max_backups {
        super::super::helpers::prune_backups(backup_dir, max, prefix).await;
    }
}

/// Atomic write: temp file -> fsync -> rename.
pub(super) async fn atomic_write(
    doc_path: &Path,
    content: &str,
    tmp_name: &str,
) -> Result<(), String> {
    let dir = doc_path
        .parent()
        .ok_or_else(|| "Document path has no parent directory".to_string())?;
    let tmp_path = dir.join(tmp_name);
    tokio::fs::write(&tmp_path, content)
        .await
        .map_err(|e| format!("Failed to write temp file: {}", e))?;
    let file = tokio::fs::File::open(&tmp_path)
        .await
        .map_err(|e| format!("Failed to open temp file for sync: {}", e))?;
    file.sync_all()
        .await
        .map_err(|e| format!("Failed to fsync temp file: {}", e))?;
    tokio::fs::rename(&tmp_path, doc_path)
        .await
        .map_err(|e| format!("Atomic rename failed: {}", e))?;
    Ok(())
}

/// Build the JSON result string.
pub(super) fn result_json(
    target: &str,
    mode: &str,
    path: &Path,
    hash: &str,
    content_length: usize,
    backup_path: Option<String>,
    extra: Option<(&str, serde_json::Value)>,
) -> String {
    let doc_name = match target {
        "soul" => "SOUL.md",
        "user" => "USER.md",
        "identity" => "IDENTITY.md",
        _ => "unknown",
    };
    let mut result = serde_json::json!({
        "status": "applied",
        "mode": mode,
        "path": path.display().to_string(),
        "content_sha256": hash,
        "content_length": content_length,
        "message": format!("{} updated successfully.", doc_name),
    });
    if let Some(bp) = backup_path {
        result["backup_path"] = serde_json::Value::String(bp);
    }
    if let Some((key, val)) = extra {
        result[key] = val;
    }
    result.to_string()
}
