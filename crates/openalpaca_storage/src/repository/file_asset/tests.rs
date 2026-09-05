//! Tests for the two migration-036 defences (plan §4.5): the orphan sweep and
//! the upload quota must both ignore produced artifacts.

use super::*;
use crate::models::file_asset::FileAsset;
use crate::test_util::test_db;

fn asset(id: &str, size_bytes: i64) -> FileAsset {
    FileAsset {
        id: id.to_string(),
        owner_id: "owner-1".to_string(),
        sha256: format!("sha-{id}"),
        filename: format!("{id}.md"),
        mime_type: "text/markdown".to_string(),
        size_bytes,
        storage_path: format!("/tmp/{id}.md"),
        status: FileAssetStatus::Ready,
        extracted_text: None,
        extract_error: None,
        metadata_json: None,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

/// Age a row past the sweep's grace period and stamp the artifact-store columns
/// migration 036 added (the repository's own `insert` leaves them at default).
fn mark(db: &Database, id: &str, origin: &str, pinned: i64, age_hours: i64) {
    db.with_connection(|conn| {
        let updated = conn.execute(
            "UPDATE file_assets
                SET origin = ?2,
                    pinned = ?3,
                    created_at = datetime('now', ?4)
              WHERE id = ?1",
            rusqlite::params![id, origin, pinned, format!("-{age_hours} hours")],
        )?;
        assert_eq!(updated, 1, "{id} should exist");
        Ok(())
    })
    .unwrap();
}

fn orphan_ids(repo: &FileAssetRepository<'_>, grace_hours: i64) -> Vec<String> {
    let mut ids: Vec<String> = repo
        .list_orphaned(grace_hours)
        .unwrap()
        .into_iter()
        .map(|a| a.id)
        .collect();
    ids.sort();
    ids
}

#[test]
fn produced_artifact_survives_the_sweep() {
    let db = test_db();
    let repo = FileAssetRepository::new(&db);

    repo.insert(&asset("produced-1", 100)).unwrap();
    repo.insert(&asset("upload-1", 100)).unwrap();
    // Well past the 24 h grace the daemon runs with (background.rs).
    mark(&db, "produced-1", "produced", 0, 30);
    mark(&db, "upload-1", "upload", 0, 30);

    // 25 simulated sweep-hours (plan §9's risk row) — and the real 24 h grace.
    for grace in [24, 25] {
        assert_eq!(
            orphan_ids(&repo, grace),
            vec!["upload-1".to_string()],
            "a produced artifact is never garbage-collected (grace {grace}h)"
        );
    }
}

#[test]
fn pinned_upload_survives_the_sweep() {
    let db = test_db();
    let repo = FileAssetRepository::new(&db);

    repo.insert(&asset("pinned-1", 100)).unwrap();
    repo.insert(&asset("upload-1", 100)).unwrap();
    mark(&db, "pinned-1", "upload", 1, 30);
    mark(&db, "upload-1", "upload", 0, 30);

    assert_eq!(orphan_ids(&repo, 24), vec!["upload-1".to_string()]);
}

#[test]
fn young_uploads_are_left_alone() {
    let db = test_db();
    let repo = FileAssetRepository::new(&db);

    repo.insert(&asset("upload-1", 100)).unwrap();
    mark(&db, "upload-1", "upload", 0, 1);

    assert!(
        orphan_ids(&repo, 24).is_empty(),
        "the grace period still applies to uploads"
    );
}

#[test]
fn quota_counts_uploads_only() {
    let db = test_db();
    let repo = FileAssetRepository::new(&db);

    assert_eq!(repo.total_storage_bytes().unwrap(), 0);

    repo.insert(&asset("upload-1", 700)).unwrap();
    // A row that predates 036 keeps the column default, `origin = 'upload'`.
    assert_eq!(repo.total_storage_bytes().unwrap(), 700);

    repo.insert(&asset("produced-1", 5_000)).unwrap();
    mark(&db, "produced-1", "produced", 0, 0);
    assert_eq!(
        repo.total_storage_bytes().unwrap(),
        700,
        "agent output must not count against the upload quota"
    );

    repo.insert(&asset("upload-2", 300)).unwrap();
    assert_eq!(repo.total_storage_bytes().unwrap(), 1_000);
}

#[test]
fn readers_tolerate_the_new_columns() {
    let db = test_db();
    let repo = FileAssetRepository::new(&db);

    repo.insert(&asset("produced-1", 42)).unwrap();
    mark(&db, "produced-1", "produced", 1, 0);

    let fetched = repo.get_by_id("produced-1").unwrap().expect("row");
    assert_eq!(fetched.size_bytes, 42);
    assert_eq!(fetched.sha256, "sha-produced-1");
    assert_eq!(fetched.status, FileAssetStatus::Ready);

    let by_sha = repo.get_by_sha256("sha-produced-1").unwrap().expect("row");
    assert_eq!(by_sha.id, "produced-1");

    let ready = repo.list_by_status(&FileAssetStatus::Ready, 10).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].id, "produced-1");
}
