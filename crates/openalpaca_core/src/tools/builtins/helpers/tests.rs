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
