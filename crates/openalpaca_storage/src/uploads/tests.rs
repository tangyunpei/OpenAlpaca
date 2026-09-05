//! The one upload writer, from both callers' point of view.
//!
//! Nothing here touches a real root: every test points
//! `OPENALPACA_HOME_STORE` at a temp directory first.

use super::*;
use crate::FileAssetRepository;
use crate::store::tests::HomeStoreGuard;
use std::path::PathBuf;
use tempfile::{TempDir, tempdir};

// ============================================================================
// Fixture
// ============================================================================

/// A temp home root (via `OPENALPACA_HOME_STORE`) plus a temp database.
struct Fixture {
    _home: TempDir,
    _env: HomeStoreGuard,
    _db_dir: TempDir,
    db: Database,
}

impl Fixture {
    fn new() -> Self {
        let home = tempdir().unwrap();
        let env = HomeStoreGuard::set(&home.path().canonicalize().unwrap());
        let db_dir = tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();
        Self {
            _home: home,
            _env: env,
            _db_dir: db_dir,
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

    fn put(&self, owner: &str, filename: &str, data: &[u8]) -> StoredUpload {
        self.store()
            .put(NewUpload {
                owner_id: owner,
                filename,
                mime_type: "text/plain",
                data,
            })
            .unwrap()
    }
}

/// Every file under `dir`, recursively, as paths relative to it.
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
// The writer
// ============================================================================

#[test]
fn put_writes_the_bytes_and_the_row() {
    let fx = Fixture::new();
    let stored = fx.put("owner-1", "notes.txt", b"hello");

    assert!(!stored.deduped);
    let asset = &stored.asset;
    assert_eq!(asset.owner_id, "owner-1");
    assert_eq!(asset.filename, "notes.txt");
    assert_eq!(asset.mime_type, "text/plain");
    assert_eq!(asset.size_bytes, 5);
    assert_eq!(asset.status, FileAssetStatus::Uploaded);

    let on_disk = std::fs::read(&asset.storage_path).expect("the bytes are on disk");
    assert_eq!(on_disk, b"hello");

    // The row is readable through the repository that owns upload rows, and it
    // counts against the upload quota wherever the bytes landed.
    let row = fx.repo().get_by_id(&asset.id).unwrap().expect("row");
    assert_eq!(row.storage_path, asset.storage_path);
    assert_eq!(fx.repo().total_storage_bytes().unwrap(), 5);
}

#[test]
fn a_duplicate_sha_for_the_same_owner_writes_nothing() {
    let fx = Fixture::new();
    let first = fx.put("owner-1", "notes.txt", b"hello");
    let before = files_under(&fx.home_root());

    // A different *name*, the same bytes: dedup keys off the sha256 column,
    // never off the path.
    let second = fx.put("owner-1", "a-different-name.txt", b"hello");

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
        files_under(&fx.home_root()),
        before,
        "a dedup hit writes no new file"
    );
    assert_eq!(
        fx.repo().total_storage_bytes().unwrap(),
        5,
        "and no second row"
    );
}

#[test]
fn the_same_bytes_from_another_owner_get_their_own_row() {
    let fx = Fixture::new();
    let mine = fx.put("owner-1", "notes.txt", b"hello");
    let theirs = fx.put("owner-2", "notes.txt", b"hello");

    assert!(!theirs.deduped, "dedup is owner-scoped");
    assert_ne!(theirs.asset.id, mine.asset.id);
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
