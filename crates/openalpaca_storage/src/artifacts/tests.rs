use super::*;
use crate::FileAssetRepository;
use crate::store::tests::HomeStoreGuard;
use std::path::Path;
use tempfile::{TempDir, tempdir};

// ============================================================================
// Fixture
// ============================================================================

/// A whole world: a temp home root (via `OPENALPACA_HOME_STORE`), a temp
/// project root, and a temp database. Nothing here ever touches a real root.
struct Fixture {
    _home: TempDir,
    _env: HomeStoreGuard,
    _db_dir: TempDir,
    project: TempDir,
    db: Database,
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

    fn store(&self) -> ArtifactStore<'_> {
        ArtifactStore::new(&self.db)
    }

    /// The canonical project root — canonical because `project_root` is the
    /// canonical-path string everywhere (§4.8) and macOS's `/var` is a symlink.
    fn project_root(&self) -> PathBuf {
        self.project.path().canonicalize().unwrap()
    }

    fn scope(&self) -> StoreScope {
        StoreScope::Project(self.project_root())
    }

    fn home_root(&self) -> PathBuf {
        self._home.path().canonicalize().unwrap()
    }

    /// `<project>/.openalpaca/artifacts` — the root `rel_path` is relative to.
    fn artifacts_root(&self) -> PathBuf {
        self.project_root().join(".openalpaca").join("artifacts")
    }

    /// A `task` row, so `file_assets.task_id`'s foreign key is satisfiable.
    fn task(&self, id: &str, title: &str) {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO task (id, title, created_by, source_lane) VALUES (?1, ?2, 'test', 'test')",
                    rusqlite::params![id, title],
                )?;
                Ok(())
            })
            .unwrap();
    }
}

const OWNER: &str = "owner-1";

fn at(day: u32) -> DateTime<Utc> {
    use chrono::TimeZone;
    Utc.with_ymd_and_hms(2026, 9, day, 12, 0, 0).unwrap()
}

/// Restores the crash hook even if the test panics mid-assertion.
struct CrashGuard;

impl CrashGuard {
    fn after(step: WriteStep) -> Self {
        crash_after(step);
        Self
    }
}

impl Drop for CrashGuard {
    fn drop(&mut self) {
        clear_crash();
    }
}

// ============================================================================
// put — placement and the clean head
// ============================================================================

#[test]
fn put_creates_the_head_at_the_clean_path() {
    let f = Fixture::new();
    f.task("3f2a1b7c-0000-4000-8000-000000000000", "Connector audit");
    let scope = f.scope();

    let mut new = NewArtifact::new(
        OWNER,
        &scope,
        ArtifactKind::Markdown,
        "Connector audit findings",
        b"a\nb\nc\n",
    );
    new.task_id = Some("3f2a1b7c-0000-4000-8000-000000000000");
    new.task_title = Some("Connector audit");
    new.created = at(1);
    new.agent_id = Some("review_agent::a1b2c3d4");
    new.agent_template_id = Some("review_agent");
    new.summary = Some("3 findings");

    let (record, created) = f.store().put(new).unwrap();

    assert!(created, "the first put creates");
    assert_eq!(record.version, 1);
    assert_eq!(record.version_count, 1);
    assert_eq!(record.origin, ArtifactOrigin::Produced);
    assert_eq!(record.kind, Some(ArtifactKind::Markdown));
    assert_eq!(record.name, "01-connector-audit-findings.md");
    assert_eq!(
        record.rel_path.as_deref(),
        Some("2026-09-01-connector-audit-3f2a1b7c/01-connector-audit-findings.md")
    );
    assert_eq!(
        record.project_root.as_deref(),
        Some(f.project_root().to_string_lossy().as_ref())
    );
    assert_eq!(record.size_bytes, 6);
    assert!(!record.missing());
    assert!(!record.pinned);

    let head = f
        .artifacts_root()
        .join("2026-09-01-connector-audit-3f2a1b7c/01-connector-audit-findings.md");
    assert_eq!(fs::read_to_string(&head).unwrap(), "a\nb\nc\n");
    assert_eq!(record.storage_path, head.to_string_lossy());
    assert!(
        !head.parent().unwrap().join(".versions").exists(),
        "a first put must not create .versions/"
    );
    assert_no_tmp_leftovers(head.parent().unwrap());
}

#[test]
fn a_taskless_put_lands_under_loose_date() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(
        OWNER,
        &scope,
        ArtifactKind::Html,
        "Weekly report",
        b"<p>hi</p>",
    );
    new.created = at(1);

    let (record, _) = f.store().put(new).unwrap();

    assert_eq!(
        record.rel_path.as_deref(),
        Some("loose/2026-09-01/01-weekly-report.html")
    );
    assert!(record.task_id.is_none());
    assert!(
        f.artifacts_root()
            .join("loose/2026-09-01/01-weekly-report.html")
            .exists()
    );
}

#[test]
fn the_home_scope_records_a_null_project_root() {
    let f = Fixture::new();
    let scope = StoreScope::Home;
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"x");
    new.created = at(1);

    let (record, _) = f.store().put(new).unwrap();

    assert!(
        record.project_root.is_none(),
        "the home root is the baseline"
    );
    let head = f.home_root().join("artifacts/loose/2026-09-01/01-notes.md");
    assert!(head.exists(), "missing {}", head.display());
    assert_eq!(record.storage_path, head.to_string_lossy());
}

// ============================================================================
// put — the version rotate (Verify: put→put leaves head clean + v1 in .versions/)
// ============================================================================

#[test]
fn a_second_put_leaves_the_head_clean_and_v1_in_versions() {
    let f = Fixture::new();
    let scope = f.scope();

    let mut first = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"a\nb\nc\n");
    first.created = at(1);
    let (v1, created_first) = f.store().put(first).unwrap();
    assert!(created_first);

    let mut second = NewArtifact::new(
        OWNER,
        &scope,
        ArtifactKind::Markdown,
        "Notes",
        b"a\nb\nc\nd\n",
    );
    second.created = at(1);
    second.note = Some("added d");
    let (v2, created_second) = f.store().put(second).unwrap();

    assert!(
        !created_second,
        "the second put supersedes, it does not create"
    );
    assert_eq!(v1.id, v2.id, "the address is the identity");
    assert_eq!(v2.version, 2);
    assert_eq!(v2.version_count, 2);

    let head = f.artifacts_root().join("loose/2026-09-01/01-notes.md");
    assert_eq!(fs::read_to_string(&head).unwrap(), "a\nb\nc\nd\n");

    let v1_file = f
        .artifacts_root()
        .join("loose/2026-09-01/.versions/01-notes/v1.md");
    assert_eq!(fs::read_to_string(&v1_file).unwrap(), "a\nb\nc\n");

    // The head is never duplicated into .versions/.
    let version_dir = f
        .artifacts_root()
        .join("loose/2026-09-01/.versions/01-notes");
    let names: Vec<String> = fs::read_dir(&version_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["v1.md".to_string()]);

    let rows = f.store().versions(&v2.id).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].version, 2, "newest first");
    assert_eq!(rows[0].rel_path, "loose/2026-09-01/01-notes.md");
    assert_eq!(rows[0].note.as_deref(), Some("added d"));
    assert_eq!(rows[0].added_lines, Some(1));
    assert_eq!(rows[0].removed_lines, Some(0));
    assert_eq!(rows[1].version, 1);
    assert_eq!(
        rows[1].rel_path,
        "loose/2026-09-01/.versions/01-notes/v1.md"
    );
    assert_eq!(rows[1].added_lines, None, "NULL on v1");
}

#[test]
fn the_sequence_is_assigned_once_and_retained_across_versions() {
    let f = Fixture::new();
    let scope = f.scope();

    let mut a = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Alpha", b"1");
    a.created = at(1);
    let (a1, _) = f.store().put(a).unwrap();
    assert_eq!(a1.name, "01-alpha.md");

    let mut b = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Beta", b"1");
    b.created = at(1);
    let (b1, _) = f.store().put(b).unwrap();
    assert_eq!(b1.name, "02-beta.md");

    // Alpha again: the same NN-, not a fresh 03-.
    let mut a2 = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Alpha", b"2");
    a2.created = at(1);
    let (a2, created) = f.store().put(a2).unwrap();
    assert!(!created);
    assert_eq!(a2.id, a1.id);
    assert_eq!(a2.name, "01-alpha.md");
    assert_eq!(a2.version, 2);

    // Beta again keeps 02-.
    let mut b2 = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Beta", b"2");
    b2.created = at(1);
    let (b2, _) = f.store().put(b2).unwrap();
    assert_eq!(b2.name, "02-beta.md");
    assert_eq!(b2.id, b1.id);
}

#[test]
fn a_different_extension_is_a_different_artifact() {
    let f = Fixture::new();
    let scope = f.scope();

    let mut md = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"1");
    md.created = at(1);
    let (md, _) = f.store().put(md).unwrap();

    let mut txt = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"1");
    txt.created = at(1);
    txt.name_hint = Some("notes.txt");
    let (txt, created) = f.store().put(txt).unwrap();

    assert!(created);
    assert_ne!(md.id, txt.id);
    assert_eq!(md.name, "01-notes.md");
    assert_eq!(txt.name, "02-notes.txt");
}

#[test]
fn put_refuses_to_supersede_another_owners_artifact() {
    let f = Fixture::new();
    let scope = f.scope();

    let mut mine = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"1");
    mine.created = at(1);
    f.store().put(mine).unwrap();

    let mut theirs = NewArtifact::new("owner-2", &scope, ArtifactKind::Markdown, "Notes", b"2");
    theirs.created = at(1);
    let err = f.store().put(theirs).unwrap_err();
    assert!(err.to_string().contains("owner"), "unexpected error: {err}");
}

// ============================================================================
// put — crash injection between the write-protocol steps
// ============================================================================

/// The bytes of every version of a two-put artifact, wherever the protocol may
/// have parked them. Asserts nothing is ever *partial*.
fn assert_nothing_is_truncated(dir: &Path, old: &str, new: &str) {
    let head = dir.join("01-notes.md");
    let rotated = dir.join(".versions/01-notes/v1.md");
    for (label, path) in [("head", &head), ("v1", &rotated)] {
        if path.exists() {
            let body = fs::read_to_string(path).unwrap();
            assert!(
                body == old || body == new,
                "{label} at {} is neither the whole old nor the whole new content: {body:?}",
                path.display()
            );
        }
    }
    assert!(
        head.exists() || rotated.exists(),
        "both the head and its rotated version are gone — the old bytes were destroyed"
    );
}

/// The protocol's `.<stem>.tmp` must never outlive the call that created it —
/// on the success path *and* on every error or crash path.
fn assert_no_tmp_leftovers(dir: &Path) {
    let leftovers: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "tmp leftovers in {}: {leftovers:?}",
        dir.display()
    );
}

/// Runs one crash injection against an artifact that already has v1, and
/// returns the run directory plus the post-crash record.
fn crash_during_second_put(step: WriteStep) -> (Fixture, PathBuf, String) {
    let f = Fixture::new();
    let scope = f.scope();

    let mut first = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"old\n");
    first.created = at(1);
    let (v1, _) = f.store().put(first).unwrap();

    {
        let _crash = CrashGuard::after(step);
        let mut second = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"new\n");
        second.created = at(1);
        let err = f.store().put(second).unwrap_err();
        assert!(
            err.to_string().contains("simulated crash"),
            "expected the injected failure, got: {err}"
        );
    }

    let dir = f.artifacts_root().join("loose/2026-09-01");
    (f, dir, v1.id)
}

#[test]
fn a_crash_after_the_tmp_write_leaves_the_head_fully_old() {
    let (f, dir, id) = crash_during_second_put(WriteStep::TmpWritten);
    assert_nothing_is_truncated(&dir, "old\n", "new\n");
    assert_no_tmp_leftovers(&dir);
    assert_eq!(
        fs::read_to_string(dir.join("01-notes.md")).unwrap(),
        "old\n"
    );
    assert!(!dir.join(".versions").exists());
    let record = f.store().get(&id, OWNER).unwrap().unwrap();
    assert_eq!(record.version, 1, "the transaction rolled back");
    assert_eq!(record.version_count, 1);
    assert_eq!(f.store().versions(&id).unwrap().len(), 1);
}

#[test]
fn a_crash_after_the_fsync_leaves_the_head_fully_old() {
    let (f, dir, id) = crash_during_second_put(WriteStep::Fsynced);
    assert_nothing_is_truncated(&dir, "old\n", "new\n");
    assert_no_tmp_leftovers(&dir);
    assert_eq!(
        fs::read_to_string(dir.join("01-notes.md")).unwrap(),
        "old\n"
    );
    let record = f.store().get(&id, OWNER).unwrap().unwrap();
    assert_eq!(record.version, 1);
}

#[test]
fn a_crash_after_the_rotate_keeps_the_old_bytes_whole_under_versions() {
    let (f, dir, id) = crash_during_second_put(WriteStep::Rotated);
    assert_nothing_is_truncated(&dir, "old\n", "new\n");
    assert_no_tmp_leftovers(&dir);
    // The specified protocol renames the head away before renaming the tmp in,
    // so this is the one window where the head path itself is absent — the old
    // bytes are whole and addressable at the rotated path.
    assert!(!dir.join("01-notes.md").exists());
    assert_eq!(
        fs::read_to_string(dir.join(".versions/01-notes/v1.md")).unwrap(),
        "old\n"
    );
    let record = f.store().get(&id, OWNER).unwrap().unwrap();
    assert_eq!(record.version, 1, "the transaction rolled back");
    assert_eq!(f.store().versions(&id).unwrap().len(), 1);
}

#[test]
fn a_crash_after_the_head_rename_leaves_the_head_fully_new() {
    let (f, dir, id) = crash_during_second_put(WriteStep::HeadRenamed);
    assert_nothing_is_truncated(&dir, "old\n", "new\n");
    assert_no_tmp_leftovers(&dir);
    assert_eq!(
        fs::read_to_string(dir.join("01-notes.md")).unwrap(),
        "new\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join(".versions/01-notes/v1.md")).unwrap(),
        "old\n"
    );
    let record = f.store().get(&id, OWNER).unwrap().unwrap();
    assert_eq!(record.version, 1, "the transaction rolled back");
}

#[test]
fn a_put_after_a_crash_still_succeeds() {
    let (f, dir, id) = crash_during_second_put(WriteStep::Rotated);
    let scope = f.scope();
    let mut retry = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"new\n");
    retry.created = at(1);
    let (record, created) = f.store().put(retry).unwrap();
    assert!(!created);
    assert_eq!(record.id, id);
    assert_eq!(record.version, 2);
    assert_eq!(
        fs::read_to_string(dir.join("01-notes.md")).unwrap(),
        "new\n"
    );

    // The interrupted attempt had already rotated v1's bytes to disk; the
    // recovering put must point v1's row at where they actually are, not leave
    // it claiming the head path it no longer owns.
    let rows = f.store().versions(&id).unwrap();
    let v1 = rows.iter().find(|r| r.version == 1).unwrap();
    assert_eq!(v1.rel_path, "loose/2026-09-01/.versions/01-notes/v1.md");
    let path = f.store().resolve_content(&id, Some(1)).unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "old\n");
    assert_no_tmp_leftovers(&dir);
}

#[test]
fn a_put_after_an_interrupted_head_rename_never_rotates_over_v1() {
    // The crash at `HeadRenamed` leaves the *uncommitted* new bytes at the head
    // and the true, committed v1 at `.versions/01-notes/v1.md` (the row rolled
    // back to v1 with sha("old\n")). A recovering put must not rotate the
    // orphaned head over v1 — that would destroy v1's only copy and leave v1's
    // row describing bytes of another version.
    let (f, dir, id) = crash_during_second_put(WriteStep::HeadRenamed);
    let v1_sha = f.store().versions(&id).unwrap()[0].sha256.clone();
    assert_eq!(v1_sha, sha256_hex(b"old\n"), "the row still describes v1");

    let scope = f.scope();
    let mut retry = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"newer\n");
    retry.created = at(1);
    let (record, created) = f.store().put(retry).unwrap();
    assert!(!created);
    assert_eq!(record.id, id);
    assert_eq!(record.version, 2);

    assert_eq!(
        fs::read_to_string(dir.join("01-notes.md")).unwrap(),
        "newer\n",
        "the head is the newest content"
    );
    assert_eq!(
        fs::read_to_string(dir.join(".versions/01-notes/v1.md")).unwrap(),
        "old\n",
        "v1's committed bytes must survive the recovering put"
    );
    let mut on_disk: Vec<String> = fs::read_dir(dir.join(".versions/01-notes"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    on_disk.sort();
    assert_eq!(
        on_disk,
        vec!["v1.md".to_string()],
        "the orphaned head is discarded, not parked in .versions/"
    );

    let rows = f.store().versions(&id).unwrap();
    assert_eq!(rows.len(), 2);
    let v1 = rows.iter().find(|r| r.version == 1).unwrap();
    assert_eq!(v1.rel_path, "loose/2026-09-01/.versions/01-notes/v1.md");
    assert_eq!(
        v1.sha256, v1_sha,
        "v1's row sha still describes v1's own bytes"
    );
    let path = f.store().resolve_content(&id, Some(1)).unwrap();
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "old\n",
        "resolve_content(v1) serves v1's bytes, not another version's"
    );
    assert_no_tmp_leftovers(&dir);
}

#[test]
fn an_interrupted_head_rename_leaves_v1_readable_without_a_put() {
    // The same interrupted state, read rather than written: `resolve_content`
    // and `verify` must find everything present and destroy nothing.
    let (f, dir, id) = crash_during_second_put(WriteStep::HeadRenamed);

    let root = f.project_root().to_string_lossy().to_string();
    assert_eq!(
        f.store().verify(Some(&root)).unwrap(),
        0,
        "the head is present, so nothing is marked missing"
    );
    let record = f.store().get(&id, OWNER).unwrap().unwrap();
    assert!(!record.missing());
    assert_eq!(record.version, 1);

    // v1 *is* the current version, so it resolves to the head — whose bytes the
    // interrupted put replaced. That stale sha is §4.8's hand-edit row (the
    // next put or `verify` records it as a version — Phase 8). What must not
    // happen is the loss of v1's bytes: they are whole under `.versions/`.
    let head = f.store().resolve_content(&id, None).unwrap();
    assert_eq!(head, dir.join("01-notes.md"));
    assert_eq!(f.store().resolve_content(&id, Some(1)).unwrap(), head);
    assert_eq!(
        fs::read_to_string(dir.join(".versions/01-notes/v1.md")).unwrap(),
        "old\n",
        "reads must not disturb the committed v1 bytes"
    );
    assert_eq!(
        f.store().versions(&id).unwrap()[0].sha256,
        sha256_hex(b"old\n")
    );
    assert_no_tmp_leftovers(&dir);
}

// ============================================================================
// put — pruning
// ============================================================================

#[test]
fn pruning_keeps_the_head_and_the_newest_versions() {
    let f = Fixture::new();
    let scope = f.scope();

    for n in 1..=5u32 {
        let body = format!("v{n}\n");
        let mut new = NewArtifact::new(
            OWNER,
            &scope,
            ArtifactKind::Markdown,
            "Notes",
            body.as_bytes(),
        );
        new.created = at(1);
        new.max_versions = Some(3);
        f.store().put(new).unwrap();
    }

    let dir = f.artifacts_root().join("loose/2026-09-01");
    assert_eq!(fs::read_to_string(dir.join("01-notes.md")).unwrap(), "v5\n");

    let id = f
        .store()
        .list(&ArtifactQuery::new(OWNER))
        .unwrap()
        .0
        .remove(0)
        .id;
    let versions = f.store().versions(&id).unwrap();
    let kept: Vec<u32> = versions.iter().map(|v| v.version).collect();
    assert_eq!(kept, vec![5, 4, 3], "head plus the newest two");

    let record = f.store().get(&id, OWNER).unwrap().unwrap();
    assert_eq!(record.version_count, 3);

    let mut on_disk: Vec<String> = fs::read_dir(dir.join(".versions/01-notes"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    on_disk.sort();
    assert_eq!(
        on_disk,
        vec!["v3.md".to_string(), "v4.md".to_string()],
        "the pruned version files are deleted with their rows"
    );
}

// ============================================================================
// The produced row and the upload machinery (Verify: sweep + quota)
// ============================================================================

#[test]
fn a_produced_row_is_invisible_to_the_sweep_and_the_quota() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"body");
    new.created = at(1);
    let (record, _) = f.store().put(new).unwrap();

    // Age the row well past any grace period.
    f.db.with_connection(|conn| {
        conn.execute(
            "UPDATE file_assets SET created_at = datetime('now', '-30 hours') WHERE id = ?1",
            rusqlite::params![record.id],
        )?;
        Ok(())
    })
    .unwrap();

    let repo = FileAssetRepository::new(&f.db);
    assert!(repo.list_orphaned(24).unwrap().is_empty());
    assert!(repo.list_orphaned(25).unwrap().is_empty());
    assert_eq!(
        repo.total_storage_bytes().unwrap(),
        0,
        "produced bytes never count against the upload quota"
    );
}

// ============================================================================
// resolve_content
// ============================================================================

#[test]
fn resolve_content_returns_the_head_and_an_older_version() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut first = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"old\n");
    first.created = at(1);
    let (v1, _) = f.store().put(first).unwrap();
    let mut second = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"new\n");
    second.created = at(1);
    f.store().put(second).unwrap();

    let head = f.store().resolve_content(&v1.id, None).unwrap();
    assert_eq!(fs::read_to_string(&head).unwrap(), "new\n");
    let head_by_number = f.store().resolve_content(&v1.id, Some(2)).unwrap();
    assert_eq!(head_by_number, head);
    let old = f.store().resolve_content(&v1.id, Some(1)).unwrap();
    assert_eq!(fs::read_to_string(&old).unwrap(), "old\n");
}

#[test]
fn resolve_content_on_a_deleted_file_marks_missing_and_returns_artifact_gone() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"body");
    new.created = at(1);
    let (record, _) = f.store().put(new).unwrap();
    assert!(!record.missing());

    fs::remove_file(&record.storage_path).unwrap();

    let err = f.store().resolve_content(&record.id, None).unwrap_err();
    let typed = err
        .downcast_ref::<ArtifactError>()
        .unwrap_or_else(|| panic!("not an ArtifactError: {err}"));
    assert_eq!(typed.code(), "ARTIFACT_GONE");
    assert!(matches!(typed, ArtifactError::Gone { id, .. } if *id == record.id));

    let after = f.store().get(&record.id, OWNER).unwrap().unwrap();
    assert!(after.missing(), "missing_since is stamped");
}

#[test]
fn resolve_content_rejects_an_unknown_id_and_an_unknown_version() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"body");
    new.created = at(1);
    let (record, _) = f.store().put(new).unwrap();

    let err = f.store().resolve_content("nope", None).unwrap_err();
    assert_eq!(
        err.downcast_ref::<ArtifactError>().unwrap().code(),
        "ARTIFACT_NOT_FOUND"
    );

    let err = f.store().resolve_content(&record.id, Some(9)).unwrap_err();
    assert_eq!(
        err.downcast_ref::<ArtifactError>().unwrap().code(),
        "ARTIFACT_VERSION_NOT_FOUND"
    );
}

// ============================================================================
// get / list
// ============================================================================

#[test]
fn get_is_owner_scoped() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"body");
    new.created = at(1);
    let (record, _) = f.store().put(new).unwrap();

    assert!(f.store().get(&record.id, OWNER).unwrap().is_some());
    assert!(f.store().get(&record.id, "owner-2").unwrap().is_none());
    assert!(f.store().get("nope", OWNER).unwrap().is_none());
}

#[test]
fn list_filters_and_totals() {
    let f = Fixture::new();
    f.task("aaaaaaaa-0000-4000-8000-000000000000", "First run");
    let scope = f.scope();

    let mut a = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Alpha", b"a");
    a.created = at(1);
    a.task_id = Some("aaaaaaaa-0000-4000-8000-000000000000");
    a.task_title = Some("First run");
    a.summary = Some("three findings");
    f.store().put(a).unwrap();

    let mut b = NewArtifact::new(OWNER, &scope, ArtifactKind::Table, "Beta", b"b");
    b.created = at(1);
    f.store().put(b).unwrap();

    let mut c = NewArtifact::new("owner-2", &scope, ArtifactKind::Markdown, "Gamma", b"c");
    c.created = at(1);
    f.store().put(c).unwrap();

    let store = f.store();

    let (rows, total) = store.list(&ArtifactQuery::new(OWNER)).unwrap();
    assert_eq!(total, 2, "other owners are invisible");
    assert_eq!(rows.len(), 2);

    let mut q = ArtifactQuery::new(OWNER);
    q.kind = Some(ArtifactKind::Table);
    let (rows, total) = store.list(&q).unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows[0].name, "01-beta.csv");

    let mut q = ArtifactQuery::new(OWNER);
    q.task_id = Some("aaaaaaaa-0000-4000-8000-000000000000".to_string());
    let (rows, total) = store.list(&q).unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows[0].name, "01-alpha.md");

    let mut q = ArtifactQuery::new(OWNER);
    q.origin = Some(ArtifactOrigin::Upload);
    assert_eq!(store.list(&q).unwrap().1, 0);

    let mut q = ArtifactQuery::new(OWNER);
    q.project_root = Some(f.project_root().to_string_lossy().to_string());
    assert_eq!(store.list(&q).unwrap().1, 2);
    let mut q = ArtifactQuery::new(OWNER);
    q.project_root = Some(String::new());
    assert_eq!(store.list(&q).unwrap().1, 0, "'' selects the home store");

    let mut q = ArtifactQuery::new(OWNER);
    q.q = Some("findings".to_string());
    let (rows, total) = store.list(&q).unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows[0].name, "01-alpha.md");

    let mut q = ArtifactQuery::new(OWNER);
    q.q = Some("beta".to_string());
    assert_eq!(store.list(&q).unwrap().1, 1);

    // A LIKE metacharacter is matched literally, not as a wildcard.
    let mut q = ArtifactQuery::new(OWNER);
    q.q = Some("%".to_string());
    assert_eq!(store.list(&q).unwrap().1, 0);

    // limit/offset page but never change the total.
    let mut q = ArtifactQuery::new(OWNER);
    q.limit = Some(1);
    let (rows, total) = store.list(&q).unwrap();
    assert_eq!((rows.len(), total), (1, 2));
    q.offset = 1;
    let (rows, total) = store.list(&q).unwrap();
    assert_eq!((rows.len(), total), (1, 2));
    q.offset = 2;
    let (rows, total) = store.list(&q).unwrap();
    assert_eq!((rows.len(), total), (0, 2));
}

#[test]
fn list_hides_missing_rows_unless_asked() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"body");
    new.created = at(1);
    let (record, _) = f.store().put(new).unwrap();
    fs::remove_file(&record.storage_path).unwrap();
    f.store().resolve_content(&record.id, None).unwrap_err();

    let store = f.store();
    assert_eq!(store.list(&ArtifactQuery::new(OWNER)).unwrap().1, 0);

    let mut q = ArtifactQuery::new(OWNER);
    q.include_missing = true;
    let (rows, total) = store.list(&q).unwrap();
    assert_eq!(total, 1);
    assert!(rows[0].missing());
}

#[test]
fn list_filters_by_pinned() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut a = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Alpha", b"a");
    a.created = at(1);
    let (a, _) = f.store().put(a).unwrap();
    let mut b = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Beta", b"b");
    b.created = at(1);
    f.store().put(b).unwrap();

    f.store().set_pinned(&a.id, true).unwrap();

    let mut q = ArtifactQuery::new(OWNER);
    q.pinned = Some(true);
    let (rows, total) = f.store().list(&q).unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows[0].id, a.id);
    assert!(rows[0].pinned);
}

// ============================================================================
// set_pinned
// ============================================================================

#[test]
fn set_pinned_round_trips_and_rejects_an_unknown_id() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"body");
    new.created = at(1);
    let (record, _) = f.store().put(new).unwrap();

    f.store().set_pinned(&record.id, true).unwrap();
    assert!(f.store().get(&record.id, OWNER).unwrap().unwrap().pinned);
    f.store().set_pinned(&record.id, false).unwrap();
    assert!(!f.store().get(&record.id, OWNER).unwrap().unwrap().pinned);

    let err = f.store().set_pinned("nope", true).unwrap_err();
    assert_eq!(
        err.downcast_ref::<ArtifactError>().unwrap().code(),
        "ARTIFACT_NOT_FOUND"
    );
}

// ============================================================================
// rebase_project
// ============================================================================

#[test]
fn rebase_project_rewrites_only_rows_under_the_old_root() {
    let f = Fixture::new();
    let other = tempdir().unwrap();
    let other_root = other.path().canonicalize().unwrap();

    let scope = f.scope();
    let mut mine = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"a");
    mine.created = at(1);
    let (mine, _) = f.store().put(mine).unwrap();

    let other_scope = StoreScope::Project(other_root.clone());
    let mut theirs = NewArtifact::new(OWNER, &other_scope, ArtifactKind::Markdown, "Notes", b"b");
    theirs.created = at(1);
    let (theirs, _) = f.store().put(theirs).unwrap();

    let home_scope = StoreScope::Home;
    let mut homely = NewArtifact::new(OWNER, &home_scope, ArtifactKind::Markdown, "Notes", b"c");
    homely.created = at(1);
    let (homely, _) = f.store().put(homely).unwrap();

    let old = f.project_root().to_string_lossy().to_string();
    let new_root = "/tmp/moved-project";
    let moved = f.store().rebase_project(&old, new_root).unwrap();
    assert_eq!(moved, 1, "only the rows under the old root move");

    let mine_after = f.store().get(&mine.id, OWNER).unwrap().unwrap();
    assert_eq!(mine_after.project_root.as_deref(), Some(new_root));
    assert_eq!(
        mine_after.storage_path,
        format!("{new_root}/.openalpaca/artifacts/loose/2026-09-01/01-notes.md")
    );
    assert_eq!(
        mine_after.rel_path.as_deref(),
        Some("loose/2026-09-01/01-notes.md"),
        "the relative address is untouched — that is the point"
    );

    let theirs_after = f.store().get(&theirs.id, OWNER).unwrap().unwrap();
    assert_eq!(
        theirs_after.project_root.as_deref(),
        Some(other_root.to_string_lossy().as_ref())
    );
    assert_eq!(theirs_after.storage_path, theirs.storage_path);

    let home_after = f.store().get(&homely.id, OWNER).unwrap().unwrap();
    assert!(home_after.project_root.is_none());
    assert_eq!(home_after.storage_path, homely.storage_path);
}

// ============================================================================
// verify
// ============================================================================

#[test]
fn verify_counts_and_marks_the_missing_rows_under_a_root() {
    let f = Fixture::new();
    let scope = f.scope();

    let mut a = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Alpha", b"a");
    a.created = at(1);
    let (a, _) = f.store().put(a).unwrap();
    let mut b = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Beta", b"b");
    b.created = at(1);
    let (b, _) = f.store().put(b).unwrap();

    let home_scope = StoreScope::Home;
    let mut c = NewArtifact::new(OWNER, &home_scope, ArtifactKind::Markdown, "Gamma", b"c");
    c.created = at(1);
    let (c, _) = f.store().put(c).unwrap();

    let root = f.project_root().to_string_lossy().to_string();
    assert_eq!(f.store().verify(Some(&root)).unwrap(), 0);

    fs::remove_file(&a.storage_path).unwrap();
    fs::remove_file(&c.storage_path).unwrap();

    assert_eq!(f.store().verify(Some(&root)).unwrap(), 1);
    assert!(f.store().get(&a.id, OWNER).unwrap().unwrap().missing());
    assert!(!f.store().get(&b.id, OWNER).unwrap().unwrap().missing());
    assert!(
        !f.store().get(&c.id, OWNER).unwrap().unwrap().missing(),
        "the home row is outside the verified root"
    );

    // Idempotent: a second pass still reports the same count.
    assert_eq!(f.store().verify(Some(&root)).unwrap(), 1);

    // None sweeps every root.
    assert_eq!(f.store().verify(None).unwrap(), 2);
    assert!(f.store().get(&c.id, OWNER).unwrap().unwrap().missing());
}

// ============================================================================
// diff
// ============================================================================

#[test]
fn diff_rejects_the_non_text_kinds() {
    let f = Fixture::new();
    let scope = f.scope();
    for kind in [ArtifactKind::Image, ArtifactKind::Binary] {
        let mut one = NewArtifact::new(OWNER, &scope, kind, "Shot", b"a");
        one.created = at(1);
        let (record, _) = f.store().put(one).unwrap();
        let mut two = NewArtifact::new(OWNER, &scope, kind, "Shot", b"b");
        two.created = at(1);
        f.store().put(two).unwrap();

        let err = f.store().diff(&record.id, 1, 2).unwrap_err();
        assert_eq!(
            err.downcast_ref::<ArtifactError>().unwrap().code(),
            "NOT_DIFFABLE",
            "kind {kind:?} must not be diffable"
        );
        f.db.with_connection(|conn| {
            conn.execute("DELETE FROM file_assets WHERE id = ?1", [&record.id])?;
            Ok(())
        })
        .unwrap();
    }
}

/// The `+`/`-` totals a human reading the patch would count: the two file
/// header lines are consumed first, then `@@` hunk headers and the
/// `\ No newline at end of file` hint are skipped. Content lines never reach
/// column 0 — every one of them carries a `+`, `-` or space prefix — so no
/// payload beginning with `@@` or `\` can be mistaken for a marker.
fn patch_totals(patch: &str) -> (i64, i64) {
    if patch.is_empty() {
        return (0, 0);
    }
    let mut lines = patch.lines();
    let from = lines.next().unwrap_or_default();
    let to = lines.next().unwrap_or_default();
    assert!(from.starts_with("--- "), "no `---` header line: {patch:?}");
    assert!(to.starts_with("+++ "), "no `+++` header line: {patch:?}");

    let mut added = 0i64;
    let mut removed = 0i64;
    for line in lines {
        if line.starts_with("@@") || line.starts_with('\\') {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'+') => added += 1,
            Some(b'-') => removed += 1,
            _ => {}
        }
    }
    (added, removed)
}

/// Writes `bodies` in order to one address, returning the artifact id.
fn versions_of(f: &Fixture, kind: ArtifactKind, bodies: &[&str]) -> String {
    let scope = f.scope();
    let mut id = String::new();
    for body in bodies {
        let mut new = NewArtifact::new(OWNER, &scope, kind, "Notes", body.as_bytes());
        new.created = at(1);
        let (record, _) = f.store().put(new).unwrap();
        id = record.id;
    }
    id
}

#[test]
fn diff_returns_a_unified_patch_between_two_versions() {
    let f = Fixture::new();
    let id = versions_of(
        &f,
        ArtifactKind::Markdown,
        &["one\ntwo\nthree\n", "one\nthree\nfour\n"],
    );

    let diff = f.store().diff(&id, 1, 2).unwrap();

    assert_eq!((diff.from, diff.to), (1, 2));
    assert_eq!(diff.format, "unified");
    assert!(
        diff.patch.starts_with("--- v1\n+++ v2\n"),
        "the header names the two versions: {:?}",
        diff.patch
    );
    assert!(diff.patch.contains("-two\n"), "{:?}", diff.patch);
    assert!(diff.patch.contains("+four\n"), "{:?}", diff.patch);
    assert!(diff.patch.contains(" one\n"), "context is kept: {:?}", diff.patch);
    assert_eq!((diff.added_lines, diff.removed_lines), (1, 1));
    assert_eq!(
        patch_totals(&diff.patch),
        (diff.added_lines, diff.removed_lines),
        "the reported counts are the patch's own totals"
    );
}

/// The patch is directional: asking for `2 → 1` is the inverse edit, not the
/// same one relabelled.
#[test]
fn diff_is_directional() {
    let f = Fixture::new();
    let id = versions_of(&f, ArtifactKind::Markdown, &["one\n", "one\ntwo\n"]);

    let forward = f.store().diff(&id, 1, 2).unwrap();
    assert_eq!((forward.added_lines, forward.removed_lines), (1, 0));
    assert!(forward.patch.contains("+two\n"));

    let backward = f.store().diff(&id, 2, 1).unwrap();
    assert_eq!((backward.from, backward.to), (2, 1));
    assert!(backward.patch.starts_with("--- v2\n+++ v1\n"));
    assert_eq!((backward.added_lines, backward.removed_lines), (0, 1));
    assert!(backward.patch.contains("-two\n"));
    assert_eq!(patch_totals(&backward.patch), (0, 1));
}

/// Identical bytes are an empty patch and a zero pair — never an error, and
/// never a patch a client would have to read as "no changes".
#[test]
fn diff_of_a_version_against_itself_is_empty() {
    let f = Fixture::new();
    let id = versions_of(&f, ArtifactKind::Markdown, &["one\ntwo\n", "one\ntwo\nthree\n"]);

    let same = f.store().diff(&id, 2, 2).unwrap();
    assert_eq!(same.patch, "");
    assert_eq!((same.added_lines, same.removed_lines), (0, 0));
}

/// A patch over more than one context radius is several hunks, and the counts
/// still sum over all of them.
#[test]
fn diff_spans_several_hunks() {
    let f = Fixture::new();
    let old: String = (1..=30).map(|n| format!("line {n}\n")).collect();
    let new: String = (1..=30)
        .map(|n| match n {
            3 => "line 3 edited\n".to_string(),
            27 => "line 27 edited\n".to_string(),
            _ => format!("line {n}\n"),
        })
        .collect();
    let id = versions_of(&f, ArtifactKind::Markdown, &[&old, &new]);

    let diff = f.store().diff(&id, 1, 2).unwrap();
    assert_eq!(
        diff.patch.matches("@@").count(),
        4,
        "two hunks, two `@@` markers each: {:?}",
        diff.patch
    );
    assert_eq!((diff.added_lines, diff.removed_lines), (2, 2));
    assert_eq!(patch_totals(&diff.patch), (2, 2));
}

#[test]
fn diff_still_validates_the_artifact_and_its_versions() {
    let f = Fixture::new();
    let id = versions_of(&f, ArtifactKind::Markdown, &["a\n", "a\nb\n"]);

    let err = f.store().diff(&id, 1, 9).unwrap_err();
    assert_eq!(
        err.downcast_ref::<ArtifactError>().unwrap().code(),
        "ARTIFACT_VERSION_NOT_FOUND"
    );
    let err = f.store().diff("nope", 1, 2).unwrap_err();
    assert_eq!(
        err.downcast_ref::<ArtifactError>().unwrap().code(),
        "ARTIFACT_NOT_FOUND"
    );
}

/// A version whose bytes were deleted is `ARTIFACT_GONE`, exactly as reading it
/// through `resolve_content` is — the diff reads the same files.
#[test]
fn diff_reports_a_deleted_version_as_gone() {
    let f = Fixture::new();
    let id = versions_of(&f, ArtifactKind::Markdown, &["a\n", "a\nb\n"]);
    fs::remove_file(
        f.artifacts_root()
            .join("loose/2026-09-01/.versions/01-notes/v1.md"),
    )
    .unwrap();

    let err = f.store().diff(&id, 1, 2).unwrap_err();
    assert_eq!(
        err.downcast_ref::<ArtifactError>().unwrap().code(),
        "ARTIFACT_GONE"
    );
}

// ============================================================================
// line counting (the write-time added/removed pair)
// ============================================================================

#[test]
fn line_counts_are_recorded_at_write_time() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut one = NewArtifact::new(
        OWNER,
        &scope,
        ArtifactKind::Markdown,
        "Notes",
        b"one\ntwo\nthree\n",
    );
    one.created = at(1);
    let (record, _) = f.store().put(one).unwrap();

    let mut two = NewArtifact::new(
        OWNER,
        &scope,
        ArtifactKind::Markdown,
        "Notes",
        b"one\nthree\nfour\nfive\n",
    );
    two.created = at(1);
    f.store().put(two).unwrap();

    let rows = f.store().versions(&record.id).unwrap();
    assert_eq!(rows[0].added_lines, Some(2), "four, five");
    assert_eq!(rows[0].removed_lines, Some(1), "two");
    assert_eq!(rows[0].author_agent_id, None);
    assert_eq!(rows[0].size_bytes, 20);
}

/// The stored pair and the patch are one computation: whatever a reader counts
/// in the unified diff is what the version row already said.
///
/// A *moved* line proves it. Under a real line diff it is one delete and one
/// insert — which is exactly what the patch shows — where the multiset tally
/// this replaced called it neither.
#[test]
fn the_stored_line_counts_are_the_patch_totals() {
    let f = Fixture::new();
    let id = versions_of(&f, ArtifactKind::Markdown, &["a\nb\nc\n", "b\nc\na\n"]);

    let rows = f.store().versions(&id).unwrap();
    let diff = f.store().diff(&id, 1, 2).unwrap();

    assert_eq!(
        (diff.added_lines, diff.removed_lines),
        (1, 1),
        "the moved line is one add and one delete"
    );
    assert_eq!(rows[0].added_lines, Some(diff.added_lines));
    assert_eq!(rows[0].removed_lines, Some(diff.removed_lines));
    assert_eq!(
        patch_totals(&diff.patch),
        (diff.added_lines, diff.removed_lines)
    );
}

/// The T23 re-review's Minor 8. An interrupted put parks *uncommitted* bytes at
/// the head while the committed v(N−1) waits under `.versions/`; the recovering
/// put discards that head. Counting the new version against it would compare it
/// to bytes no row ever described — the pair belongs to the two versions the
/// rows claim, which is where `write_bytes` says v(N−1) actually is.
#[test]
fn the_line_counts_are_taken_against_the_committed_previous_version() {
    let f = Fixture::new();
    let scope = f.scope();

    let mut first = NewArtifact::new(
        OWNER,
        &scope,
        ArtifactKind::Markdown,
        "Notes",
        b"alpha\nbeta\n",
    );
    first.created = at(1);
    let (v1, _) = f.store().put(first).unwrap();

    {
        let _crash = CrashGuard::after(WriteStep::HeadRenamed);
        let mut orphan =
            NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"gamma\n");
        orphan.created = at(1);
        f.store().put(orphan).unwrap_err();
    }
    let dir = f.artifacts_root().join("loose/2026-09-01");
    assert_eq!(
        fs::read_to_string(dir.join("01-notes.md")).unwrap(),
        "gamma\n",
        "the orphaned head is what a naive count would read"
    );

    let mut retry = NewArtifact::new(
        OWNER,
        &scope,
        ArtifactKind::Markdown,
        "Notes",
        b"alpha\nbeta\ndelta\n",
    );
    retry.created = at(1);
    let (v2, _) = f.store().put(retry).unwrap();
    assert_eq!(v2.version, 2);

    let rows = f.store().versions(&v1.id).unwrap();
    assert_eq!(rows[0].version, 2);
    assert_eq!(
        (rows[0].added_lines, rows[0].removed_lines),
        (Some(1), Some(0)),
        "counted against v1's committed bytes (+delta), not the orphan (+3 −1)"
    );

    // …and the patch over the pair the rows describe says the same thing.
    let diff = f.store().diff(&v1.id, 1, 2).unwrap();
    assert_eq!((diff.added_lines, diff.removed_lines), (1, 0));
    assert_eq!(patch_totals(&diff.patch), (1, 0));
}

#[test]
fn line_counts_are_absent_for_a_binary_kind() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut one = NewArtifact::new(OWNER, &scope, ArtifactKind::Binary, "Blob", &[0u8, 1, 2]);
    one.created = at(1);
    let (record, _) = f.store().put(one).unwrap();
    let mut two = NewArtifact::new(OWNER, &scope, ArtifactKind::Binary, "Blob", &[0u8, 1, 2, 3]);
    two.created = at(1);
    f.store().put(two).unwrap();

    let rows = f.store().versions(&record.id).unwrap();
    assert_eq!(rows[0].version, 2);
    assert_eq!(rows[0].added_lines, None);
    assert_eq!(rows[0].removed_lines, None);
}
