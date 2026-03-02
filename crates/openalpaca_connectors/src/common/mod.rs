//! Common utilities shared across all connectors.

use openalpaca_core::security::policy::Principal;
use openalpaca_storage::IdentityRepository;

#[cfg(any(feature = "telegram", feature = "imessage", feature = "discord"))]
use openalpaca_core::gateway::ResolvedAttachment;
#[cfg(any(feature = "telegram", feature = "imessage", feature = "discord"))]
use openalpaca_storage::{Database, FileAsset, FileAssetRepository, FileAssetStatus};

/// Resolve a Principal from an external identity.
///
/// Returns:
/// - `Principal::User` if the external identity is linked to a global_user
/// - `Principal::External` if not linked (untrusted)
pub fn resolve_principal(
    identity_repo: &IdentityRepository<'_>,
    provider: &str,
    provider_user_id: &str,
    display_name: Option<&str>,
) -> Result<(Principal, i64), String> {
    let external_identity = identity_repo
        .get_or_create_external_identity(provider, provider_user_id, display_name)
        .map_err(|e| format!("Failed to get/create identity: {}", e))?;

    let principal = match &external_identity.global_user_id {
        Some(global_id) => Principal::User {
            global_id: global_id.clone(),
        },
        None => Principal::External {
            provider: provider.to_string(),
            id: provider_user_id.to_string(),
        },
    };

    Ok((principal, external_identity.id))
}

/// Format a denial message for TrustGate rejection.
pub fn format_denial_message(error: &str) -> String {
    format!("⚠️ {}\n\nUse /link <token> to link your account.", error)
}

/// Redact a token for safe logging (show only first 4 chars).
pub fn redact_token(token: &str) -> String {
    if token.len() <= 4 {
        "****".to_string()
    } else {
        let prefix: String = token.chars().take(4).collect();
        format!("{}****", prefix)
    }
}

/// Handle the /link command logic.
///
/// Uses an atomic consume-and-link transaction so that the token is not
/// consumed if linking the identity fails.
pub fn handle_link_token(
    identity_repo: &IdentityRepository<'_>,
    token: &str,
    external_identity_id: i64,
) -> Result<LinkResult, String> {
    match identity_repo.consume_and_link(token, external_identity_id) {
        Ok(Some(global_user_id)) => Ok(LinkResult::Success(global_user_id)),
        Ok(None) => Ok(LinkResult::InvalidToken),
        Err(e) => Err(e.to_string()),
    }
}

/// Result of a link operation.
pub enum LinkResult {
    /// Successfully linked to the given global_user_id
    Success(String),
    /// Token was invalid, expired, or already used
    InvalidToken,
}

/// Store an inbound attachment from a connector.
///
/// Computes SHA-256, deduplicates, writes to disk, and inserts a DB row.
/// Returns a `ResolvedAttachment` ready for `GatewayRequest`.
#[cfg(any(feature = "telegram", feature = "imessage", feature = "discord"))]
pub fn store_attachment(
    db: &Database,
    owner_id: &str,
    filename: &str,
    mime_type: &str,
    data: &[u8],
    max_file_size: u64,
    max_image_dimension: u32,
) -> Result<ResolvedAttachment, String> {
    use openalpaca_core::security::sanitizer::InputSanitizer;
    use sha2::{Digest, Sha256};

    // Validate upload (defense-in-depth for connector-sourced files)
    if let Err(violation) = InputSanitizer::validate_upload_with_image_limit(
        filename,
        data,
        mime_type,
        max_file_size,
        max_image_dimension,
    ) {
        return Err(format!("Upload validation failed: {violation}"));
    }

    // 1. Compute SHA-256
    let mut hasher = Sha256::new();
    hasher.update(data);
    let sha256 = format!("{:x}", hasher.finalize());

    // 2. Dedup: check if asset with same SHA-256 already exists for this owner
    let repo = FileAssetRepository::new(db);
    if let Ok(Some(existing)) = repo.get_by_sha256(&sha256)
        && existing.owner_id == owner_id
    {
        return Ok(ResolvedAttachment {
            file_id: existing.id,
            filename: existing.filename,
            mime_type: existing.mime_type,
            size_bytes: existing.size_bytes,
            extracted_text: existing.extracted_text,
            storage_path: existing.storage_path,
        });
    }

    // 3. Compute sharded storage path
    let storage_path = openalpaca_storage::paths::asset_storage_path(&sha256)
        .map_err(|e| format!("Failed to compute storage path: {e}"))?;

    // 4. Write file to disk
    if let Some(parent) = storage_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create asset directory: {e}"))?;
    }
    std::fs::write(&storage_path, data).map_err(|e| format!("Failed to write asset file: {e}"))?;

    // 5. Insert DB row
    let file_id = uuid::Uuid::new_v4().to_string();
    let asset = FileAsset {
        id: file_id.clone(),
        owner_id: owner_id.to_string(),
        sha256,
        filename: filename.to_string(),
        mime_type: mime_type.to_string(),
        size_bytes: data.len() as i64,
        storage_path: storage_path.to_string_lossy().to_string(),
        status: FileAssetStatus::Uploaded,
        extracted_text: None,
        extract_error: None,
        metadata_json: None,
        created_at: String::new(), // set by DB default
        updated_at: String::new(), // set by DB default
    };
    repo.insert(&asset)
        .map_err(|e| format!("Failed to insert file asset: {e}"))?;

    // 6. Return ResolvedAttachment
    Ok(ResolvedAttachment {
        file_id,
        filename: filename.to_string(),
        mime_type: mime_type.to_string(),
        size_bytes: data.len() as i64,
        extracted_text: None,
        storage_path: storage_path.to_string_lossy().to_string(),
    })
}

#[cfg(test)]
mod tests;
