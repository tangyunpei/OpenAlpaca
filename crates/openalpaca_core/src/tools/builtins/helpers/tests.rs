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
    let path1 = unique_backup_path(&backup_dir, "SOUL");
    std::fs::write(&path1, "first").unwrap();

    let path2 = unique_backup_path(&backup_dir, "SOUL");
    assert_ne!(path1, path2, "Second path should differ from first");
    assert!(!path2.exists(), "Second path should not exist yet");
}

#[test]
fn test_unique_backup_path_uses_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let backup_dir = dir.path().join("backups");
    std::fs::create_dir_all(&backup_dir).unwrap();

    let soul_path = unique_backup_path(&backup_dir, "SOUL");
    let user_path = unique_backup_path(&backup_dir, "USER");
    let identity_path = unique_backup_path(&backup_dir, "IDENTITY");

    let soul_name = soul_path.file_name().unwrap().to_str().unwrap();
    let user_name = user_path.file_name().unwrap().to_str().unwrap();
    let identity_name = identity_path.file_name().unwrap().to_str().unwrap();

    assert!(
        soul_name.starts_with("SOUL."),
        "SOUL prefix expected: {}",
        soul_name
    );
    assert!(
        user_name.starts_with("USER."),
        "USER prefix expected: {}",
        user_name
    );
    assert!(
        identity_name.starts_with("IDENTITY."),
        "IDENTITY prefix expected: {}",
        identity_name
    );

    assert!(soul_name.ends_with(".md"));
    assert!(user_name.ends_with(".md"));
    assert!(identity_name.ends_with(".md"));
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

    prune_backups(&backup_dir, 2, "SOUL").await;

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

    prune_backups(&backup_dir, 5, "SOUL").await;

    let count = std::fs::read_dir(&backup_dir).unwrap().count();
    assert_eq!(count, 2, "Should not remove any backups when under max");
}

#[tokio::test]
async fn test_prune_backups_with_mixed_filename_formats() {
    let dir = tempfile::tempdir().unwrap();
    let backup_dir = dir.path().join("backups");
    std::fs::create_dir_all(&backup_dir).unwrap();

    // Old format (second-precision, legacy)
    std::fs::write(backup_dir.join("SOUL.20250101T000001Z.md"), "old1").unwrap();
    // New format (nanosecond-precision + UUID suffix)
    std::fs::write(
        backup_dir.join("SOUL.20260101T120000.123456789Z.a1b2c3d4.md"),
        "new1",
    )
    .unwrap();
    // Same timestamp, different UUID suffix (concurrent backups)
    std::fs::write(
        backup_dir.join("SOUL.20260101T120000.123456789Z.e5f6g7h8.md"),
        "new2",
    )
    .unwrap();
    // Later timestamp
    std::fs::write(
        backup_dir.join("SOUL.20260201T000000.000000000Z.deadbeef.md"),
        "new3",
    )
    .unwrap();

    prune_backups(&backup_dir, 2, "SOUL").await;

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
    // Lexicographic sort orders by timestamp first, then UUID suffix.
    // The two most recent entries are the two with later timestamps.
    assert!(remaining.contains(&"SOUL.20260101T120000.123456789Z.e5f6g7h8.md".to_string()));
    assert!(remaining.contains(&"SOUL.20260201T000000.000000000Z.deadbeef.md".to_string()));
}

#[tokio::test]
async fn test_prune_backups_only_prunes_matching_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let backup_dir = dir.path().join("backups");
    std::fs::create_dir_all(&backup_dir).unwrap();

    // Create backups for different prefixes in the same directory
    std::fs::write(backup_dir.join("SOUL.20260101T000001Z.md"), "s1").unwrap();
    std::fs::write(backup_dir.join("SOUL.20260101T000002Z.md"), "s2").unwrap();
    std::fs::write(backup_dir.join("SOUL.20260101T000003Z.md"), "s3").unwrap();
    std::fs::write(backup_dir.join("USER.20260101T000001Z.md"), "u1").unwrap();
    std::fs::write(backup_dir.join("USER.20260101T000002Z.md"), "u2").unwrap();

    // Prune SOUL backups to keep 1
    prune_backups(&backup_dir, 1, "SOUL").await;

    let remaining: Vec<String> = std::fs::read_dir(&backup_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    // Should keep 1 SOUL + 2 USER = 3 total
    assert_eq!(remaining.len(), 3, "Got: {:?}", remaining);
    assert!(remaining.contains(&"SOUL.20260101T000003Z.md".to_string()));
    assert!(remaining.contains(&"USER.20260101T000001Z.md".to_string()));
    assert!(remaining.contains(&"USER.20260101T000002Z.md".to_string()));
}

// --- is_store_owned_path (memory redesign, design §2; plan 3.2) ---

#[test]
fn store_state_is_refused_on_every_access() {
    for path in [
        "state/memory-control/home-32acea7eb739b08afd959db91e46d450.json",
        "state/openalpaca.db",
        "state",
        "STATE/x",
        "./state/x",
    ] {
        assert!(is_store_owned_path(path, StoreAccess::Read).is_some(), "{path}");
        assert!(is_store_owned_path(path, StoreAccess::Write).is_some(), "{path}");
    }
    for path in ["src/state/x.rs", "states/x", "my-state/x", "state.md"] {
        assert!(is_store_owned_path(path, StoreAccess::Read).is_none(), "{path}");
        assert!(is_store_owned_path(path, StoreAccess::Write).is_none(), "{path}");
    }
}

#[test]
fn partition_control_files_are_refused_on_write_only() {
    for path in [
        ".openalpaca/memory/owners/k/.history/id/rev.md",
        "memory/owners/k/.history",
        "memory/owners/k/MEMORY.md",
        "MEMORY.md",
        "memory.md",
        ".openalpaca/memory/.gitignore",
        "memory/.gitignore",
    ] {
        assert!(is_store_owned_path(path, StoreAccess::Write).is_some(), "{path}");
        assert!(is_store_owned_path(path, StoreAccess::Read).is_none(), "{path} is readable");
    }
    for path in [
        "memory/owners/k/notes/a.md",
        ".gitignore",
        "src/.gitignore",
        "history/x.md",
        "MEMORY.txt",
        "docs/MEMORY-notes.md",
    ] {
        assert!(is_store_owned_path(path, StoreAccess::Write).is_none(), "{path}");
    }
}

// --- resolve_workspace_path_for_write (S3-R1, plan §7a) ---

/// A workspace root with a `state/` directory beside an ordinary one, as the
/// home store the daemon runs in has.
#[cfg(unix)]
fn write_resolution_root() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("state")).unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    let canonical = dir.path().canonicalize().unwrap();
    (dir, canonical)
}

/// The nearest existing ancestor is what gets resolved: a link into
/// `state/` followed by directories that do not exist yet resolves under
/// `state/`, where the store check can see it.
#[cfg(unix)]
#[test]
fn a_new_directory_behind_a_link_resolves_through_the_link() {
    let (dir, root) = write_resolution_root();
    std::os::unix::fs::symlink("state", dir.path().join("link")).unwrap();

    let resolved = resolve_workspace_path_for_write("link/newdir/deeper/x.json", dir.path())
        .expect("a link inside the workspace stays inside it");
    assert_eq!(resolved, root.join("state/newdir/deeper/x.json"));
    assert!(
        is_store_owned_resolved(&resolved, dir.path(), StoreAccess::Write).is_some(),
        "the resolved path is the store's"
    );
}

/// The same walk refuses a link out of the workspace before a new
/// directory, and a dangling link (nothing to resolve it to).
#[cfg(unix)]
#[test]
fn a_new_directory_behind_an_escaping_or_dangling_link_is_refused() {
    let (dir, _root) = write_resolution_root();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("out")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("missing"), dir.path().join("dangling"))
        .unwrap();

    let err = resolve_workspace_path_for_write("out/newdir/x.json", dir.path())
        .expect_err("a link out of the workspace is refused");
    assert!(err.contains("outside the workspace boundary"), "{err}");
    let err = resolve_workspace_path_for_write("out/x.json", dir.path())
        .expect_err("with an existing parent too");
    assert!(err.contains("outside the workspace boundary"), "{err}");
    resolve_workspace_path_for_write("dangling/newdir/x.json", dir.path())
        .expect_err("a dangling link resolves to nothing");
    assert!(!outside.path().join("missing").exists());
}

/// No link on the way: a new nested path resolves under the canonical root
/// (on macOS a tempdir's `/var` is itself a link to `/private/var`), and an
/// existing parent keeps resolving as it did.
#[cfg(unix)]
#[test]
fn a_plain_new_path_resolves_under_the_canonical_root() {
    let (dir, root) = write_resolution_root();
    assert_eq!(
        resolve_workspace_path_for_write("a/b/c.txt", dir.path()).unwrap(),
        root.join("a/b/c.txt")
    );
    assert_eq!(
        resolve_workspace_path_for_write("src/main.rs", dir.path()).unwrap(),
        root.join("src/main.rs")
    );
    assert_eq!(
        resolve_workspace_path_for_write("./src/new/x.rs", dir.path()).unwrap(),
        root.join("src/new/x.rs")
    );
    assert_eq!(
        resolve_workspace_path_for_write("top.txt", dir.path()).unwrap(),
        root.join("top.txt")
    );
}
