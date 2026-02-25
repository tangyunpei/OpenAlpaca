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
