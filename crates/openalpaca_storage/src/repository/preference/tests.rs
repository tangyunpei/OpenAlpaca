use super::*;
use tempfile::tempdir;

fn setup_db() -> Database {
    let dir = tempdir().unwrap();
    Database::open(&dir.path().join("test.db")).unwrap()
}

#[test]
fn test_set_and_get() {
    let db = setup_db();
    let repo = PreferenceRepository::new(&db);

    repo.set("user1", "theme", "dark", None).unwrap();

    let pref = repo.get("user1", "theme").unwrap().unwrap();
    assert_eq!(pref.value, "dark");
    assert_eq!(pref.version, 1);
}

#[test]
fn test_upsert_increments_version() {
    let db = setup_db();
    let repo = PreferenceRepository::new(&db);

    repo.set("user1", "lang", "en", None).unwrap();
    repo.set("user1", "lang", "zh", None).unwrap();

    let pref = repo.get("user1", "lang").unwrap().unwrap();
    assert_eq!(pref.value, "zh");
    assert_eq!(pref.version, 2);
}

#[test]
fn test_optimistic_lock_success() {
    let db = setup_db();
    let repo = PreferenceRepository::new(&db);

    repo.set("user1", "color", "blue", None).unwrap();
    // Version is 1, so expected_version=1 should succeed
    repo.set("user1", "color", "red", Some(1)).unwrap();

    let pref = repo.get("user1", "color").unwrap().unwrap();
    assert_eq!(pref.value, "red");
    assert_eq!(pref.version, 2);
}

#[test]
fn test_optimistic_lock_conflict() {
    let db = setup_db();
    let repo = PreferenceRepository::new(&db);

    repo.set("user1", "color", "blue", None).unwrap();
    // Version is 1, but we pass expected_version=99 -> should fail
    let result = repo.set("user1", "color", "red", Some(99));
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Optimistic lock conflict")
    );
}

#[test]
fn test_delete() {
    let db = setup_db();
    let repo = PreferenceRepository::new(&db);

    repo.set("user1", "key1", "val1", None).unwrap();
    repo.delete("user1", "key1").unwrap();

    assert!(repo.get("user1", "key1").unwrap().is_none());
}

#[test]
fn test_list_for_user() {
    let db = setup_db();
    let repo = PreferenceRepository::new(&db);

    repo.set("user1", "alpha", "1", None).unwrap();
    repo.set("user1", "beta", "2", None).unwrap();
    repo.set("user2", "gamma", "3", None).unwrap();

    let prefs = repo.list_for_user("user1").unwrap();
    assert_eq!(prefs.len(), 2);
    assert_eq!(prefs[0].key, "alpha");
    assert_eq!(prefs[1].key, "beta");
}

#[test]
fn test_get_nonexistent() {
    let db = setup_db();
    let repo = PreferenceRepository::new(&db);

    assert!(repo.get("nobody", "nothing").unwrap().is_none());
}
