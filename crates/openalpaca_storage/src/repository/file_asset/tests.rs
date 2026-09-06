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

/// §4.8's "two numbers, never one", read in one pass: the quota-bearing upload
/// total and the informational produced total, from the same grouped scan.
#[test]
fn storage_bytes_split_upload_from_produced_in_one_query() {
    let db = test_db();
    let repo = FileAssetRepository::new(&db);

    let empty = repo.storage_bytes_by_origin().unwrap();
    assert_eq!(empty.upload_bytes, 0);
    assert_eq!(empty.produced_bytes, 0);

    repo.insert(&asset("upload-1", 700)).unwrap();
    repo.insert(&asset("upload-2", 300)).unwrap();
    repo.insert(&asset("produced-1", 5_000)).unwrap();
    mark(&db, "produced-1", "produced", 0, 0);
    repo.insert(&asset("produced-2", 11)).unwrap();
    mark(&db, "produced-2", "produced", 0, 0);

    let split = repo.storage_bytes_by_origin().unwrap();
    assert_eq!(split.upload_bytes, 1_000);
    assert_eq!(split.produced_bytes, 5_011);
    // The quota reader and the split must never disagree about uploads.
    assert_eq!(split.upload_bytes, repo.total_storage_bytes().unwrap());
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

// ── GAP-23: the message ⇄ artifact link ─────────────────────────────

/// Stamp the run and the kind migration 036 added; `insert` leaves both unset.
fn produced_by(db: &Database, id: &str, task_id: &str, kind: &str) {
    db.with_connection(|conn| {
        conn.execute(
            "INSERT OR IGNORE INTO task (id, title, created_by, source_lane)
             VALUES (?1, 'A run', 'tester', 'user:gui')",
            [task_id],
        )?;
        let updated = conn.execute(
            "UPDATE file_assets SET origin = 'produced', task_id = ?2, kind = ?3 WHERE id = ?1",
            rusqlite::params![id, task_id, kind],
        )?;
        assert_eq!(updated, 1, "{id} should exist");
        Ok(())
    })
    .unwrap();
}

fn message(db: &Database, lane_key: &str, content: &str) -> i64 {
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO conversation_messages (lane_key, role, content)
             VALUES (?1, 'assistant', ?2)",
            [lane_key, content],
        )?;
        Ok(conn.last_insert_rowid())
    })
    .unwrap()
}

fn role_of(db: &Database, message_id: i64, file_id: &str) -> String {
    db.with_connection(|conn| {
        Ok(conn.query_row(
            "SELECT role FROM conversation_message_attachments
              WHERE message_id = ?1 AND file_id = ?2",
            rusqlite::params![message_id, file_id],
            |row| row.get(0),
        )?)
    })
    .unwrap()
}

#[test]
fn link_to_message_still_writes_the_attachment_role() {
    let db = test_db();
    let repo = FileAssetRepository::new(&db);
    repo.insert(&asset("upload-1", 10)).unwrap();
    let msg = message(&db, "user:gui", "here is the file");

    repo.link_to_message(msg, "upload-1", 0, Some("a caption"))
        .unwrap();

    assert_eq!(role_of(&db, msg, "upload-1"), "attachment");
    // The upload half of the read is unchanged by the new role.
    let attachments = repo.get_attachments_for_message(msg).unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].0, "upload-1");
    assert_eq!(attachments[0].2.as_deref(), Some("a caption"));
}

#[test]
fn link_to_message_with_role_writes_the_artifact_role() {
    let db = test_db();
    let repo = FileAssetRepository::new(&db);
    repo.insert(&asset("produced-1", 10)).unwrap();
    produced_by(&db, "produced-1", "task-1", "markdown");
    let msg = message(&db, "user:gui", "the report");

    repo.link_to_message_with_role(msg, "produced-1", 0, None, ARTIFACT_ROLE)
        .unwrap();

    assert_eq!(role_of(&db, msg, "produced-1"), "artifact");
    // The artifact row must not come back from the attachment-only reader.
    assert!(repo.get_attachments_for_message(msg).unwrap().is_empty());
}

/// A message with one upload and one artifact link: each accessor returns
/// exactly its own row, never the other's (Important #1 — `Self::get_attachments_for_message`
/// had no `role` predicate and returned both).
#[test]
fn attachments_and_artifacts_readers_stay_split() {
    let db = test_db();
    let repo = FileAssetRepository::new(&db);
    repo.insert(&asset("upload-1", 10)).unwrap();
    repo.insert(&asset("produced-1", 20)).unwrap();
    produced_by(&db, "produced-1", "task-1", "markdown");
    let msg = message(&db, "user:gui", "the report");

    repo.link_to_message(msg, "upload-1", 0, None).unwrap();
    repo.link_to_message_with_role(msg, "produced-1", 1, None, ARTIFACT_ROLE)
        .unwrap();

    let attachments = repo.get_attachments_for_message(msg).unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].0, "upload-1");

    let artifacts = repo.get_artifacts_for_message(msg).unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].0, "produced-1");
}

#[test]
fn artifact_links_read_back_only_the_artifact_rows() {
    let db = test_db();
    let repo = FileAssetRepository::new(&db);
    repo.insert(&asset("upload-1", 10)).unwrap();
    repo.insert(&asset("produced-1", 20)).unwrap();
    repo.insert(&asset("produced-2", 30)).unwrap();
    produced_by(&db, "produced-1", "task-1", "markdown");
    produced_by(&db, "produced-2", "task-1", "code");

    let turn = message(&db, "user:gui", "here is the file");
    let report = message(&db, "user:gui", "the report");
    repo.link_to_message(turn, "upload-1", 0, None).unwrap();
    repo.link_to_message_with_role(report, "produced-1", 0, None, ARTIFACT_ROLE)
        .unwrap();
    repo.link_to_message_with_role(report, "produced-2", 1, None, ARTIFACT_ROLE)
        .unwrap();

    let links = repo.artifact_links_for_messages(&[turn, report]).unwrap();

    // The uploaded attachment is not an artifact chip.
    assert!(!links.contains_key(&turn));
    let artifacts = links.get(&report).expect("the report's artifacts");
    assert_eq!(
        artifacts,
        &vec![
            MessageArtifact {
                id: "produced-1".to_string(),
                name: "produced-1.md".to_string(),
                kind: Some("markdown".to_string()),
            },
            MessageArtifact {
                id: "produced-2".to_string(),
                name: "produced-2.md".to_string(),
                kind: Some("code".to_string()),
            },
        ]
    );
}

#[test]
fn artifact_links_for_no_messages_touches_nothing() {
    let db = test_db();
    let repo = FileAssetRepository::new(&db);
    assert!(repo.artifact_links_for_messages(&[]).unwrap().is_empty());
}

#[test]
fn produced_ids_for_task_skips_uploads_and_other_runs() {
    let db = test_db();
    let repo = FileAssetRepository::new(&db);
    repo.insert(&asset("produced-1", 10)).unwrap();
    repo.insert(&asset("produced-2", 20)).unwrap();
    repo.insert(&asset("elsewhere", 30)).unwrap();
    repo.insert(&asset("upload-1", 40)).unwrap();
    produced_by(&db, "produced-1", "task-1", "markdown");
    produced_by(&db, "produced-2", "task-1", "code");
    produced_by(&db, "elsewhere", "task-2", "markdown");
    // An upload the user attached *during* the run is still an upload.
    db.with_connection(|conn| {
        conn.execute(
            "UPDATE file_assets SET task_id = 'task-1' WHERE id = 'upload-1'",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    assert_eq!(
        repo.produced_ids_for_task("task-1").unwrap(),
        vec!["produced-1".to_string(), "produced-2".to_string()]
    );
    assert!(repo.produced_ids_for_task("task-none").unwrap().is_empty());
}
