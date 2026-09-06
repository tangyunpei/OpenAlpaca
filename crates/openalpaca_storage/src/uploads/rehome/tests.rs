//! The boot-time re-home of pre-D2 uploads, including every point a process can
//! die inside it.
//!
//! Nothing here touches a real root: every test points `OPENALPACA_HOME_STORE`
//! at a temp directory first.

use super::*;
use crate::store::tests::{HomeStoreGuard, interim_blob_path};
use tempfile::{TempDir, tempdir};

// ============================================================================
// Fixture
// ============================================================================

/// A temp home root (via `OPENALPACA_HOME_STORE`) and a temp database.
struct Fixture {
    home: TempDir,
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
            home,
            _env: env,
            _db_dir: db_dir,
            db,
        }
    }

    fn home_root(&self) -> PathBuf {
        self.home.path().canonicalize().unwrap()
    }

    /// `<home>/uploads` — where the re-home puts things.
    fn uploads(&self) -> PathBuf {
        self.home_root().join("uploads")
    }

    /// `<home>/state/assets` — where it takes them from.
    fn assets(&self) -> PathBuf {
        self.home_root().join("state").join("assets")
    }

    /// A row exactly as the pre-D2 writer left it: bytes sharded under
    /// `state/assets`, `origin = 'upload'`, no `rel_path`, no `project_root`.
    fn pre_d2(
        &self,
        id: &str,
        owner: &str,
        filename: &str,
        data: &[u8],
        created_at: &str,
    ) -> PathBuf {
        let blob = interim_blob_path(&crate::content_io::sha256_hex(data));
        fs::create_dir_all(blob.parent().unwrap()).unwrap();
        fs::write(&blob, data).unwrap();
        self.insert_row(id, owner, filename, &blob, data, created_at);
        blob
    }

    /// The row alone, addressing `storage_path` — the fixture for a second row
    /// sharing one blob, and for a row whose bytes are not there at all.
    fn insert_row(
        &self,
        id: &str,
        owner: &str,
        filename: &str,
        storage_path: &Path,
        data: &[u8],
        created_at: &str,
    ) {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO file_assets
                        (id, owner_id, sha256, filename, mime_type, size_bytes, storage_path,
                         status, origin, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 'text/plain', ?5, ?6, 'ready', 'upload', ?7, ?7)",
                    rusqlite::params![
                        id,
                        owner,
                        crate::content_io::sha256_hex(data),
                        filename,
                        data.len() as i64,
                        storage_path.to_string_lossy(),
                        created_at,
                    ],
                )?;
                Ok(())
            })
            .unwrap();
    }

    /// `(storage_path, rel_path, project_root)` — the address a row records.
    fn address(&self, id: &str) -> (String, Option<String>, Option<String>) {
        self.db
            .with_connection(|conn| {
                Ok(conn.query_row(
                    "SELECT storage_path, rel_path, project_root FROM file_assets WHERE id = ?1",
                    rusqlite::params![id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?)
            })
            .unwrap()
    }

    /// Whether — and, if so, when — a row was marked `missing_since` (T23's
    /// convention). `Some` once [`mark_missing`] has run for it, `None` before.
    fn missing_since(&self, id: &str) -> Option<String> {
        self.db
            .with_connection(|conn| {
                Ok(conn.query_row(
                    "SELECT missing_since FROM file_assets WHERE id = ?1",
                    rusqlite::params![id],
                    |row| row.get(0),
                )?)
            })
            .unwrap()
    }

    /// The file names in one day directory, sorted — the assertion that a resumed
    /// pass left one file and not two.
    fn day_entries(&self, day: &str) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.uploads().join(day))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

const DAY: &str = "2026-03-04";
const CREATED: &str = "2026-03-04 09:15:00";

// ============================================================================
// The move itself
// ============================================================================

#[test]
fn a_pre_d2_upload_takes_a_d2_address_and_its_blob_is_deleted() {
    let fx = Fixture::new();
    let blob = fx.pre_d2(
        "up-1",
        "owner-1",
        "Quarterly Notes.TXT",
        b"legacy bytes",
        CREATED,
    );

    let summary = rehome_inner(&fx.db, None);
    assert_eq!(
        summary,
        RehomeSummary {
            moved: 1,
            ..Default::default()
        }
    );

    // The D2 address, from the row's own created_at and the T26 grammar.
    let head = fx.uploads().join(DAY).join("01-quarterly-notes.txt");
    assert_eq!(fs::read(&head).unwrap(), b"legacy bytes");

    let (storage_path, rel_path, project_root) = fx.address("up-1");
    assert_eq!(storage_path, head.to_string_lossy());
    assert_eq!(
        rel_path.as_deref(),
        Some("2026-03-04/01-quarterly-notes.txt")
    );
    assert_eq!(
        project_root, None,
        "a pre-D2 upload re-homes into the home store"
    );

    assert!(!blob.exists(), "the interim blob outlived the move");
    assert!(
        !fx.assets().exists(),
        "the interim directory outlived the last blob in it"
    );
}

#[test]
fn a_second_pass_moves_nothing() {
    let fx = Fixture::new();
    fx.pre_d2("up-1", "owner-1", "notes.txt", b"legacy bytes", CREATED);

    rehome_inner(&fx.db, None);
    let before = fx.address("up-1");

    assert_eq!(rehome_inner(&fx.db, None), RehomeSummary::default());
    assert_eq!(
        fx.address("up-1"),
        before,
        "the second pass re-addressed a row"
    );
    assert_eq!(fx.day_entries(DAY), vec!["01-notes.txt"]);
    assert!(!fx.assets().exists());
}

/// Pre-D2 placement was content-addressed, so two rows can share one blob. Each
/// gets its own copy at its own address, and the blob is unlinked only once the
/// last of them has moved — otherwise the second row's source would be gone.
#[test]
fn two_rows_sharing_one_blob_each_get_their_own_copy() {
    let fx = Fixture::new();
    let blob = fx.pre_d2("up-a", "owner-1", "shared.txt", b"same bytes", CREATED);
    fx.insert_row(
        "up-b",
        "owner-2",
        "theirs.txt",
        &blob,
        b"same bytes",
        CREATED,
    );

    let summary = rehome_inner(&fx.db, None);
    assert_eq!(
        summary,
        RehomeSummary {
            moved: 2,
            ..Default::default()
        }
    );

    assert_eq!(fx.day_entries(DAY), vec!["01-shared.txt", "02-theirs.txt"]);
    for (id, name) in [("up-a", "01-shared.txt"), ("up-b", "02-theirs.txt")] {
        let head = fx.uploads().join(DAY).join(name);
        assert_eq!(fs::read(&head).unwrap(), b"same bytes");
        assert_eq!(fx.address(id).0, head.to_string_lossy());
    }
    assert!(!blob.exists(), "the shared blob outlived both rows");
    assert!(!fx.assets().exists());
}

// ============================================================================
// Every point the process can die
// ============================================================================

/// Killed after the O_EXCL reservation: an empty file stands at the head name and
/// the row has not moved. The next pass reclaims the reservation (R33) rather
/// than bumping past it, so the resumed move lands at the same address.
#[test]
fn a_kill_after_the_reservation_resumes_onto_the_same_address() {
    let fx = Fixture::new();
    let blob = fx.pre_d2("up-1", "owner-1", "notes.txt", b"legacy bytes", CREATED);

    rehome_inner(&fx.db, Some(Stop::Reserve));
    let head = fx.uploads().join(DAY).join("01-notes.txt");
    assert_eq!(fs::read(&head).unwrap(), b"", "the reservation is empty");
    assert_eq!(
        fx.address("up-1"),
        (blob.to_string_lossy().into_owned(), None, None)
    );
    assert!(blob.exists());

    assert_eq!(
        rehome_inner(&fx.db, None),
        RehomeSummary {
            moved: 1,
            ..Default::default()
        }
    );
    assert_eq!(fx.day_entries(DAY), vec!["01-notes.txt"]);
    assert_eq!(fs::read(&head).unwrap(), b"legacy bytes");
    assert_eq!(
        fx.address("up-1").1.as_deref(),
        Some("2026-03-04/01-notes.txt")
    );
    assert!(!blob.exists());
    assert!(!fx.assets().exists());
}

/// Killed after the bytes landed but before the commit: the row is the commit, so
/// the file belongs to nobody and the next pass reclaims it. The resumed move
/// leaves one file, not two.
#[test]
fn a_kill_after_the_copy_resumes_without_stranding_a_second_file() {
    let fx = Fixture::new();
    let blob = fx.pre_d2("up-1", "owner-1", "notes.txt", b"legacy bytes", CREATED);

    rehome_inner(&fx.db, Some(Stop::Copy));
    let head = fx.uploads().join(DAY).join("01-notes.txt");
    assert_eq!(fs::read(&head).unwrap(), b"legacy bytes");
    assert_eq!(
        fx.address("up-1"),
        (blob.to_string_lossy().into_owned(), None, None)
    );
    assert!(blob.exists(), "nothing is unlinked before the commit");

    assert_eq!(
        rehome_inner(&fx.db, None),
        RehomeSummary {
            moved: 1,
            ..Default::default()
        }
    );
    assert_eq!(fx.day_entries(DAY), vec!["01-notes.txt"]);
    assert_eq!(fs::read(&head).unwrap(), b"legacy bytes");
    assert_eq!(
        fx.address("up-1").1.as_deref(),
        Some("2026-03-04/01-notes.txt")
    );
    assert!(!blob.exists());
    assert!(!fx.assets().exists());
}

/// Killed between the commit and the unlink: the row is already at its D2
/// address, so *this* pass never learns where its blob was. But the row still
/// carries the `sha256` that named the old address, so the next boot's
/// disposal pass can reconstruct it, confirm the D2 copy is intact, and
/// reclaim the stray — it is this pass's own residue, not a human's file
/// (Important 2, fix round 1).
#[test]
fn a_kill_after_the_commit_strands_a_blob_the_next_boot_reclaims() {
    let fx = Fixture::new();
    let blob = fx.pre_d2("up-1", "owner-1", "notes.txt", b"legacy bytes", CREATED);

    rehome_inner(&fx.db, Some(Stop::Commit));
    let head = fx.uploads().join(DAY).join("01-notes.txt");
    assert_eq!(fs::read(&head).unwrap(), b"legacy bytes");
    assert_eq!(
        fx.address("up-1").1.as_deref(),
        Some("2026-03-04/01-notes.txt")
    );
    assert!(blob.exists(), "the unlink is the step that did not run");

    assert_eq!(rehome_inner(&fx.db, None), RehomeSummary::default());
    assert_eq!(fx.day_entries(DAY), vec!["01-notes.txt"]);
    assert!(
        !blob.exists(),
        "the stray is reclaimed: its row already moved to an intact D2 copy"
    );
    assert!(
        !fx.assets().exists(),
        "nothing was left once its only stray was reclaimed"
    );
}

// ============================================================================
// What the pass refuses to do
// ============================================================================

/// A file the pass did not put there keeps the directory alive. Deleting it is
/// not this pass's call, and saying so at `warn` is the whole of its response.
#[test]
fn a_stray_file_keeps_the_interim_directory() {
    let fx = Fixture::new();
    let blob = fx.pre_d2("up-1", "owner-1", "notes.txt", b"legacy bytes", CREATED);
    let stray = fx.assets().join("a-human-put-this-here.txt");
    fs::write(&stray, b"mine").unwrap();

    assert_eq!(
        rehome_inner(&fx.db, None),
        RehomeSummary {
            moved: 1,
            ..Default::default()
        }
    );
    assert!(!blob.exists(), "the row's own blob still moves");
    assert_eq!(fs::read(&stray).unwrap(), b"mine", "the stray is untouched");
    assert!(fx.assets().exists());
}

/// A row whose bytes are gone cannot be re-homed: rewriting its address would
/// only move the hole. It is marked `missing_since` (T23's own convention)
/// instead, and a marked row leaves the `remaining` set — its blob being gone
/// is not a reason to keep `state/assets` alive forever (Important 1, fix
/// round 1).
#[test]
fn a_row_whose_blob_is_missing_is_marked_and_disposal_proceeds() {
    let fx = Fixture::new();
    fs::create_dir_all(fx.assets()).unwrap();
    let ghost = interim_blob_path(&crate::content_io::sha256_hex(b"gone"));
    fx.insert_row("up-1", "owner-1", "gone.txt", &ghost, b"gone", CREATED);

    assert_eq!(
        rehome_inner(&fx.db, None),
        RehomeSummary {
            missing: 1,
            ..Default::default()
        }
    );
    let (storage_path, rel_path, _project_root) = fx.address("up-1");
    assert_eq!(storage_path, ghost.to_string_lossy());
    assert_eq!(
        rel_path, None,
        "a missing blob is never given a bytes-less D2 address"
    );
    assert!(
        fx.missing_since("up-1").is_some(),
        "the row is marked so this warning does not repeat"
    );
    assert!(
        !fx.assets().exists(),
        "the only remaining row is marked missing, so disposal proceeds"
    );
}

/// Marking is idempotent: a row already marked missing on an earlier boot is
/// neither re-marked nor re-warned about on a later one.
#[tracing_test::traced_test]
#[test]
fn a_row_already_marked_missing_does_not_rewarn_on_a_second_boot() {
    let fx = Fixture::new();
    fs::create_dir_all(fx.assets()).unwrap();
    let ghost = interim_blob_path(&crate::content_io::sha256_hex(b"gone"));
    fx.insert_row("up-1", "owner-1", "gone.txt", &ghost, b"gone", CREATED);

    rehome_inner(&fx.db, None);
    let marked_at = fx
        .missing_since("up-1")
        .expect("the first pass marks the row");

    assert_eq!(
        rehome_inner(&fx.db, None),
        RehomeSummary {
            missing: 1,
            ..Default::default()
        }
    );
    assert_eq!(
        fx.missing_since("up-1"),
        Some(marked_at),
        "a row already marked missing is not re-marked"
    );

    logs_assert(|lines: &[&str]| {
        let warns = lines
            .iter()
            .filter(|l| l.contains("WARN") && l.contains("has no bytes at"))
            .count();
        match warns {
            1 => Ok(()),
            n => Err(format!(
                "expected exactly one WARN across both boots, saw {n}"
            )),
        }
    });
}

/// The day directory comes from the row's `created_at`, not from today. An
/// unreadable value is the one case that falls back — the address has to be some
/// day, and the row keeps its own record of when it arrived.
#[test]
fn an_unreadable_created_at_falls_back_to_today() {
    let fx = Fixture::new();
    fx.pre_d2(
        "up-1",
        "owner-1",
        "notes.txt",
        b"legacy bytes",
        "not a date",
    );

    assert_eq!(
        rehome_inner(&fx.db, None),
        RehomeSummary {
            moved: 1,
            ..Default::default()
        }
    );
    let today = Utc::now().format("%Y-%m-%d").to_string();
    assert_eq!(
        fx.address("up-1").1.as_deref(),
        Some(format!("{today}/01-notes.txt").as_str())
    );
}
