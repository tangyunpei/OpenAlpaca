//! The one upload writer, from both callers' point of view, plus D2 placement.
//!
//! Nothing here touches a real root: every test points
//! `OPENALPACA_HOME_STORE` at a temp directory first, and a project scope is
//! always a temp directory too.

use super::*;
use crate::FileAssetRepository;
use crate::store::tests::HomeStoreGuard;
use crate::store::{interim_asset_storage_path, store_root};
use chrono::TimeZone;
use std::path::PathBuf;
use tempfile::{TempDir, tempdir};

// ============================================================================
// Fixture
// ============================================================================

/// A temp home root (via `OPENALPACA_HOME_STORE`), a temp project root, and a
/// temp database.
struct Fixture {
    _home: TempDir,
    _env: HomeStoreGuard,
    _db_dir: TempDir,
    project: TempDir,
    db: Database,
}

/// A fixed day, so every expected path in this file is spelled out in full.
fn day() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 5, 12, 0, 0).unwrap()
}

impl Fixture {
    fn new() -> Self {
        let home = tempdir().unwrap();
        let env = HomeStoreGuard::set(&home.path().canonicalize().unwrap());
        let project = tempdir().unwrap();
        let db_dir = tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();
        Self {
            _home: home,
            _env: env,
            _db_dir: db_dir,
            project,
            db,
        }
    }

    fn store(&self) -> UploadStore<'_> {
        UploadStore::new(&self.db)
    }

    fn repo(&self) -> FileAssetRepository<'_> {
        FileAssetRepository::new(&self.db)
    }

    fn home_root(&self) -> PathBuf {
        self._home.path().canonicalize().unwrap()
    }

    /// The canonical project root — canonical because `project_root` is the
    /// canonical-path string everywhere (§4.8) and macOS's `/var` is a symlink.
    fn project_root(&self) -> PathBuf {
        self.project.path().canonicalize().unwrap()
    }

    fn project_scope(&self) -> StoreScope {
        StoreScope::Project(self.project_root())
    }

    /// `<store>/uploads` — the root `rel_path` is relative to.
    fn uploads_root(&self, scope: &StoreScope) -> PathBuf {
        store_root(scope).unwrap().join("uploads")
    }

    fn put_in(&self, scope: &StoreScope, owner: &str, filename: &str, data: &[u8]) -> StoredUpload {
        self.store()
            .put(NewUpload {
                owner_id: owner,
                filename,
                mime_type: "text/plain",
                data,
                scope,
                created: day(),
            })
            .unwrap()
    }

    fn put(&self, owner: &str, filename: &str, data: &[u8]) -> StoredUpload {
        self.put_in(&StoreScope::Home, owner, filename, data)
    }

    /// The `(project_root, rel_path)` address a row records.
    fn address(&self, id: &str) -> (Option<String>, Option<String>) {
        self.db
            .with_connection(|conn| {
                Ok(conn.query_row(
                    "SELECT project_root, rel_path FROM file_assets WHERE id = ?1",
                    rusqlite::params![id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?)
            })
            .unwrap()
    }

    fn origin(&self, id: &str) -> String {
        self.db
            .with_connection(|conn| {
                Ok(conn.query_row(
                    "SELECT origin FROM file_assets WHERE id = ?1",
                    rusqlite::params![id],
                    |row| row.get(0),
                )?)
            })
            .unwrap()
    }
}

/// Every file under `dir`, recursively, as `/`-joined paths relative to it.
fn files_under(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(
                    path.strip_prefix(dir)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    out.sort();
    out
}

// ============================================================================
// D2 placement
// ============================================================================

/// The Verify item: an upload whose request named a project lands in that
/// project's store, under the day directory, with the §4.2 file grammar.
#[test]
fn a_project_scoped_upload_lands_in_the_project_store() {
    let fx = Fixture::new();
    let scope = fx.project_scope();

    let stored = fx.put_in(&scope, "owner-1", "Quarterly Report.PDF", b"pdf-bytes");

    let expected = fx
        .project_root()
        .join(".openalpaca")
        .join("uploads")
        .join("2026-09-05")
        .join("01-quarterly-report.pdf");
    assert_eq!(PathBuf::from(&stored.asset.storage_path), expected);
    assert_eq!(std::fs::read(&expected).unwrap(), b"pdf-bytes");

    // The address of record, and the origin the quota and the sweep key off.
    let (project_root, rel_path) = fx.address(&stored.asset.id);
    assert_eq!(project_root.as_deref(), fx.project_root().to_str());
    assert_eq!(
        rel_path.as_deref(),
        Some("2026-09-05/01-quarterly-report.pdf")
    );
    assert_eq!(fx.origin(&stored.asset.id), "upload");

    // The name the user sees is untouched; only the file on disk is slugified.
    assert_eq!(stored.asset.filename, "Quarterly Report.PDF");

    // Nothing leaked into the home store.
    assert!(
        files_under(&fx.home_root())
            .iter()
            .all(|f| !f.starts_with("uploads/")),
        "a project-scoped upload must not touch the home store's uploads/"
    );
}

/// No project signal — the route without the header, and every connector
/// attachment — takes the home store. `project_root` is `NULL`, the address
/// baseline.
#[test]
fn an_upload_with_no_project_lands_in_the_home_store() {
    let fx = Fixture::new();
    let stored = fx.put("owner-1", "notes.txt", b"hello");

    let expected = fx
        .home_root()
        .join("uploads")
        .join("2026-09-05")
        .join("01-notes.txt");
    assert_eq!(PathBuf::from(&stored.asset.storage_path), expected);
    assert_eq!(std::fs::read(&expected).unwrap(), b"hello");

    let (project_root, rel_path) = fx.address(&stored.asset.id);
    assert_eq!(project_root, None, "the home store is the address baseline");
    assert_eq!(rel_path.as_deref(), Some("2026-09-05/01-notes.txt"));
}

/// The sequence is per day directory, and per store: two projects both start
/// at `01`, and a second upload in the same directory gets `02`.
#[test]
fn the_sequence_counts_within_one_day_directory() {
    let fx = Fixture::new();
    let scope = fx.project_scope();

    let first = fx.put_in(&scope, "owner-1", "a.txt", b"one");
    let second = fx.put_in(&scope, "owner-1", "b.txt", b"two");
    let home = fx.put("owner-1", "c.txt", b"three");

    assert!(first.asset.storage_path.ends_with("/01-a.txt"));
    assert!(second.asset.storage_path.ends_with("/02-b.txt"));
    assert!(
        home.asset.storage_path.ends_with("/01-c.txt"),
        "another store's day directory starts at 01: {}",
        home.asset.storage_path
    );
}

/// Two digits, widening to three past 99 rather than truncating (§4.2).
#[test]
fn the_sequence_widens_past_ninety_nine() {
    let fx = Fixture::new();
    let existing = fx.put("owner-1", "first.txt", b"first");
    // Re-address the existing row as `99-`, the state the writer reads.
    fx.db
        .with_connection(|conn| {
            conn.execute(
                "UPDATE file_assets SET rel_path = '2026-09-05/99-first.txt' WHERE id = ?1",
                rusqlite::params![existing.asset.id],
            )?;
            Ok(())
        })
        .unwrap();

    let next = fx.put("owner-1", "hundredth.txt", b"hundredth");
    assert!(
        next.asset.storage_path.ends_with("/100-hundredth.txt"),
        "the sequence widens rather than truncating: {}",
        next.asset.storage_path
    );
}

/// Traversal safety falls out of the grammar — a separator cannot survive
/// slugification — and the result is confined to the store root regardless.
#[test]
fn a_traversal_shaped_name_is_slugified_into_the_day_directory() {
    let fx = Fixture::new();
    let scope = fx.project_scope();

    let stored = fx.put_in(&scope, "owner-1", "../../../etc/passwd.png", b"not-a-png");

    let day_dir = fx
        .project_root()
        .join(".openalpaca")
        .join("uploads")
        .join("2026-09-05");
    assert_eq!(
        PathBuf::from(&stored.asset.storage_path),
        day_dir.join("01-passwd.png")
    );
    assert!(
        !fx.project_root().join("etc").exists(),
        "nothing may be written outside the store"
    );
    let (_, rel_path) = fx.address(&stored.asset.id);
    assert_eq!(rel_path.as_deref(), Some("2026-09-05/01-passwd.png"));
}

/// The store's day directory holds the head and nothing else — the write
/// protocol's `.<stem>.tmp` is renamed, never left behind.
#[test]
fn a_completed_write_leaves_no_temp_file() {
    let fx = Fixture::new();
    let scope = fx.project_scope();
    fx.put_in(&scope, "owner-1", "notes.txt", b"hello");

    let files = files_under(&fx.uploads_root(&scope));
    assert_eq!(files, vec!["2026-09-05/01-notes.txt".to_string()]);
}

// ============================================================================
// The head is never clobbered
// ============================================================================

/// Rows are the source of truth for the *number*, but disk gets a veto over the
/// *name*. A file can outlive its row — a sweep whose `remove_file` failed, or a
/// crash between the rename and the commit — and its number is then handed out
/// again. The writer must take the next free one rather than rename over it.
#[test]
fn a_stray_file_at_the_next_slot_is_never_overwritten() {
    let fx = Fixture::new();
    let scope = fx.project_scope();

    let day_dir = fx.uploads_root(&scope).join("2026-09-05");
    std::fs::create_dir_all(&day_dir).unwrap();
    let stray = day_dir.join("01-notes.txt");
    std::fs::write(&stray, b"bytes with no row").unwrap();

    let stored = fx.put_in(&scope, "owner-1", "notes.txt", b"hello");

    assert_eq!(
        std::fs::read(&stray).unwrap(),
        b"bytes with no row",
        "the stray file must be left exactly as it was"
    );
    assert!(
        stored.asset.storage_path.ends_with("/02-notes.txt"),
        "the upload takes the next free slot: {}",
        stored.asset.storage_path
    );
    assert_eq!(std::fs::read(&stored.asset.storage_path).unwrap(), b"hello");
}

/// Placement identity *is* address identity. A `<project>/.openalpaca` symlinked
/// at another store — the home store here — puts both scopes' bytes in one
/// directory, so they must share one sequence space; keying the sequence off the
/// path the caller named would restart it at `01` and rename over a live file
/// whose row still points at it. `confine_to_root` cannot catch that: the
/// symlink is *at* the root it canonicalizes, not under it.
#[cfg(unix)]
#[test]
fn a_symlinked_store_root_cannot_open_a_second_sequence_space() {
    let fx = Fixture::new();
    std::os::unix::fs::symlink(fx.home_root(), fx.project_root().join(".openalpaca")).unwrap();

    let home = fx.put("owner-1", "notes.txt", b"hello");
    let project = fx.put_in(&fx.project_scope(), "owner-2", "notes.txt", b"other bytes");

    assert!(
        home.asset.storage_path.ends_with("/01-notes.txt"),
        "{}",
        home.asset.storage_path
    );
    assert!(
        project.asset.storage_path.ends_with("/02-notes.txt"),
        "the second write shares the first's sequence space: {}",
        project.asset.storage_path
    );
    assert_eq!(std::fs::read(&home.asset.storage_path).unwrap(), b"hello");
    assert_eq!(
        std::fs::read(&project.asset.storage_path).unwrap(),
        b"other bytes"
    );

    // One directory, one address space: the row records the store the bytes
    // reached, not the path the caller named.
    assert_eq!(fx.address(&project.asset.id).0, None);
}

// ============================================================================
// Dedup
// ============================================================================

#[test]
fn a_duplicate_sha_for_the_same_owner_writes_nothing() {
    let fx = Fixture::new();
    let scope = fx.project_scope();
    let first = fx.put_in(&scope, "owner-1", "notes.txt", b"hello");
    let before = files_under(&fx.uploads_root(&scope));

    // A different *name*, the same bytes: dedup keys off the sha256 column,
    // never off the path.
    let second = fx.put_in(&scope, "owner-1", "a-different-name.txt", b"hello");

    assert!(
        second.deduped,
        "the same owner + the same sha is a dedup hit"
    );
    assert_eq!(second.asset.id, first.asset.id);
    assert_eq!(second.asset.storage_path, first.asset.storage_path);
    assert_eq!(
        second.asset.filename, "notes.txt",
        "the first row is returned"
    );
    assert_eq!(
        files_under(&fx.uploads_root(&scope)),
        before,
        "a dedup hit writes no new file"
    );
    assert_eq!(
        fx.repo().total_storage_bytes().unwrap(),
        5,
        "and no second row"
    );
}

/// Dedup is the sha256 query, so it is store-blind: the same bytes uploaded
/// again with a *project* this time still resolve to the row that already
/// exists. Placement never gets a second opinion about identity.
#[test]
fn a_duplicate_sha_deduplicates_across_stores() {
    let fx = Fixture::new();
    let home = fx.put("owner-1", "notes.txt", b"hello");
    let second = fx.put_in(&fx.project_scope(), "owner-1", "notes.txt", b"hello");

    assert!(second.deduped);
    assert_eq!(second.asset.id, home.asset.id);
    assert_eq!(second.asset.storage_path, home.asset.storage_path);
}

#[test]
fn the_same_bytes_from_another_owner_get_their_own_row() {
    let fx = Fixture::new();
    let mine = fx.put("owner-1", "notes.txt", b"hello");
    let theirs = fx.put("owner-2", "notes.txt", b"hello");

    assert!(!theirs.deduped, "dedup is owner-scoped");
    assert_ne!(theirs.asset.id, mine.asset.id);
    assert_ne!(theirs.asset.storage_path, mine.asset.storage_path);
    assert!(theirs.asset.storage_path.ends_with("/02-notes.txt"));
}

// ============================================================================
// The rows that predate D2
// ============================================================================

/// Existing content-addressed blobs stay where they are (the re-home is Phase
/// 8): their `storage_path` is untouched by a new upload, they still resolve,
/// and a duplicate of their bytes dedups to them rather than being re-placed
/// under `uploads/`.
#[test]
fn pre_d2_content_addressed_rows_still_resolve_and_still_dedup() {
    let fx = Fixture::new();

    // A row exactly as the pre-D2 writer left it: sharded under state/assets,
    // no project_root, no rel_path.
    let sha = crate::content_io::sha256_hex(b"legacy");
    let legacy_path = interim_asset_storage_path(&sha).unwrap();
    std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
    std::fs::write(&legacy_path, b"legacy").unwrap();
    fx.repo()
        .insert(&FileAsset {
            id: "legacy-1".to_string(),
            owner_id: "owner-1".to_string(),
            sha256: sha,
            filename: "legacy.bin".to_string(),
            mime_type: "application/octet-stream".to_string(),
            size_bytes: 6,
            storage_path: legacy_path.to_string_lossy().to_string(),
            status: FileAssetStatus::Ready,
            extracted_text: None,
            extract_error: None,
            metadata_json: None,
            created_at: String::new(),
            updated_at: String::new(),
        })
        .unwrap();

    // A new upload does not disturb it.
    fx.put("owner-1", "new.txt", b"new bytes");
    let legacy = fx.repo().get_by_id("legacy-1").unwrap().expect("row");
    assert_eq!(legacy.storage_path, legacy_path.to_string_lossy());
    assert_eq!(std::fs::read(&legacy_path).unwrap(), b"legacy");

    // And re-uploading its bytes dedups to it — nothing is re-placed.
    let again = fx.put("owner-1", "legacy-again.bin", b"legacy");
    assert!(again.deduped);
    assert_eq!(again.asset.id, "legacy-1");
    assert_eq!(again.asset.storage_path, legacy_path.to_string_lossy());
}

// ============================================================================
// The row the rest of the system reads
// ============================================================================

#[test]
fn put_writes_the_row_the_repository_reads() {
    let fx = Fixture::new();
    let stored = fx.put_in(&fx.project_scope(), "owner-1", "notes.txt", b"hello");

    let asset = &stored.asset;
    assert!(!stored.deduped);
    assert_eq!(asset.owner_id, "owner-1");
    assert_eq!(asset.mime_type, "text/plain");
    assert_eq!(asset.size_bytes, 5);
    assert_eq!(asset.status, FileAssetStatus::Uploaded);
    assert!(
        !asset.created_at.is_empty(),
        "the row is read back, so the column defaults are real"
    );

    let row = fx.repo().get_by_id(&asset.id).unwrap().expect("row");
    assert_eq!(row.storage_path, asset.storage_path);
}

/// The quota counts `origin = 'upload'` bytes wherever they landed — a project
/// store is not a way to upload past the cap.
#[test]
fn the_quota_counts_uploads_in_every_store() {
    let fx = Fixture::new();
    fx.put("owner-1", "home.txt", b"12345");
    fx.put_in(&fx.project_scope(), "owner-1", "project.txt", b"1234567890");

    assert_eq!(fx.repo().total_storage_bytes().unwrap(), 15);
}

#[test]
fn a_second_upload_of_different_bytes_gets_its_own_row() {
    let fx = Fixture::new();
    let first = fx.put("owner-1", "notes.txt", b"hello");
    let second = fx.put("owner-1", "notes.txt", b"goodbye");

    assert!(!second.deduped);
    assert_ne!(second.asset.id, first.asset.id);
    assert_ne!(second.asset.storage_path, first.asset.storage_path);
    assert_eq!(std::fs::read(&first.asset.storage_path).unwrap(), b"hello");
    assert_eq!(
        std::fs::read(&second.asset.storage_path).unwrap(),
        b"goodbye"
    );
}
