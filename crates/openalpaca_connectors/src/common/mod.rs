//! Common utilities shared across all connectors.

use openalpaca_core::security::policy::Principal;
use openalpaca_storage::IdentityRepository;

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

/// Handle the /link command logic.
pub fn handle_link_token(
    identity_repo: &IdentityRepository<'_>,
    token: &str,
    external_identity_id: i64,
) -> Result<LinkResult, String> {
    match identity_repo.consume_link_token(token) {
        Ok(Some(global_user_id)) => {
            identity_repo
                .link_external_identity(external_identity_id, &global_user_id)
                .map_err(|e| format!("Failed to link identity: {}", e))?;
            Ok(LinkResult::Success(global_user_id))
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_storage::Database;
    use tempfile::tempdir;

    fn test_db() -> Database {
        let dir = tempdir().unwrap();
        Database::open(&dir.path().join("test.db")).unwrap()
    }

    #[test]
    fn test_resolve_principal_untrusted() {
        let db = test_db();
        let repo = IdentityRepository::new(&db);

        // Test untrusted
        let (principal, _) =
            resolve_principal(&repo, "telegram", "user123", Some("Alice")).unwrap();
        assert!(matches!(principal, Principal::External { id, .. } if id == "user123"));
    }

    #[test]
    fn test_resolve_principal_trusted() {
        let db = test_db();
        let repo = IdentityRepository::new(&db);

        // Link user first
        repo.create_global_user("global1", None).unwrap();
        let ext = repo
            .get_or_create_external_identity("telegram", "user123", None)
            .unwrap();
        repo.link_external_identity(ext.id, "global1").unwrap();

        // Test trusted
        let (principal, _) = resolve_principal(&repo, "telegram", "user123", None).unwrap();
        assert!(matches!(principal, Principal::User { global_id } if global_id == "global1"));
    }

    #[test]
    fn test_handle_link_token_flow() {
        let db = test_db();
        let repo = IdentityRepository::new(&db);

        repo.create_global_user("global1", None).unwrap();
        repo.create_link_token("global1", "TOKEN1").unwrap();
        let ext = repo
            .get_or_create_external_identity("telegram", "user123", None)
            .unwrap();

        // Consume
        let res = handle_link_token(&repo, "TOKEN1", ext.id).unwrap();
        assert!(matches!(res, LinkResult::Success(uid) if uid == "global1"));

        // Verify linked in DB
        let ext_after = repo
            .get_external_identity("telegram", "user123")
            .unwrap()
            .unwrap();
        assert_eq!(ext_after.global_user_id, Some("global1".to_string()));
    }
}
