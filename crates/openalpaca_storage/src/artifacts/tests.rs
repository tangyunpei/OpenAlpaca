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
        // A project scope is a registered project's (design §3, R12) —
        // registered under this fixture's home, never the developer's.
        crate::store::projects::register(&project.path().canonicalize().unwrap()).unwrap();
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

    /// The canonical project root — canonical because the fixture registers
    /// its project there, so `project_root` records that string (§4.8; R-T19:
    /// the root `lookup` finds), and macOS's `/var` is a symlink.
    fn project_root(&self) -> PathBuf {
        self.project.path().canonicalize().unwrap()
    }

    fn scope(&self) -> StoreScope {
        StoreScope::Project(self.project_root())
    }

    fn home_root(&self) -> PathBuf {
        self._home.path().canonicalize().unwrap()
    }

    /// The project's `artifacts/` — the root `rel_path` is relative to.
    fn artifacts_root(&self) -> PathBuf {
        crate::store::store_root(&self.scope())
            .unwrap()
            .join("artifacts")
    }

    /// A result file the user has: real bytes in the project folder, and its
    /// pointer row written by R35's method — never a hand `INSERT` (R39).
    /// Returns the row, the file, and its modification time as written.
    fn result_file(
        &self,
        rel: &str,
        bytes: &[u8],
    ) -> (ArtifactRecord, PathBuf, std::time::SystemTime) {
        let path = self.project_root().join(rel);
        fs::create_dir_all(path.parent().expect("a file has a parent")).unwrap();
        fs::write(&path, bytes).unwrap();
        let mtime = fs::metadata(&path).unwrap().modified().unwrap();
        let root = self.project_root().to_string_lossy().into_owned();
        let (record, inserted) = self
            .store()
            .record_project_file(NewProjectFile {
                owner_id: OWNER,
                project_root: &root,
                rel_path: rel,
                content: bytes,
                task_id: None,
                agent_id: None,
            })
            .unwrap();
        assert!(inserted, "{rel} is a new result file");
        (record, path, mtime)
    }

    /// A `file_assets` row written by neither `ArtifactStore` nor this owner's
    /// produced history — an **upload**, the table's other writer — addressing
    /// `rel` in this project's store. `dir_rows` never sees it (it filters on
    /// `origin = 'produced'`), so it is exactly the foreign claim R33 must not
    /// reclaim.
    fn foreign_row(&self, id: &str, rel: &str, storage_path: &Path) {
        let root = self.project_root().to_string_lossy().to_string();
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO file_assets
                        (id, owner_id, sha256, filename, mime_type, size_bytes, storage_path,
                         status, origin, project_root, rel_path)
                     VALUES (?1, ?2, 'sha', 'notes.md', 'text/markdown', 0, ?3, 'ready',
                             'upload', ?4, ?5)",
                    rusqlite::params![
                        id,
                        OWNER,
                        storage_path.to_string_lossy(),
                        root,
                        rel
                    ],
                )?;
                Ok(())
            })
            .unwrap();
    }

    /// An `artifact_versions` row pointing at `rel` — a version's bytes, which
    /// are as referenced as a head's.
    fn version_row(&self, artifact_id: &str, version: u32, rel: &str) {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO artifact_versions
                        (artifact_id, version, rel_path, sha256, size_bytes)
                     VALUES (?1, ?2, ?3, 'sha', 0)",
                    rusqlite::params![artifact_id, version, rel],
                )?;
                Ok(())
            })
            .unwrap();
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

    /// A `task` bound to a workspace, in a given state — the other three
    /// members of §4.8's one transaction start here.
    fn task_in(&self, id: &str, workspace: &str, status: &str) {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO task (id, title, created_by, source_lane, workspace_id, status)
                     VALUES (?1, ?1, 'test', 'test', ?2, ?3)",
                    rusqlite::params![id, workspace, status],
                )?;
                Ok(())
            })
            .unwrap();
    }

    fn session(&self, id: &str, workspace: &str) {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO session (id, lane_key, source, workspace_id)
                     VALUES (?1, ?1, 'gui', ?2)",
                    rusqlite::params![id, workspace],
                )?;
                Ok(())
            })
            .unwrap();
    }

    /// A `file_assets` row addressed under an arbitrary `root` at `rel` —
    /// the way another project's artifact looks from here.
    fn asset_under(&self, id: &str, root: &str, rel: &str) {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO file_assets
                        (id, owner_id, sha256, filename, mime_type, size_bytes, storage_path,
                         status, origin, project_root, rel_path)
                     VALUES (?1, ?2, 'sha', 'notes.md', 'text/markdown', 0, ?3, 'ready',
                             'produced', ?4, ?5)",
                    rusqlite::params![
                        id,
                        OWNER,
                        format!("{root}/.openalpaca/artifacts/{rel}"),
                        root,
                        rel
                    ],
                )?;
                Ok(())
            })
            .unwrap();
    }

    /// `(sessions, tasks)` still naming `root` — the two members that are not
    /// artifact rows.
    fn members_at(&self, root: &str) -> (i64, i64) {
        self.db
            .with_connection(|conn| {
                let one = |sql: &str| -> Result<i64> {
                    Ok(conn.query_row(sql, rusqlite::params![root], |r| r.get(0))?)
                };
                Ok((
                    one("SELECT COUNT(*) FROM session WHERE workspace_id = ?1")?,
                    one("SELECT COUNT(*) FROM task WHERE workspace_id = ?1")?,
                ))
            })
            .unwrap()
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
// put — the head reservation and the address rule (R24)
// ============================================================================

/// R33, as revised. A row-less file *with bytes* at the head address is either
/// the head of a create that died between the final rename and `tx.commit()`,
/// or a file that outlived its rows for a reason that makes it the user's (a
/// factory reset). Nothing on disk tells the two apart, so the store never
/// takes the name from it: the file keeps its bytes and the new artifact
/// takes the next `NN-`.
#[test]
fn a_row_less_head_with_bytes_keeps_its_name_and_the_create_steps_past_it() {
    let f = Fixture::new();
    let scope = f.scope();

    let day_dir = f.artifacts_root().join("loose/2026-09-01");
    fs::create_dir_all(&day_dir).unwrap();
    let orphan = day_dir.join("01-notes.md");
    fs::write(&orphan, b"bytes with no row").unwrap();

    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"hello");
    new.created = at(1);
    let (record, created) = f.store().put(new).unwrap();

    assert!(created);
    assert_eq!(record.name, "02-notes.md");
    assert_eq!(fs::read(&orphan).unwrap(), b"bytes with no row");
    assert_eq!(fs::read_to_string(&record.storage_path).unwrap(), "hello");
    assert_no_tmp_leftovers(&day_dir);

    // The artifact is the one at 02-: writing the title again supersedes it,
    // and the file at 01- is still nobody's business.
    let mut again = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"v2");
    again.created = at(1);
    let (again, created) = f.store().put(again).unwrap();
    assert!(!created);
    assert_eq!(again.id, record.id);
    assert_eq!(again.name, "02-notes.md");
    assert_eq!(fs::read(&orphan).unwrap(), b"bytes with no row");
}

/// The finding this ruling answers: `config reset --factory` deletes the
/// database and promises the artifact files "stay exactly where they are". The
/// fresh database has no rows, so the same title on the same day computes the
/// same `01-` name the pre-reset artifact holds — and must not take it.
#[test]
fn an_artifact_written_after_a_factory_reset_never_replaces_a_pre_reset_file() {
    let f = Fixture::new();
    let scope = f.scope();

    let mut before = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Summary", b"old\n");
    before.created = at(1);
    let (before, _) = f.store().put(before).unwrap();
    assert_eq!(before.name, "01-summary.md");
    let day_dir = f.artifacts_root().join("loose/2026-09-01");
    let snapshot = |p: &Path| fs::read(p).unwrap();
    let head_before = snapshot(&day_dir.join("01-summary.md"));
    assert_eq!(head_before, b"old\n");

    // The reset: a new, empty database over the same store.
    let fresh_dir = tempdir().unwrap();
    let fresh = Database::open(&fresh_dir.path().join("fresh.db")).unwrap();
    let store = ArtifactStore::new(&fresh);

    let mut after = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Summary", b"new\n");
    after.created = at(1);
    let (after, created) = store.put(after).unwrap();
    assert!(created);
    assert_eq!(after.name, "02-summary.md");
    assert_eq!(snapshot(&day_dir.join("01-summary.md")), head_before);
    assert_eq!(fs::read_to_string(&after.storage_path).unwrap(), "new\n");

    // And superseding the new one rotates into its own history, not the old.
    let mut after_v2 = NewArtifact::new(
        OWNER,
        &scope,
        ArtifactKind::Markdown,
        "Summary",
        b"new v2\n",
    );
    after_v2.created = at(1);
    store.put(after_v2).unwrap();
    assert_eq!(
        fs::read_to_string(day_dir.join(".versions/02-summary/v1.md")).unwrap(),
        "new\n"
    );
    assert!(!day_dir.join(".versions/01-summary").exists());
    assert_eq!(snapshot(&day_dir.join("01-summary.md")), head_before);
}

/// The cheaper half of the crash: a create that died right after `create_new`
/// leaves an empty reservation and no row. Removing an empty file destroys no
/// bytes, so this is the one case the store still reclaims — the retry keeps
/// the name it would have had.
#[test]
fn an_empty_reservation_left_by_a_crashed_create_is_reclaimed() {
    let f = Fixture::new();
    let scope = f.scope();

    let day_dir = f.artifacts_root().join("loose/2026-09-01");
    fs::create_dir_all(&day_dir).unwrap();
    fs::write(day_dir.join("01-notes.md"), b"").unwrap();

    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"hello");
    new.created = at(1);
    let (record, created) = f.store().put(new).unwrap();

    assert!(created);
    assert_eq!(record.name, "01-notes.md");
    assert_eq!(fs::read_to_string(&record.storage_path).unwrap(), "hello");
    assert!(!day_dir.join("02-notes.md").exists());
}

/// A file a row *does* reference is never taken either. `dir_rows` only sees
/// produced heads, so an upload — the other writer of `file_assets` —
/// addressing the same `project_root` + `rel_path` is stepped past like any
/// other held name.
#[test]
fn a_head_another_row_describes_is_never_overwritten() {
    let f = Fixture::new();
    let scope = f.scope();

    let day_dir = f.artifacts_root().join("loose/2026-09-01");
    fs::create_dir_all(&day_dir).unwrap();
    let taken = day_dir.join("01-notes.md");
    fs::write(&taken, b"bytes another row describes").unwrap();
    f.foreign_row("upload-1", "loose/2026-09-01/01-notes.md", &taken);

    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"hello");
    new.created = at(1);
    let (record, created) = f.store().put(new).unwrap();

    assert!(created);
    assert_eq!(record.name, "02-notes.md");
    assert_eq!(
        fs::read(&taken).unwrap(),
        b"bytes another row describes",
        "the file must be left exactly as it was"
    );
    assert_no_tmp_leftovers(&day_dir);
}

/// A referenced *empty* file is not an abandoned reservation: a row owns it.
/// The zero-length reclaim applies only to what no row describes.
#[test]
fn an_empty_file_a_row_describes_is_never_reclaimed() {
    let f = Fixture::new();
    let scope = f.scope();

    let day_dir = f.artifacts_root().join("loose/2026-09-01");
    fs::create_dir_all(&day_dir).unwrap();
    let taken = day_dir.join("01-notes.md");
    fs::write(&taken, b"").unwrap();
    f.foreign_row("upload-1", "loose/2026-09-01/01-notes.md", &taken);

    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"hello");
    new.created = at(1);
    let (record, _) = f.store().put(new).unwrap();

    assert_eq!(record.name, "02-notes.md");
    assert_eq!(fs::read(&taken).unwrap(), b"");
}

/// A version row references bytes just as a head row does — the `.versions/`
/// side of the same rule. A file some artifact's history still points at is
/// never taken.
#[test]
fn a_head_a_version_row_still_points_at_is_never_overwritten() {
    let f = Fixture::new();
    let scope = f.scope();

    let mut first = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Alpha", b"a\n");
    first.created = at(1);
    let (alpha, _) = f.store().put(first).unwrap();

    // Beta's address, claimed by a version row of Alpha's history.
    let day_dir = f.artifacts_root().join("loose/2026-09-01");
    let taken = day_dir.join("02-beta.md");
    fs::write(&taken, b"a version's bytes").unwrap();
    f.version_row(&alpha.id, 9, "loose/2026-09-01/02-beta.md");

    let mut second = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Beta", b"b\n");
    second.created = at(1);
    let (beta, _) = f.store().put(second).unwrap();

    assert_eq!(beta.name, "03-beta.md");
    assert_eq!(fs::read(&taken).unwrap(), b"a version's bytes");
}

/// A `.versions/<stem>/` with no head beside it — the user deleted a pre-reset
/// head and kept its history. Taking that name would make the new artifact's
/// first supersede rotate into, and `fs::rename` replace, versions nobody in
/// this database wrote.
#[test]
fn a_name_whose_version_directory_survives_is_stepped_past() {
    let f = Fixture::new();
    let scope = f.scope();

    let day_dir = f.artifacts_root().join("loose/2026-09-01");
    let history = day_dir.join(".versions/01-notes");
    fs::create_dir_all(&history).unwrap();
    fs::write(history.join("v1.md"), b"a pre-reset version").unwrap();

    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"hello");
    new.created = at(1);
    let (record, _) = f.store().put(new).unwrap();

    assert_eq!(record.name, "02-notes.md");
    assert!(!day_dir.join("01-notes.md").exists());
    assert_eq!(
        fs::read(history.join("v1.md")).unwrap(),
        b"a pre-reset version"
    );
}

/// A directory at the name used to be the one state R33 could not recover on
/// its own. It is stepped past like anything else that holds a name.
#[test]
fn a_directory_at_the_head_name_is_stepped_past() {
    let f = Fixture::new();
    let scope = f.scope();

    let day_dir = f.artifacts_root().join("loose/2026-09-01");
    fs::create_dir_all(day_dir.join("01-notes.md")).unwrap();

    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"hello");
    new.created = at(1);
    let (record, _) = f.store().put(new).unwrap();

    assert_eq!(record.name, "02-notes.md");
    assert!(day_dir.join("01-notes.md").is_dir());
}

/// Stepping past is bounded. When every name it may try is held, the create
/// refuses with `ARTIFACT_NAME_TAKEN` and every file is left as it was.
#[test]
fn a_create_that_finds_every_name_held_refuses_and_touches_nothing() {
    let f = Fixture::new();
    let scope = f.scope();

    let day_dir = f.artifacts_root().join("loose/2026-09-01");
    fs::create_dir_all(&day_dir).unwrap();
    for seq in 1..=MAX_HEAD_PROBES {
        fs::write(
            day_dir.join(artifact_file_name(seq, "Notes", "md")),
            format!("stray {seq}"),
        )
        .unwrap();
    }

    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"hello");
    new.created = at(1);
    let err = f.store().put(new).unwrap_err();

    assert_eq!(
        err.downcast_ref::<ArtifactError>()
            .unwrap_or_else(|| panic!("not an ArtifactError: {err}"))
            .code(),
        "ARTIFACT_NAME_TAKEN"
    );
    for seq in 1..=MAX_HEAD_PROBES {
        assert_eq!(
            fs::read_to_string(day_dir.join(artifact_file_name(seq, "Notes", "md"))).unwrap(),
            format!("stray {seq}")
        );
    }
    let next = day_dir.join(artifact_file_name(MAX_HEAD_PROBES + 1, "Notes", "md"));
    assert!(!next.exists(), "the probe stops at its bound");
    assert_eq!(f.store().list(&ArtifactQuery::new(OWNER)).unwrap().1, 0);
    assert_no_tmp_leftovers(&day_dir);
}

/// R33 is about the crash the process does not survive; inside the process the
/// `Drop` guard is still what answers. A create that dies *after* the final
/// rename — the head already holding the new bytes — leaves neither the bytes
/// nor a row, because no row describes them either way.
#[test]
fn a_crash_after_the_head_rename_on_a_create_leaves_nothing() {
    let f = Fixture::new();
    let scope = f.scope();

    {
        let _crash = CrashGuard::after(WriteStep::HeadRenamed);
        let mut doomed = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"a\n");
        doomed.created = at(1);
        f.store().put(doomed).unwrap_err();
    }

    let day_dir = f.artifacts_root().join("loose/2026-09-01");
    assert!(
        !day_dir.join("01-notes.md").exists(),
        "the reservation took the uncommitted bytes with it"
    );
    assert_eq!(f.store().list(&ArtifactQuery::new(OWNER)).unwrap().1, 0);
    assert_no_tmp_leftovers(&day_dir);

    let mut retry = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"b\n");
    retry.created = at(1);
    let (record, created) = f.store().put(retry).unwrap();
    assert!(created);
    assert_eq!(fs::read_to_string(&record.storage_path).unwrap(), "b\n");
}

/// The reservation must not outlive the call that made it. A create that fails
/// after claiming the name leaves nothing behind — otherwise the very next
/// attempt at the same address would meet its own orphan and refuse it.
#[test]
fn a_create_that_fails_releases_the_name_it_reserved() {
    let f = Fixture::new();
    let scope = f.scope();

    {
        let _crash = CrashGuard::after(WriteStep::TmpWritten);
        let mut doomed = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"a\n");
        doomed.created = at(1);
        f.store().put(doomed).unwrap_err();
    }

    let dir = f.artifacts_root().join("loose/2026-09-01");
    assert!(
        !dir.join("01-notes.md").exists(),
        "the reservation was removed with the failed write"
    );
    assert_no_tmp_leftovers(&dir);

    let mut retry = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"a\n");
    retry.created = at(1);
    let (record, created) = f.store().put(retry).unwrap();
    assert!(created);
    assert_eq!(record.name, "01-notes.md");
    assert_eq!(fs::read_to_string(&record.storage_path).unwrap(), "a\n");
}

/// R15: a project artifact's bytes land in the project's folder under the
/// home store's `projects/`, and its row records the turn's project root —
/// never the directory the bytes reached. Nothing lands in the project.
#[test]
fn a_project_artifact_lands_under_projects_and_records_the_turns_root() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"a\n");
    new.created = at(1);
    let (record, _) = f.store().put(new).unwrap();

    let folder = f
        .home_root()
        .join(crate::store::projects::PROJECTS_DIR)
        .join(crate::store::projects::folder_name(&f.project_root()));
    assert_eq!(
        PathBuf::from(&record.storage_path),
        folder
            .join("artifacts")
            .join("loose")
            .join("2026-09-01")
            .join("01-notes.md")
    );
    assert_eq!(record.project_root.as_deref(), f.project_root().to_str());
    assert!(!f.project_root().join(".openalpaca").exists(), "ST-1");
}

/// The lead's ruling R-T19 (ST-23 with R15): a by-product and an upload
/// filed in a turn at the old root of a project whose folder moved with a
/// link left there record that root — the project `lookup` finds, whose
/// folder under `projects/` holds their bytes — and never the folder the
/// link reaches, though a project is registered there. That project's own
/// rows keep its canonical root, through another link to it too.
#[cfg(unix)]
#[test]
fn a_write_at_a_missing_projects_old_root_records_that_root_and_never_the_folder_it_reaches() {
    let f = Fixture::new();
    // The root as it is recorded, and its folder, taken before the move:
    // `project_root()` canonicalizes, which after the move would answer the
    // moved folder.
    let root = f.project_root();
    let own = crate::store::projects::lookup(&root).unwrap().unwrap();
    let aside = tempdir().unwrap();
    let base = aside.path().canonicalize().unwrap();
    let moved = base.join("app-moved");
    fs::rename(&root, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &root).unwrap();
    // A turn in the moved folder registers it as a project of its own.
    let theirs = crate::store::projects::register(&moved).unwrap();
    assert_ne!(theirs.id, own.id);
    let address = |id: &str| -> Option<String> {
        f.db.with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT project_root FROM file_assets WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )?)
        })
        .unwrap()
    };
    let recorded = root.to_string_lossy().into_owned();
    let reached = moved.to_string_lossy().into_owned();

    let old = StoreScope::Project(root.clone());
    let mut new = NewArtifact::new(OWNER, &old, ArtifactKind::Markdown, "Notes", b"a\n");
    new.created = at(1);
    let (artifact, _) = f.store().put(new).unwrap();
    let upload = crate::uploads::UploadStore::new(&f.db)
        .put(crate::uploads::NewUpload {
            owner_id: OWNER,
            filename: "notes.txt",
            mime_type: "text/plain",
            data: b"up\n",
            scope: &old,
            created: at(1),
        })
        .unwrap();

    assert_eq!(artifact.project_root.as_deref(), Some(recorded.as_str()));
    assert_eq!(
        address(&upload.asset.id).as_deref(),
        Some(recorded.as_str())
    );
    for path in [&artifact.storage_path, &upload.asset.storage_path] {
        assert!(
            Path::new(path).starts_with(&own.dir),
            "its bytes are in its own folder: {path}"
        );
    }

    // The project registered where the link leads keeps its canonical root,
    // and a write through another link to it records that root too.
    let alias = base.join("alias");
    std::os::unix::fs::symlink(&moved, &alias).unwrap();
    for scope in [
        StoreScope::Project(moved.clone()),
        StoreScope::Project(alias),
    ] {
        let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Plan", b"b\n");
        new.created = at(1);
        let (row, _) = f.store().put(new).unwrap();
        assert_eq!(row.project_root.as_deref(), Some(reached.as_str()));
        assert!(
            Path::new(&row.storage_path).starts_with(&theirs.dir),
            "{}",
            row.storage_path
        );
    }
}

/// R15/R12: a row whose own columns do not name its store falls back to
/// its scope's store, and that fallback creates nothing — for a project no
/// folder under `projects/` records, nothing at all.
#[test]
fn resolving_a_row_whose_columns_do_not_name_its_store_creates_nothing() {
    let f = Fixture::new();
    let stray = tempdir().unwrap();
    let stray_root = stray.path().canonicalize().unwrap();
    f.db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO file_assets (id, owner_id, sha256, filename, mime_type, size_bytes,
                 storage_path, status, origin, project_root, rel_path)
             VALUES ('stray-row', ?1, 'sha', 'notes.md', 'text/markdown', 1, '/nowhere/x.md',
                     'ready', 'produced', ?2, 'loose/2026-09-01/01-notes.md')",
            rusqlite::params![OWNER, stray_root.to_string_lossy()],
        )?;
        Ok(())
    })
    .unwrap();

    assert!(f.store().resolve_content("stray-row", None).is_err());
    assert!(
        !stray_root.join(".openalpaca").exists(),
        "nothing in the folder"
    );
    assert!(
        !f.home_root()
            .join(crate::store::projects::PROJECTS_DIR)
            .join(crate::store::projects::folder_name(&stray_root))
            .exists(),
        "and nothing under projects/"
    );
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

/// R39: the user's file is exactly as they had it — its bytes, and its
/// modification time, which any write (even of the same bytes) would move.
fn assert_untouched(path: &Path, bytes: &[u8], mtime: std::time::SystemTime) {
    assert_eq!(fs::read(path).unwrap(), bytes, "the user's bytes");
    assert_eq!(
        fs::metadata(path).unwrap().modified().unwrap(),
        mtime,
        "the user's modification time"
    );
}

fn file_asset_count(f: &Fixture) -> i64 {
    f.db.with_connection(|conn| {
        Ok(conn.query_row("SELECT COUNT(*) FROM file_assets", [], |row| row.get(0))?)
    })
    .unwrap()
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
fn a_put_after_an_interrupted_head_rename_keeps_v1_and_records_the_orphan() {
    // The crash at `HeadRenamed` leaves the *uncommitted* new bytes at the head
    // and the true, committed v1 at `.versions/01-notes/v1.md` (the row rolled
    // back to v1 with sha("old\n")). The head therefore holds bytes v1's row
    // does not describe, which is §4.8's hand edit however it got there: the
    // recovering put records it as the version nobody wrote *before* writing
    // its own, exactly as a read of the same state does. v1's only copy is
    // never rotated over.
    let (f, dir, id) = crash_during_second_put(WriteStep::HeadRenamed);
    let v1_sha = f.store().versions(&id).unwrap()[0].sha256.clone();
    assert_eq!(v1_sha, sha256_hex(b"old\n"), "the row still describes v1");

    let scope = f.scope();
    let mut retry = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"newer\n");
    retry.created = at(1);
    let (record, created) = f.store().put(retry).unwrap();
    assert!(!created);
    assert_eq!(record.id, id);
    assert_eq!(record.version, 3, "the orphan is v2, this write is v3");

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
    assert_eq!(
        fs::read_to_string(dir.join(".versions/01-notes/v2.md")).unwrap(),
        "new\n",
        "the orphaned head is kept under the version that now describes it"
    );
    let mut on_disk: Vec<String> = fs::read_dir(dir.join(".versions/01-notes"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    on_disk.sort();
    assert_eq!(on_disk, vec!["v1.md".to_string(), "v2.md".to_string()]);

    let rows = f.store().versions(&id).unwrap();
    assert_eq!(rows.len(), 3);
    let v1 = rows.iter().find(|r| r.version == 1).unwrap();
    assert_eq!(v1.rel_path, "loose/2026-09-01/.versions/01-notes/v1.md");
    assert_eq!(
        v1.sha256, v1_sha,
        "v1's row sha still describes v1's own bytes"
    );
    let v2 = rows.iter().find(|r| r.version == 2).unwrap();
    assert_eq!(v2.author_agent_id, None, "nobody committed to those bytes");
    assert_eq!(v2.sha256, sha256_hex(b"new\n"));
    let path = f.store().resolve_content(&id, Some(1)).unwrap();
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "old\n",
        "resolve_content(v1) serves v1's bytes, not another version's"
    );
    assert_no_tmp_leftovers(&dir);
}

#[test]
fn an_interrupted_head_rename_leaves_v1_readable_and_records_the_orphaned_head() {
    // The same interrupted state, read rather than written. The head holds
    // bytes no completed write left there, so its hash does not match v1's row
    // — §4.8's hand-edit rotation, which repairs v1's row (it was claiming the
    // head path) and preserves the orphaned bytes as v2 rather than letting the
    // next put discard them. Nothing on disk is destroyed either way.
    let (f, dir, id) = crash_during_second_put(WriteStep::HeadRenamed);

    let root = f.project_root().to_string_lossy().to_string();
    let report = f.store().verify(Some(&root)).unwrap();
    assert_eq!(
        report.missing, 0,
        "the head is present, so nothing is marked missing"
    );
    assert_eq!(report.user_edits.len(), 1, "the orphaned head is recorded");
    assert_eq!(report.user_edits[0].version, 2);

    let record = f.store().get(&id, OWNER).unwrap().unwrap();
    assert!(!record.missing());
    assert_eq!(record.version, 2);
    assert_eq!(record.sha256, sha256_hex(b"new\n"));

    let head = f.store().resolve_content(&id, None).unwrap();
    assert_eq!(head, dir.join("01-notes.md"));
    assert_eq!(fs::read_to_string(&head).unwrap(), "new\n");

    // v1's row now points where v1's bytes actually are, so reading it serves
    // them instead of another version's.
    let v1 = f.store().resolve_content(&id, Some(1)).unwrap();
    assert_eq!(v1, dir.join(".versions/01-notes/v1.md"));
    assert_eq!(fs::read_to_string(&v1).unwrap(), "old\n");

    let rows = f.store().versions(&id).unwrap();
    let v2 = rows.iter().find(|r| r.version == 2).unwrap();
    assert_eq!(v2.author_agent_id, None);
    assert_eq!(v2.note.as_deref(), Some(USER_EDIT_NOTE));
    // The previous version's bytes survived, so the counts are real.
    assert_eq!((v2.added_lines, v2.removed_lines), (Some(1), Some(1)));
    assert_eq!(
        rows.iter().find(|r| r.version == 1).unwrap().sha256,
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

/// C1: the head is removed by hand, then the same address is put again. v(N-1)'s
/// row must stop naming the head path, or the prune that closes the put deletes
/// the bytes the put just committed.
#[test]
fn a_put_over_a_removed_head_keeps_the_new_head_through_the_prune() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut first = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"v1\n");
    first.created = at(1);
    first.max_versions = Some(1);
    let (v1, _) = f.store().put(first).unwrap();

    fs::remove_file(&v1.storage_path).unwrap();

    let mut second = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"v2\n");
    second.created = at(1);
    second.max_versions = Some(1);
    let (v2, _) = f.store().put(second).unwrap();
    assert_eq!(v2.id, v1.id, "the same artifact was superseded");

    assert_eq!(
        fs::read_to_string(&v2.storage_path).unwrap(),
        "v2\n",
        "the prune unlinked v1's empty version slot, not the live head"
    );
    assert_eq!(
        fs::read_to_string(f.store().resolve_content(&v2.id, None).unwrap()).unwrap(),
        "v2\n"
    );
}

#[test]
fn a_put_over_a_removed_head_repoints_the_previous_version_row() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut first = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"v1\n");
    first.created = at(1);
    first.max_versions = Some(3);
    let (v1, _) = f.store().put(first).unwrap();

    fs::remove_file(&v1.storage_path).unwrap();

    let mut second = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"v2\n");
    second.created = at(1);
    second.max_versions = Some(3);
    let (v2, _) = f.store().put(second).unwrap();

    let versions = f.store().versions(&v2.id).unwrap();
    let one = versions
        .iter()
        .find(|v| v.version == 1)
        .expect("v1's row survives");
    assert_eq!(
        one.rel_path, "loose/2026-09-01/.versions/01-notes/v1.md",
        "v1's row names its own (empty) version slot, never the head"
    );
    let two = versions.iter().find(|v| v.version == 2).unwrap();
    assert_eq!(two.rel_path, "loose/2026-09-01/01-notes.md");

    // v1's bytes really are gone — the row says so instead of serving v2's.
    let err = f.store().resolve_content(&v2.id, Some(1)).unwrap_err();
    assert_eq!(
        err.downcast_ref::<ArtifactError>().unwrap().code(),
        "ARTIFACT_GONE"
    );

    // Three more puts cross keep = 3, the path the default config takes.
    for n in 3..=5u32 {
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
    let head = f.store().get(&v2.id, OWNER).unwrap().unwrap();
    assert_eq!(fs::read_to_string(&head.storage_path).unwrap(), "v5\n");
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
    for literal in ["_", "\\"] {
        let mut q = ArtifactQuery::new(OWNER);
        q.q = Some(literal.to_string());
        assert_eq!(store.list(&q).unwrap().1, 0, "{literal:?} is literal");
    }

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
// repoint_rows
// ============================================================================

/// §6 step 2: one transaction moves the five path columns from the old root
/// to the new one and rewrites both `storage_path` prefixes — the project's
/// folder under `projects/`, and (a pointer row's) the project's own
/// path — each only on rows whose `project_root` is the old root, and only
/// where the prefix is followed by a separator.
#[test]
fn repoint_rows_rewrites_the_five_columns_and_both_storage_path_prefixes() {
    let f = Fixture::new();
    let (old, new) = ("/w/app", "/w/app-moved");
    let (old_dir, new_dir) = ("/h/projects/-w-app", "/h/projects/-w-app-moved");
    f.db.with_connection(|conn| {
        conn.execute_batch(
            "INSERT INTO session (id, lane_key, source, workspace_id) VALUES
                 ('s-1', 'u:gui', 'gui', '/w/app'), ('s-2', 'u:cli', 'cli', '/elsewhere');
             INSERT INTO task (id, title, created_by, source_lane, workspace_id)
                 VALUES ('t-1', 't', 'u', 'u:gui', '/w/app');
             INSERT INTO conversation_messages (lane_key, role, content, session_id, workspace_root)
                 VALUES ('u:gui', 'user', 'hi', 's-1', '/w/app');
             INSERT INTO lane_followups (lane_key, kind, content, principal_json, workspace_path)
                 VALUES ('u:gui', 'followup', 'next', '{}', '/w/app');
             INSERT INTO file_assets (id, owner_id, sha256, filename, mime_type, size_bytes,
                                      storage_path, origin, project_root, rel_path) VALUES
                 ('by-product', 'owner-1', 'a', 'n.md', 'text/markdown', 1,
                  '/h/projects/-w-app/artifacts/loose/2026-09-01/01-n.md', 'produced',
                  '/w/app', 'loose/2026-09-01/01-n.md'),
                 ('pointer', 'owner-1', 'b', 'main.rs', 'text/plain', 1,
                  '/w/app/src/main.rs', 'produced', '/w/app', NULL),
                 ('lookalike', 'owner-1', 'c', 'x.md', 'text/markdown', 1,
                  '/h/projects/-w-app-old/artifacts/x.md', 'produced', '/w/app', 'x.md'),
                 ('other-project', 'owner-1', 'd', 'y.md', 'text/markdown', 1,
                  '/h/projects/-w-app/artifacts/y.md', 'produced', '/w/other', 'y.md');",
        )?;
        Ok(())
    })
    .unwrap();

    // `t-1` reads the column's default, `queued`: the run check is the
    // re-point's (`project_life`), this test is the rewrite's.
    let counts = f
        .store()
        .repoint_rows(old, new, old_dir, new_dir, InFlightRuns::Ignore)
        .unwrap();
    assert_eq!(
        counts,
        RebaseCounts {
            file_assets: 3,
            sessions: 1,
            tasks: 1,
            messages: 1,
            followups: 1,
        }
    );

    let address = |id: &str| -> (Option<String>, String) {
        f.db.with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT project_root, storage_path FROM file_assets WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap()
    };
    assert_eq!(
        address("by-product"),
        (
            Some(new.to_string()),
            "/h/projects/-w-app-moved/artifacts/loose/2026-09-01/01-n.md".to_string()
        )
    );
    assert_eq!(
        address("pointer"),
        (
            Some(new.to_string()),
            "/w/app-moved/src/main.rs".to_string()
        )
    );
    assert_eq!(
        address("lookalike"),
        (
            Some(new.to_string()),
            "/h/projects/-w-app-old/artifacts/x.md".to_string()
        ),
        "a prefix not followed by a separator is not the project's"
    );
    assert_eq!(
        address("other-project"),
        (
            Some("/w/other".to_string()),
            "/h/projects/-w-app/artifacts/y.md".to_string()
        ),
        "another project's row is not touched"
    );
    let count = |sql: &str| -> i64 {
        f.db.with_connection(|conn| Ok(conn.query_row(sql, [], |r| r.get(0))?))
            .unwrap()
    };
    assert_eq!(
        count("SELECT COUNT(*) FROM session WHERE workspace_id = '/w/app-moved'"),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM session WHERE workspace_id = '/elsewhere'"),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM task WHERE workspace_id = '/w/app-moved'"),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM conversation_messages WHERE workspace_root = '/w/app-moved'"),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM lane_followups WHERE workspace_path = '/w/app-moved'"),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM file_assets WHERE project_root = '/w/app'"),
        0
    );
}

/// `workspace_rows` counts what `repoint_rows` moves: a root named only by a
/// message and a follow-up is a root rows name.
#[test]
fn workspace_rows_counts_messages_and_followups_too() {
    let f = Fixture::new();
    f.db.with_connection(|conn| {
        conn.execute_batch(
            "INSERT INTO conversation_messages (lane_key, role, content, workspace_root)
                 VALUES ('u:gui', 'user', 'hi', '/w/app');
             INSERT INTO lane_followups (lane_key, kind, content, principal_json, workspace_path)
                 VALUES ('u:gui', 'followup', 'next', '{}', '/w/app');",
        )?;
        Ok(())
    })
    .unwrap();

    let rows = f.store().workspace_rows("/w/app", None).unwrap();
    assert_eq!(
        rows.counts,
        RebaseCounts {
            file_assets: 0,
            sessions: 0,
            tasks: 0,
            messages: 1,
            followups: 1,
        }
    );
    assert!(!rows.counts.is_empty());
}

/// I8: the mark says "nothing is at `storage_path`", and the re-base is what
/// rewrites `storage_path` — so it clears the mark with the address it
/// described. Otherwise a project moved by hand and then re-based stays hidden
/// from the Library for good.
#[test]
fn repoint_rows_clears_the_missing_mark_it_re_addresses() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"body");
    new.created = at(1);
    let (record, _) = f.store().put(new).unwrap();

    // The project is moved by hand; opening the artifact in the Library stamps
    // the row on the way to `ARTIFACT_GONE`.
    fs::remove_file(&record.storage_path).unwrap();
    assert!(f.store().resolve_content(&record.id, None).is_err());
    assert!(f.store().get(&record.id, OWNER).unwrap().unwrap().missing());
    assert_eq!(
        f.store().list(&ArtifactQuery::new(OWNER)).unwrap().1,
        0,
        "a marked row is hidden from the default query"
    );

    let old = f.project_root().to_string_lossy().to_string();
    let store_dir = crate::store::store_root(&f.scope())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    f.store()
        .repoint_rows(
            &old,
            "/tmp/moved-project",
            &store_dir,
            "/tmp/moved-dir",
            InFlightRuns::Refuse,
        )
        .unwrap();

    let after = f.store().get(&record.id, OWNER).unwrap().unwrap();
    assert!(
        !after.missing(),
        "the mark described the old address and went with it"
    );
    assert_eq!(
        f.store().list(&ArtifactQuery::new(OWNER)).unwrap().1,
        1,
        "the row is back in the Library"
    );
}

#[test]
fn a_failing_member_rolls_the_whole_repoint_back() {
    let f = Fixture::new();
    let old = f.project_root().to_string_lossy().to_string();
    let new_root = "/tmp/already-a-project";

    let scope = f.scope();
    let mut produced = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"a");
    produced.created = at(1);
    let (produced, _) = f.store().put(produced).unwrap();
    f.session("session-1", &old);
    f.task_in("task-1", &old, "completed");

    // The collision: `idx_file_assets_addr` is
    // (COALESCE(project_root, ''), in_project, rel_path), so re-keying this
    // by-product onto a root that already holds one at the same address
    // violates it. That is the failure the one transaction
    // exists for — two projects must not silently merge, and the other two
    // members, which the transaction has already rewritten by the time the
    // artifact statement runs, must not stay moved without it.
    f.asset_under("theirs", new_root, produced.rel_path.as_deref().unwrap());

    let store_dir = crate::store::store_root(&f.scope())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let err = f
        .store()
        .repoint_rows(
            &old,
            new_root,
            &store_dir,
            "/tmp/elsewhere-dir",
            InFlightRuns::Refuse,
        )
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("unique"),
        "expected the unique-index violation, got: {err}"
    );

    assert_eq!(
        f.store()
            .get(&produced.id, OWNER)
            .unwrap()
            .unwrap()
            .project_root
            .as_deref(),
        Some(old.as_str()),
        "the artifact row rolled back with the rest"
    );
    assert_eq!(
        f.members_at(&old),
        (1, 1),
        "the session and the run were re-keyed before the collision and rolled back"
    );
    assert_eq!(f.members_at(new_root), (0, 0));
}

#[test]
fn workspace_rows_counts_the_members_and_the_runs_in_flight() {
    let f = Fixture::new();
    let root = f.project_root().to_string_lossy().to_string();
    assert_eq!(
        f.store().workspace_rows(&root, None).unwrap(),
        WorkspaceRows::default()
    );
    assert!(
        f.store()
            .workspace_rows(&root, None)
            .unwrap()
            .counts
            .is_empty()
    );

    let scope = f.scope();
    let mut produced = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"a");
    produced.created = at(1);
    f.store().put(produced).unwrap();
    f.session("session-1", &root);
    f.task_in("done", &root, "completed");
    f.task_in("queued", &root, "queued");

    let rows = f.store().workspace_rows(&root, None).unwrap();
    assert_eq!(
        rows.counts,
        RebaseCounts {
            file_assets: 1,
            sessions: 1,
            tasks: 2,
            messages: 0,
            followups: 0,
        }
    );
    assert_eq!(
        rows.active_tasks, 0,
        "a queued run is counted apart from the running ones"
    );

    f.task_in("live", &root, "running");
    f.task_in("held", &root, "paused");
    assert_eq!(
        f.store().workspace_rows(&root, None).unwrap().active_tasks,
        2
    );
}

#[test]
fn busy_tasks_counts_queued_running_and_paused_but_not_a_finished_run() {
    let f = Fixture::new();
    let root = f.project_root().to_string_lossy().to_string();
    let other = "/some-other-root";

    assert_eq!(f.store().busy_tasks(&root).unwrap(), 0);

    f.task_in("done", &root, "completed");
    f.task_in("failed", &root, "failed");
    assert_eq!(
        f.store().busy_tasks(&root).unwrap(),
        0,
        "a finished run is not busy, whichever way it finished"
    );

    // Unlike `active_tasks`, a queued run counts here — it already named this
    // root and is about to resolve the store the moment it starts.
    f.task_in("queued", &root, "queued");
    assert_eq!(f.store().busy_tasks(&root).unwrap(), 1);

    f.task_in("live", &root, "running");
    f.task_in("held", &root, "paused");
    f.task_in("elsewhere", other, "running");
    assert_eq!(
        f.store().busy_tasks(&root).unwrap(),
        3,
        "a run under another root is not this root's business"
    );
}

#[test]
fn workspace_rows_scopes_the_one_member_that_has_an_owner() {
    let f = Fixture::new();
    let root = f.project_root().to_string_lossy().to_string();
    let scope = f.scope();

    let mut mine = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Mine", b"a");
    mine.created = at(1);
    f.store().put(mine).unwrap();
    let mut theirs = NewArtifact::new("owner-2", &scope, ArtifactKind::Markdown, "Theirs", b"b");
    theirs.created = at(1);
    f.store().put(theirs).unwrap();
    f.session("session-1", &root);
    f.task_in("task-1", &root, "completed");

    let all = f.store().workspace_rows(&root, None).unwrap();
    assert_eq!(all.counts.file_assets, 2);
    assert_eq!(all.other_owners, 0, "an unscoped count leaves nobody out");

    let mine = f.store().workspace_rows(&root, Some(OWNER)).unwrap();
    assert_eq!(mine.counts.file_assets, 1);
    assert_eq!(
        mine.other_owners, 1,
        "one artifact belongs to somebody else"
    );
    // `session` and `task` carry no owner, so their counts are the root's
    // either way — which is why the route refuses rather than half-scopes.
    assert_eq!(mine.counts.sessions, 1);
    assert_eq!(mine.counts.tasks, 1);

    let nobody = f.store().workspace_rows(&root, Some("owner-3")).unwrap();
    assert_eq!(nobody.counts.file_assets, 0);
    assert_eq!(nobody.other_owners, 2);
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
    assert_eq!(f.store().verify(Some(&root)).unwrap().missing, 0);

    fs::remove_file(&a.storage_path).unwrap();
    fs::remove_file(&c.storage_path).unwrap();

    assert_eq!(f.store().verify(Some(&root)).unwrap().missing, 1);
    assert!(f.store().get(&a.id, OWNER).unwrap().unwrap().missing());
    assert!(!f.store().get(&b.id, OWNER).unwrap().unwrap().missing());
    assert!(
        !f.store().get(&c.id, OWNER).unwrap().unwrap().missing(),
        "the home row is outside the verified root"
    );

    // Idempotent: a second pass still reports the same count.
    assert_eq!(f.store().verify(Some(&root)).unwrap().missing, 1);

    // None sweeps every root.
    assert_eq!(f.store().verify(None).unwrap().missing, 2);
    assert!(f.store().get(&c.id, OWNER).unwrap().unwrap().missing());
}

// ============================================================================
// The hand edit (§4.8) — user-edit detection as a version
// ============================================================================

/// Restores the rotation crash hook even if the test panics mid-assertion.
struct RotateCrashGuard;

impl RotateCrashGuard {
    fn after(step: RotateStep) -> Self {
        crash_rotate_after(step);
        Self
    }
}

impl Drop for RotateCrashGuard {
    fn drop(&mut self) {
        clear_rotate_crash();
    }
}

/// One artifact at v1, with its head overwritten by hand.
fn hand_edited(body: &str) -> (Fixture, ArtifactRecord) {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(
        OWNER,
        &scope,
        ArtifactKind::Markdown,
        "Notes",
        b"one\ntwo\n",
    );
    new.created = at(1);
    let (record, _) = f.store().put(new).unwrap();
    fs::write(&record.storage_path, body).unwrap();
    (f, record)
}

/// Restores the hash hook even if the test panics mid-assertion.
struct HashHook;

impl HashHook {
    fn install(hook: impl Fn() + 'static) -> Self {
        on_head_hash(Some(std::sync::Arc::new(hook)));
        Self
    }
}

impl Drop for HashHook {
    fn drop(&mut self) {
        on_head_hash(None);
    }
}

#[test]
fn an_unchanged_head_is_never_re_hashed() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(
        OWNER,
        &scope,
        ArtifactKind::Markdown,
        "Notes",
        b"one\ntwo\n",
    );
    new.created = at(1);
    let (record, _) = f.store().put(new).unwrap();

    // The put stamped the head it had just written, so not even the first read
    // pays for a hash — and no later one does either.
    let before = head_hashes();
    for _ in 0..3 {
        f.store().resolve_content(&record.id, None).unwrap();
    }
    assert_eq!(
        head_hashes(),
        before,
        "an unchanged head is never re-hashed"
    );

    // The changed path still rotates, hashing exactly once...
    fs::write(&record.storage_path, "one\ntwo\nthree\n").unwrap();
    let before = head_hashes();
    let edit = f
        .store()
        .resolve_content_with_edit(&record.id, None)
        .unwrap()
        .1
        .expect("the edit is recorded");
    assert_eq!(edit.version, 2);
    assert_eq!(edit.sha256, sha256_hex(b"one\ntwo\nthree\n"));
    assert_eq!(head_hashes(), before + 1);

    // ...and the rotation re-stamps, so the reads after it are free again.
    let before = head_hashes();
    f.store().resolve_content(&record.id, None).unwrap();
    assert_eq!(head_hashes(), before);

    // The stamp is the store's own bookkeeping: a client is served the
    // writer's metadata, which here is none at all.
    let stamped = f.store().get(&record.id, OWNER).unwrap().unwrap();
    assert!(stamped.metadata_json.is_some(), "the stamp is on the row");
    assert_eq!(stamped.metadata(), None, "and never on the wire");
}

#[test]
fn a_head_stamp_rides_beside_the_writers_metadata() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"body\n");
    new.created = at(1);
    new.metadata_json = Some(r#"{"source":"web"}"#);
    let (record, _) = f.store().put(new).unwrap();

    let stored: serde_json::Value =
        serde_json::from_str(record.metadata_json.as_deref().unwrap()).unwrap();
    assert_eq!(stored["source"], "web");
    assert!(stored.get(HEAD_STAMP_KEY).is_some());
    assert_eq!(
        record.metadata(),
        Some(serde_json::json!({"source": "web"}))
    );
}

#[test]
fn the_head_is_hashed_with_the_connection_free() {
    let (f, v1) = hand_edited("one\ntwo\nthree\n");
    let db = f.db.clone();
    let free = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = free.clone();

    // R32: while the head is being hashed another thread must be able to take
    // the connection. Inside `with_connection` — where this check used to run —
    // the hand-off below never completes.
    let _hook = HashHook::install(move || {
        let db = db.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let taken = db.with_connection(|_| Ok(())).is_ok();
            let _ = tx.send(taken);
        });
        if rx.recv_timeout(std::time::Duration::from_secs(5)) == Ok(true) {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    });

    let edit = f.store().resolve_content_with_edit(&v1.id, None).unwrap().1;
    assert!(edit.is_some(), "the edit is still recorded");
    assert!(
        free.load(std::sync::atomic::Ordering::SeqCst),
        "the connection was held while the head was hashed"
    );
}

#[test]
fn a_read_records_a_hand_edited_head_as_a_version_authored_by_nobody() {
    let (f, v1) = hand_edited("one\ntwo\nthree\n");

    let (path, edit) = f.store().resolve_content_with_edit(&v1.id, None).unwrap();
    assert_eq!(path, PathBuf::from(&v1.storage_path));
    let edit = edit.expect("the edit is reported so the daemon can announce it");
    assert_eq!(edit.version, 2);
    assert_eq!(edit.sha256, sha256_hex(b"one\ntwo\nthree\n"));
    assert_eq!(edit.size_bytes, 14);
    assert_eq!(edit.version_count, 2);
    assert!(!edit.missing());

    let rows = f.store().versions(&v1.id).unwrap();
    assert_eq!(rows.len(), 2);
    let v2 = &rows[0];
    assert_eq!(v2.version, 2);
    assert_eq!(v2.author_agent_id, None, "nobody in OpenAlpaca wrote it");
    assert_eq!(v2.note.as_deref(), Some(USER_EDIT_NOTE));
    assert_eq!(v2.rel_path, v1.rel_path.clone().unwrap());
    // The bytes v2 replaced were overwritten in place, so there is nothing to
    // count against — the same `NULL` a put records when v(N-1) is gone.
    assert_eq!((v2.added_lines, v2.removed_lines), (None, None));

    // v1's row points at the slot its bytes would have been rotated into.
    // Nothing is there, and saying so is the truth about a file edited in place.
    assert_eq!(
        rows[1].rel_path,
        "loose/2026-09-01/.versions/01-notes/v1.md"
    );
    let err = f.store().resolve_content(&v1.id, Some(1)).unwrap_err();
    assert_eq!(
        err.downcast_ref::<ArtifactError>().unwrap().code(),
        "ARTIFACT_GONE"
    );
}

#[test]
fn the_hand_edit_is_recorded_exactly_once() {
    let (f, v1) = hand_edited("edited\n");

    assert!(
        f.store()
            .resolve_content_with_edit(&v1.id, None)
            .unwrap()
            .1
            .is_some()
    );
    // Every later read finds the bytes the row now describes.
    for _ in 0..3 {
        assert!(
            f.store()
                .resolve_content_with_edit(&v1.id, None)
                .unwrap()
                .1
                .is_none()
        );
    }
    let root = f.project_root().to_string_lossy().to_string();
    assert!(f.store().verify(Some(&root)).unwrap().user_edits.is_empty());
    assert_eq!(f.store().versions(&v1.id).unwrap().len(), 2);
}

#[test]
fn verify_records_the_hand_edits_it_finds() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut a = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Alpha", b"a\n");
    a.created = at(1);
    let (a, _) = f.store().put(a).unwrap();
    let mut b = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Beta", b"b\n");
    b.created = at(1);
    let (b, _) = f.store().put(b).unwrap();

    fs::write(&a.storage_path, "a edited\n").unwrap();

    let root = f.project_root().to_string_lossy().to_string();
    let report = f.store().verify(Some(&root)).unwrap();
    assert_eq!(report.missing, 0);
    assert_eq!(report.user_edits.len(), 1);
    assert_eq!(report.user_edits[0].id, a.id);
    assert_eq!(report.user_edits[0].version, 2);

    assert_eq!(f.store().get(&b.id, OWNER).unwrap().unwrap().version, 1);
}

#[test]
fn a_missing_head_is_never_rotated() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"body\n");
    new.created = at(1);
    let (record, _) = f.store().put(new).unwrap();

    // Absent bytes: the sweep marks the row and records no edit.
    fs::remove_file(&record.storage_path).unwrap();
    let root = f.project_root().to_string_lossy().to_string();
    let report = f.store().verify(Some(&root)).unwrap();
    assert_eq!(report.missing, 1);
    assert!(report.user_edits.is_empty());
    assert!(f.store().get(&record.id, OWNER).unwrap().unwrap().missing());

    // The bytes come back, different — and the row is still `missing`, which
    // only a put clears. A missing head is never rotated (§4.8): the version
    // history of a row whose bytes went away and returned is the writer's to
    // re-establish, not the reader's.
    fs::write(&record.storage_path, "restored by hand\n").unwrap();
    let report = f.store().verify(Some(&root)).unwrap();
    assert!(report.user_edits.is_empty());
    assert_eq!(
        f.store().get(&record.id, OWNER).unwrap().unwrap().version,
        1
    );
    assert_eq!(f.store().versions(&record.id).unwrap().len(), 1);
}

#[test]
fn an_upload_is_never_rotated() {
    let f = Fixture::new();
    let dir = f.artifacts_root().join("loose/2026-09-01");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("01-notes.md");
    fs::write(&path, "uploaded\n").unwrap();
    f.foreign_row("upload-1", "loose/2026-09-01/01-notes.md", &path);

    // `foreign_row` writes sha 'sha', which no bytes hash to — so the only
    // thing keeping this row out of the rotation is its origin.
    assert!(
        f.store()
            .resolve_content_with_edit("upload-1", None)
            .unwrap()
            .1
            .is_none()
    );
    assert_eq!(f.store().versions("upload-1").unwrap().len(), 0);
}

#[test]
fn a_crash_between_the_rotation_steps_records_nothing() {
    for step in [RotateStep::PreviousRepointed, RotateStep::VersionInserted] {
        let (f, v1) = hand_edited("edited\n");
        {
            let _crash = RotateCrashGuard::after(step);
            let err = f
                .store()
                .resolve_content_with_edit(&v1.id, None)
                .unwrap_err();
            assert!(
                err.to_string().contains("simulated crash"),
                "expected the injected failure at {step:?}, got: {err}"
            );
        }

        // The transaction rolled back: the row, the version history and v1's
        // own rel_path are all exactly as they were.
        let after = f.store().get(&v1.id, OWNER).unwrap().unwrap();
        assert_eq!(after.version, 1, "{step:?}");
        assert_eq!(after.sha256, v1.sha256, "{step:?}");
        assert_eq!(after.version_count, 1, "{step:?}");
        let rows = f.store().versions(&v1.id).unwrap();
        assert_eq!(rows.len(), 1, "{step:?}");
        assert_eq!(rows[0].rel_path, v1.rel_path.clone().unwrap(), "{step:?}");

        // And the next read records the edit exactly once.
        let edit = f
            .store()
            .resolve_content_with_edit(&v1.id, None)
            .unwrap()
            .1
            .expect("the retry records it");
        assert_eq!(edit.version, 2, "{step:?}");
        assert_eq!(f.store().versions(&v1.id).unwrap().len(), 2, "{step:?}");
    }
}

#[test]
fn a_put_onto_an_unnoticed_hand_edit_records_the_edit_first() {
    // Nothing has read this artifact since it was edited by hand, so the agent
    // is about to write over bytes no version row describes.
    let (f, v1) = hand_edited("mine\n");

    let scope = f.scope();
    let mut third = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"theirs\n");
    third.created = at(1);
    third.agent_id = Some("writer::1");
    let (record, created) = f.store().put(third).unwrap();
    assert!(!created);
    assert_eq!(
        record.version, 3,
        "the hand edit is v2 and the agent's write is v3"
    );
    assert_eq!(record.version_count, 3);

    let rows = f.store().versions(&v1.id).unwrap();
    assert_eq!(rows.len(), 3);

    // v3 is the agent's, and its bytes are at the head.
    assert_eq!(rows[0].version, 3);
    assert_eq!(rows[0].author_agent_id.as_deref(), Some("writer::1"));
    assert_eq!(rows[0].sha256, sha256_hex(b"theirs\n"));

    // v2 is the hand edit, and every row's sha describes the bytes its
    // rel_path points at — which is the whole point.
    assert_eq!(rows[1].version, 2);
    assert_eq!(rows[1].author_agent_id, None);
    assert_eq!(rows[1].note.as_deref(), Some(USER_EDIT_NOTE));
    assert_eq!(rows[1].sha256, sha256_hex(b"mine\n"));
    let v2_path = f.store().resolve_content(&v1.id, Some(2)).unwrap();
    assert_eq!(fs::read_to_string(v2_path).unwrap(), "mine\n");

    // v1's bytes were overwritten in place by the hand edit; its row says so
    // rather than claiming the user's bytes.
    assert_eq!(rows[2].version, 1);
    assert_eq!(rows[2].sha256, v1.sha256);
    let err = f.store().resolve_content(&v1.id, Some(1)).unwrap_err();
    assert_eq!(
        err.downcast_ref::<ArtifactError>().unwrap().code(),
        "ARTIFACT_GONE"
    );

    // And the read that follows finds nothing left to record.
    assert!(
        f.store()
            .resolve_content_with_edit(&v1.id, None)
            .unwrap()
            .1
            .is_none()
    );
}

#[test]
fn a_crash_between_the_two_rotations_records_neither() {
    let (f, v1) = hand_edited("mine\n");
    let scope = f.scope();

    {
        let _crash = CrashGuard::after(WriteStep::UserEditRotated);
        let mut third =
            NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"theirs\n");
        third.created = at(1);
        let err = f.store().put(third).unwrap_err();
        assert!(
            err.to_string().contains("simulated crash"),
            "expected the injected failure, got: {err}"
        );
    }

    // Both rotations are in one transaction, so the abort leaves the store
    // exactly as it was — the user's bytes still standing at the head.
    let after = f.store().get(&v1.id, OWNER).unwrap().unwrap();
    assert_eq!(after.version, 1);
    assert_eq!(after.sha256, v1.sha256);
    assert_eq!(after.version_count, 1);
    let rows = f.store().versions(&v1.id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].rel_path, v1.rel_path.clone().unwrap());
    assert_eq!(fs::read_to_string(&after.storage_path).unwrap(), "mine\n");

    // So the next read records the edit exactly once, as if nothing had tried.
    let edit = f
        .store()
        .resolve_content_with_edit(&v1.id, None)
        .unwrap()
        .1
        .expect("the retry records it");
    assert_eq!(edit.version, 2);
    assert_eq!(f.store().versions(&v1.id).unwrap().len(), 2);
}

#[test]
fn a_put_after_a_recorded_hand_edit_supersedes_it() {
    let (f, v1) = hand_edited("mine\n");
    f.store().resolve_content(&v1.id, None).unwrap();

    let scope = f.scope();
    let mut third = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "Notes", b"theirs\n");
    third.created = at(1);
    third.agent_id = Some("writer::1");
    let (record, created) = f.store().put(third).unwrap();
    assert!(!created);
    assert_eq!(record.version, 3);

    let rows = f.store().versions(&v1.id).unwrap();
    assert_eq!(rows[0].version, 3);
    assert_eq!(rows[0].author_agent_id.as_deref(), Some("writer::1"));
    // v2 — the hand edit — kept its bytes: the put rotated them into the slot.
    let v2 = f.store().resolve_content(&v1.id, Some(2)).unwrap();
    assert_eq!(fs::read_to_string(v2).unwrap(), "mine\n");
    assert_eq!(
        (rows[0].added_lines, rows[0].removed_lines),
        (Some(1), Some(1))
    );
}

// ============================================================================
// diff
// ============================================================================

#[test]
fn diff_rejects_the_non_text_kinds() {
    let f = Fixture::new();
    let scope = f.scope();
    // Distinct titles, because both kinds fall through to `.bin`: one title
    // would make the second pair a *supersede* of the first artifact rather
    // than a second artifact.
    for (kind, title) in [
        (ArtifactKind::Image, "Shot"),
        (ArtifactKind::Binary, "Blob"),
    ] {
        let mut one = NewArtifact::new(OWNER, &scope, kind, title, b"a");
        one.created = at(1);
        let (record, _) = f.store().put(one).unwrap();
        let mut two = NewArtifact::new(OWNER, &scope, kind, title, b"b");
        two.created = at(1);
        f.store().put(two).unwrap();

        let err = f.store().diff(&record.id, 1, 2).unwrap_err();
        assert_eq!(
            err.downcast_ref::<ArtifactError>().unwrap().code(),
            "NOT_DIFFABLE",
            "kind {kind:?} must not be diffable"
        );
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

/// R32. The patch is bounded by size before a byte is read: a version above
/// `MAX_DIFF_BYTES` is `DIFF_TOO_LARGE`, not 8 MiB in memory and a Myers run
/// over a million lines. The version rows' stored `added_lines`/`removed_lines`
/// remain the answer for a file that big.
///
/// The 8 MiB is written straight to disk on purpose: `put` would have to carry
/// the whole thing through memory to set up the same state, and the number is
/// spelled here rather than read from the constant so that raising the cap
/// cannot silently pass.
#[test]
fn a_version_over_the_diff_cap_is_diff_too_large() {
    let f = Fixture::new();
    let id = versions_of(&f, ArtifactKind::Markdown, &["one\n", "two\n"]);
    let head = f.store().get(&id, OWNER).unwrap().unwrap().storage_path;
    fs::write(&head, vec![b'x'; 8 * 1024 * 1024 + 1]).unwrap();

    let err = f.store().diff(&id, 1, 2).unwrap_err();
    assert_eq!(
        err.downcast_ref::<ArtifactError>()
            .unwrap_or_else(|| panic!("not an ArtifactError: {err}"))
            .code(),
        "DIFF_TOO_LARGE"
    );
    assert!(
        err.to_string().contains("version 2"),
        "the refusal names the version that is too big: {err}"
    );
}

/// One byte under the cap is an ordinary diff — the bound refuses what it says
/// it refuses and nothing else.
#[test]
fn a_version_at_the_diff_cap_still_diffs() {
    let f = Fixture::new();
    let id = versions_of(&f, ArtifactKind::Markdown, &["one\n", "two\n"]);
    let head = f.store().get(&id, OWNER).unwrap().unwrap().storage_path;
    let mut body = vec![b'x'; 8 * 1024 * 1024 - 1];
    body.push(b'\n');
    fs::write(&head, &body).unwrap();

    let diff = f.store().diff(&id, 1, 2).unwrap();
    assert_eq!((diff.added_lines, diff.removed_lines), (1, 1));
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

/// The T23 re-review's Minor 8, as §4.8 leaves it. An interrupted put parks
/// bytes at the head that no row describes; the recovering put now *records*
/// them as the version nobody wrote before writing its own, so the counts are
/// still taken against the bytes the rows claim — they are simply one more
/// version than they used to be. Nothing is ever counted against a file no row
/// describes.
#[test]
fn the_line_counts_are_taken_against_the_version_the_rows_describe() {
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
    let (v3, _) = f.store().put(retry).unwrap();
    assert_eq!(v3.version, 3, "the orphan became v2 and this is v3");

    let rows = f.store().versions(&v1.id).unwrap();
    assert_eq!(rows[0].version, 3);
    assert_eq!(
        (rows[0].added_lines, rows[0].removed_lines),
        (Some(3), Some(1)),
        "counted against v2 — the orphan, now a version with a row of its own"
    );

    // …and the patch over the pair the rows describe says the same thing.
    let diff = f.store().diff(&v1.id, 2, 3).unwrap();
    assert_eq!((diff.added_lines, diff.removed_lines), (3, 1));
    assert_eq!(patch_totals(&diff.patch), (3, 1));

    // v1's own bytes are still readable and still its own.
    let path = f.store().resolve_content(&v1.id, Some(1)).unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "alpha\nbeta\n");
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

// ============================================================================
// Stage 2 — result files (ST-12): the column, the filter, Open (ST-18)
// ============================================================================

/// Every record reads `in_project` — `false` for a by-product — a by-product
/// always opens (ST-18), and `list` filters on the column. A result file's
/// side of each is pinned in S4a.3, against rows R35's method writes (R39:
/// no hand `INSERT`).
#[test]
fn a_by_product_is_outside_the_project_and_opens() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "notes", b"a draft\n");
    new.created = at(1);
    let (by_product, _) = f.store().put(new).unwrap();

    assert!(!by_product.in_project);
    assert!(by_product.openable(), "every by-product opens");
    let ids = |in_project: Option<bool>| -> Vec<String> {
        let mut q = ArtifactQuery::new(OWNER);
        q.in_project = in_project;
        f.store()
            .list(&q)
            .unwrap()
            .0
            .into_iter()
            .map(|r| r.id)
            .collect()
    };
    assert_eq!(ids(Some(false)), vec![by_product.id.clone()]);
    assert!(ids(Some(true)).is_empty(), "no result file yet");
    assert_eq!(ids(None), vec![by_product.id.clone()], "absent: both");
}

// ============================================================================
// R35 — the pointer row (ST-12)
// ============================================================================

/// The row R35 writes: produced, ready, in the project, the real file's
/// address, no version row, nothing under `.versions/` or the project's
/// `artifacts/`, and the file untouched.
#[test]
fn recording_a_result_file_writes_its_pointer_row_and_no_version() {
    let f = Fixture::new();
    f.task("3f2a1b7c-0000-4000-8000-000000000000", "Build the parser");
    let path = f.project_root().join("src").join("main.rs");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"fn main() {}\n").unwrap();
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    let root = f.project_root().to_string_lossy().into_owned();

    let (row, inserted) = f
        .store()
        .record_project_file(NewProjectFile {
            owner_id: OWNER,
            project_root: &root,
            rel_path: "src/main.rs",
            content: b"fn main() {}\n",
            task_id: Some("3f2a1b7c-0000-4000-8000-000000000000"),
            agent_id: Some("code_agent::a1"),
        })
        .unwrap();

    assert!(inserted);
    assert!(row.in_project);
    assert_eq!(row.origin, ArtifactOrigin::Produced);
    assert_eq!(row.project_root.as_deref(), Some(root.as_str()));
    assert_eq!(row.rel_path.as_deref(), Some("src/main.rs"));
    assert_eq!(row.name, "src/main.rs");
    assert_eq!(row.storage_path, path.to_string_lossy());
    assert_eq!(row.kind, Some(ArtifactKind::Code));
    assert_eq!(row.mime_type, "text/plain");
    assert_eq!(row.sha256, sha256_hex(b"fn main() {}\n"));
    assert_eq!(row.size_bytes, 13);
    assert_eq!(
        row.task_id.as_deref(),
        Some("3f2a1b7c-0000-4000-8000-000000000000")
    );
    assert_eq!(row.agent_id.as_deref(), Some("code_agent::a1"));
    assert_eq!((row.version, row.version_count, row.pinned), (1, 0, false));
    assert!(row.summary.is_none());
    assert!(row.metadata().is_none(), "only the store's own head stamp");
    let asset = FileAssetRepository::new(&f.db)
        .get_by_id(&row.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        asset.status.as_str(),
        "ready",
        "the extractor never reads it"
    );
    assert!(
        f.store().versions(&row.id).unwrap().is_empty(),
        "no artifact_versions row"
    );
    assert!(
        !f.artifacts_root().exists(),
        "nothing is written under the project's artifacts/"
    );
    assert!(!f.project_root().join("src").join(".versions").exists());
    assert_untouched(&path, b"fn main() {}\n", mtime);
}

/// R39: a second `file_write` of a path updates its own pointer row — never a
/// by-product that shares its address (fails when the lookup ignores
/// `in_project`).
#[test]
fn a_second_file_write_of_a_path_updates_only_its_own_row() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "notes", b"a draft\n");
    new.created = at(1);
    let (by_product, _) = f.store().put(new).unwrap();
    let rel = by_product.rel_path.clone().unwrap();
    assert_eq!(rel, "loose/2026-09-01/01-notes.md");

    let (first, path, _) = f.result_file(&rel, b"first\n");
    fs::write(&path, b"second, longer\n").unwrap();
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    let root = f.project_root().to_string_lossy().into_owned();
    let (second, inserted) = f
        .store()
        .record_project_file(NewProjectFile {
            owner_id: OWNER,
            project_root: &root,
            rel_path: &rel,
            content: b"second, longer\n",
            task_id: None,
            agent_id: None,
        })
        .unwrap();

    assert!(!inserted, "the second write updates");
    assert_eq!(second.id, first.id);
    assert_eq!(second.sha256, sha256_hex(b"second, longer\n"));
    assert_eq!(second.size_bytes, 15);
    assert_eq!(
        (second.version, second.version_count),
        (1, 0),
        "no version: a result file's history is git's (ST-12)"
    );
    let untouched = f.store().get(&by_product.id, OWNER).unwrap().unwrap();
    assert!(!untouched.in_project);
    assert_eq!(untouched.storage_path, by_product.storage_path);
    assert_eq!(untouched.sha256, by_product.sha256);
    assert_eq!(fs::read(&by_product.storage_path).unwrap(), b"a draft\n");
    assert_eq!(file_asset_count(&f), 2);
    assert_untouched(&path, b"second, longer\n", mtime);
}

/// R39: a result file whose path equals an upload's address is a row of its
/// own, and the upload's row is left alone (fails when the index or the
/// lookup ignores `in_project`).
#[test]
fn a_pointer_path_equal_to_an_uploads_address_leaves_the_upload_row_alone() {
    let f = Fixture::new();
    let scope = f.scope();
    let upload = crate::UploadStore::new(&f.db)
        .put(crate::NewUpload {
            owner_id: OWNER,
            filename: "report.txt",
            mime_type: "text/plain",
            data: b"uploaded\n",
            scope: &scope,
            created: at(1),
        })
        .unwrap()
        .asset;
    let upload_row = f.store().get(&upload.id, OWNER).unwrap().unwrap();
    let rel = upload_row.rel_path.clone().unwrap();
    assert_eq!(rel, "2026-09-01/01-report.txt");

    let (result, path, mtime) = f.result_file(&rel, b"the user's report\n");

    assert_ne!(result.id, upload.id);
    assert!(result.in_project);
    let after = f.store().get(&upload.id, OWNER).unwrap().unwrap();
    assert_eq!(after.origin, ArtifactOrigin::Upload);
    assert!(!after.in_project);
    assert_eq!(after.storage_path, upload_row.storage_path);
    assert_eq!(after.sha256, upload_row.sha256);
    assert_eq!(fs::read(&upload_row.storage_path).unwrap(), b"uploaded\n");
    assert_untouched(&path, b"the user's report\n", mtime);
}

/// A pointer row another owner recorded is never updated by this one.
#[test]
fn a_result_file_another_owner_recorded_is_never_taken_over() {
    let f = Fixture::new();
    let (theirs, path, mtime) = f.result_file("docs/plan.md", b"theirs\n");
    let root = f.project_root().to_string_lossy().into_owned();

    let err = f
        .store()
        .record_project_file(NewProjectFile {
            owner_id: "owner-2",
            project_root: &root,
            rel_path: "docs/plan.md",
            content: b"mine\n",
            task_id: None,
            agent_id: None,
        })
        .unwrap_err();

    assert!(
        format!("{err:#}").contains("belongs to another owner"),
        "{err:#}"
    );
    let row = f.store().get(&theirs.id, OWNER).unwrap().unwrap();
    assert_eq!(row.sha256, theirs.sha256);
    assert_untouched(&path, b"theirs\n", mtime);
}

/// R35's guards on a result file's address (S4a's deferred (4)): a project
/// root that is not absolute, and a path that is empty, absolute, or climbs
/// out with `..` — after its `.` components are dropped — are each refused,
/// and nothing is recorded for any of them.
#[test]
fn a_result_file_with_no_address_in_its_project_is_refused_and_records_nothing() {
    let f = Fixture::new();
    let root = f.project_root().to_string_lossy().into_owned();
    const PLAIN: &str = "must be relative and plain";
    let cases = [
        ("relative/root", "docs/plan.md", "must be an absolute path"),
        (root.as_str(), "", PLAIN),
        (root.as_str(), ".", PLAIN),
        (root.as_str(), "./", PLAIN),
        (root.as_str(), "/etc/passwd", PLAIN),
        (root.as_str(), "../outside.md", PLAIN),
        (root.as_str(), "docs/../plan.md", PLAIN),
        (root.as_str(), "docs/./../../outside.md", PLAIN),
    ];

    for (project_root, rel_path, refusal) in cases {
        let err = f
            .store()
            .record_project_file(NewProjectFile {
                owner_id: OWNER,
                project_root,
                rel_path,
                content: b"x\n",
                task_id: None,
                agent_id: None,
            })
            .unwrap_err();
        assert!(
            format!("{err:#}").contains(refusal),
            "{project_root} {rel_path:?}: {err:#}"
        );
    }
    assert_eq!(file_asset_count(&f), 0);
}

/// S4a's deferred (4): a result file's path is normalised lexically before it
/// is recorded — `docs/./plan.md`, `docs//plan.md` and `./docs/plan.md` are
/// `docs/plan.md` — so one file has one row, its name, address and storage
/// path in one spelling, whatever spelling the caller passed.
#[test]
fn a_result_files_path_is_recorded_in_one_spelling() {
    let f = Fixture::new();
    let (first, path, _) = f.result_file("docs/plan.md", b"# Plan\n");
    let root = f.project_root().to_string_lossy().into_owned();

    for spelling in ["docs/./plan.md", "docs//plan.md", "./docs/plan.md"] {
        let (row, inserted) = f
            .store()
            .record_project_file(NewProjectFile {
                owner_id: OWNER,
                project_root: &root,
                rel_path: spelling,
                content: b"# Plan\n",
                task_id: None,
                agent_id: None,
            })
            .unwrap();
        assert!(!inserted, "{spelling}: the row docs/plan.md already has");
        assert_eq!(row.id, first.id, "{spelling}");
        assert_eq!(row.rel_path.as_deref(), Some("docs/plan.md"), "{spelling}");
        assert_eq!(row.name, "docs/plan.md", "{spelling}");
        assert_eq!(row.storage_path, path.to_string_lossy(), "{spelling}");
    }
    assert_eq!(file_asset_count(&f), 1);
}

/// ST-12, ST-18: a result file's record says it is in the project, Open is
/// offered only when a kind keeps its extension, and `list` filters on the
/// column — against rows R35's method wrote.
#[test]
fn a_result_files_record_says_so_and_the_list_filters_on_it() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "notes", b"a draft\n");
    new.created = at(1);
    let (by_product, _) = f.store().put(new).unwrap();
    let (code, code_path, code_mtime) = f.result_file("src/main.rs", b"fn main() {}\n");
    let (script, script_path, script_mtime) = f.result_file("scripts/build.sh", b"echo hi\n");

    assert!(code.in_project && script.in_project);
    assert!(code.openable(), "a kind keeps .rs");
    assert!(!script.openable(), "a shell script is Reveal only (ST-18)");
    let ids = |in_project: Option<bool>| -> Vec<String> {
        let mut q = ArtifactQuery::new(OWNER);
        q.in_project = in_project;
        let mut ids: Vec<String> = f
            .store()
            .list(&q)
            .unwrap()
            .0
            .into_iter()
            .map(|r| r.id)
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(ids(Some(false)), vec![by_product.id.clone()]);
    let mut results = vec![code.id.clone(), script.id.clone()];
    results.sort();
    assert_eq!(ids(Some(true)), results);
    assert_eq!(ids(None).len(), 3, "absent: both");
    assert_untouched(&code_path, b"fn main() {}\n", code_mtime);
    assert_untouched(&script_path, b"echo hi\n", script_mtime);
}

/// The lead's ruling R-T20: a by-product's folder is its store's
/// `artifacts/`, an upload's its store's `uploads/` — the home store's or the
/// project's folder under `projects/` — and a result file has none (its bytes
/// are in the project, R37).
#[test]
fn a_rows_content_root_is_its_stores_artifacts_or_uploads_folder_and_a_result_file_has_none() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "notes", b"a draft\n");
    new.created = at(1);
    let (by_product, _) = f.store().put(new).unwrap();
    let home = StoreScope::Home;
    let mut mine = NewArtifact::new(OWNER, &home, ArtifactKind::Markdown, "mine", b"mine\n");
    mine.created = at(1);
    let (home_by_product, _) = f.store().put(mine).unwrap();
    let upload = crate::UploadStore::new(&f.db)
        .put(crate::NewUpload {
            owner_id: OWNER,
            filename: "report.txt",
            mime_type: "text/plain",
            data: b"uploaded\n",
            scope: &scope,
            created: at(1),
        })
        .unwrap()
        .asset;
    let upload = f.store().get(&upload.id, OWNER).unwrap().unwrap();
    let (result, _, _) = f.result_file("src/main.rs", b"fn main() {}\n");

    assert_eq!(by_product.content_root().unwrap(), f.artifacts_root());
    assert_eq!(
        home_by_product.content_root().unwrap(),
        f.home_root().join("artifacts")
    );
    assert_eq!(
        upload.content_root().unwrap(),
        crate::store::store_root(&scope).unwrap().join("uploads")
    );
    let err = result.content_root().unwrap_err();
    assert!(
        format!("{err:#}").contains("is a result file in its project"),
        "{err:#}"
    );
}

/// R39: `put` never adopts a pointer row whose path is artifact-shaped — the
/// by-product is a new row under the store, at the name the result file's row
/// holds, and the result file's row and file are untouched (fails when
/// `put`'s scan, or its name reservation, ignores `in_project`).
#[test]
fn put_never_adopts_a_pointer_row_at_an_artifact_shaped_path() {
    let f = Fixture::new();
    let (pointer, file, mtime) =
        f.result_file("loose/2026-09-01/01-notes.md", b"the user's notes\n");
    let scope = f.scope();
    let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, "notes", b"a draft\n");
    new.created = at(1);

    let (by_product, created) = f.store().put(new).unwrap();

    assert!(
        created,
        "a new by-product, not a version of the result file"
    );
    assert_ne!(by_product.id, pointer.id);
    assert!(!by_product.in_project);
    assert_eq!(
        by_product.rel_path.as_deref(),
        Some("loose/2026-09-01/01-notes.md"),
        "the name the result file's row holds is not stepped past"
    );
    assert!(Path::new(&by_product.storage_path).starts_with(f.artifacts_root()));
    assert_eq!(fs::read(&by_product.storage_path).unwrap(), b"a draft\n");
    let after = f.store().get(&pointer.id, OWNER).unwrap().unwrap();
    assert!(after.in_project);
    assert_eq!(after.storage_path, pointer.storage_path);
    assert_eq!(after.sha256, pointer.sha256);
    assert_eq!(after.version, 1);
    assert!(f.store().versions(&pointer.id).unwrap().is_empty());
    assert_untouched(&file, b"the user's notes\n", mtime);
}

// ============================================================================
// R37 — every reader goes through the resolver; no version is ever written
// ============================================================================

/// R39: a user's edit of a result file records no version — the read-time
/// check refreshes the row's hash and size and nothing else, and the sweep
/// passes result files over, neither recording an edit nor marking a deleted
/// one missing (fails when `verify` or the read check ignores `in_project`).
#[test]
fn a_users_edit_of_a_result_file_records_no_version() {
    let f = Fixture::new();
    let (row, path, _) = f.result_file("docs/plan.md", b"# Plan\n");
    let (gone, gone_path, _) = f.result_file("docs/old.md", b"# Old\n");
    // The user edits one file in their editor and deletes another.
    fs::write(&path, b"# Plan, revised by hand\n").unwrap();
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    fs::remove_file(&gone_path).unwrap();
    let root = f.project_root().to_string_lossy().into_owned();

    let report = f.store().verify(Some(&root)).unwrap();
    assert!(report.user_edits.is_empty(), "{report:?}");
    assert_eq!(
        report.missing, 0,
        "the sweep never marks a result file: {report:?}"
    );
    assert!(
        f.store()
            .get(&gone.id, OWNER)
            .unwrap()
            .unwrap()
            .missing_since
            .is_none()
    );

    let served = f.store().resolve_content(&row.id, None).unwrap();
    assert_eq!(served, path.canonicalize().unwrap());
    let after = f.store().get(&row.id, OWNER).unwrap().unwrap();
    assert_eq!(after.sha256, sha256_hex(b"# Plan, revised by hand\n"));
    assert_eq!(after.size_bytes, 24);
    assert_eq!((after.version, after.version_count), (1, 0));
    assert!(
        f.store().versions(&row.id).unwrap().is_empty(),
        "no version row"
    );
    assert!(!f.project_root().join("docs").join(".versions").exists());
    assert_untouched(&path, b"# Plan, revised by hand\n", mtime);
}

/// R37: a result file the user moved away reads as gone and is marked
/// missing; once it is back, the next read clears the mark.
#[test]
fn a_result_file_found_again_clears_its_missing_mark() {
    let f = Fixture::new();
    let (row, path, _) = f.result_file("notes/todo.md", b"- one\n");
    let aside = f.project_root().join("todo.md.aside");
    fs::rename(&path, &aside).unwrap();

    let err = f.store().resolve_content(&row.id, None).unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<ArtifactError>(),
            Some(ArtifactError::Gone { .. })
        ),
        "{err:#}"
    );
    assert!(f.store().get(&row.id, OWNER).unwrap().unwrap().missing());

    fs::rename(&aside, &path).unwrap();
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(f.store().resolve_content(&row.id, None).unwrap(), path);
    assert!(
        !f.store().get(&row.id, OWNER).unwrap().unwrap().missing(),
        "found again"
    );
    assert_untouched(&path, b"- one\n", mtime);
}

/// R37: a result file has no older version to read or diff. (Passes before
/// this task too: it pins R37's "no History or Diff" at the store.)
#[test]
fn a_result_file_has_no_older_version_to_read_or_diff() {
    let f = Fixture::new();
    let (row, _, _) = f.result_file("src/lib.rs", b"pub fn a() {}\n");
    assert!(f.store().versions(&row.id).unwrap().is_empty());
    for err in [
        f.store().resolve_content(&row.id, Some(2)).unwrap_err(),
        f.store().diff_paths(&row.id, 1, 2).unwrap_err(),
    ] {
        assert!(
            matches!(
                err.downcast_ref::<ArtifactError>(),
                Some(ArtifactError::VersionNotFound { version: 2, .. })
            ),
            "{err:#}"
        );
    }
}

/// R37 with R31's rule: a result file replaced by a link out of the project
/// is neither hashed into the row nor read.
#[cfg(unix)]
#[test]
fn a_result_file_replaced_by_a_link_out_of_the_project_is_not_read() {
    let f = Fixture::new();
    let (row, path, _) = f.result_file("docs/plan.md", b"# Plan\n");
    let outside = tempdir().unwrap();
    let secret = outside.path().join("secret.txt");
    fs::write(&secret, b"not the project's\n").unwrap();
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&secret, &path).unwrap();

    let err = f.store().resolve_content(&row.id, None).unwrap_err();

    assert!(format!("{err:#}").contains("escapes"), "{err:#}");
    let after = f.store().get(&row.id, OWNER).unwrap().unwrap();
    assert_eq!(
        after.sha256, row.sha256,
        "nothing outside the project was hashed in"
    );
    assert!(f.store().versions(&row.id).unwrap().is_empty());
    assert_eq!(fs::read(&secret).unwrap(), b"not the project's\n");
}

/// The lead's ruling R-T18 (R37 with ST-23): a result file of a project whose
/// folder moved with a link left at its old root is not read, refreshed or
/// served through that link — the project is missing, and the file reads as
/// gone, marked as an absent file is. Once the folder is back at its root the
/// next read finds the file again and clears the mark.
#[cfg(unix)]
#[test]
fn a_result_file_of_a_project_moved_behind_a_link_is_not_read_through_its_old_root() {
    let f = Fixture::new();
    let (row, path, _) = f.result_file("docs/plan.md", b"# Plan\n");
    // The root as it is recorded, taken before the move: `project_root()`
    // canonicalizes, which after the move would answer the moved folder.
    let root = f.project_root();
    let aside = tempdir().unwrap();
    let moved = aside.path().join("app-moved");
    fs::rename(&root, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &root).unwrap();
    // The user edits the file where it is now.
    fs::write(
        moved.join("docs").join("plan.md"),
        b"# Plan, moved and edited\n",
    )
    .unwrap();
    assert!(
        path.is_file(),
        "the old path still opens the file, through the link"
    );

    let err = f.store().resolve_content(&row.id, None).unwrap_err();

    assert!(
        matches!(
            err.downcast_ref::<ArtifactError>(),
            Some(ArtifactError::Gone { .. })
        ),
        "{err:#}"
    );
    let after = f.store().get(&row.id, OWNER).unwrap().unwrap();
    assert_eq!(
        after.sha256, row.sha256,
        "nothing read through the link was hashed in"
    );
    assert_eq!(after.size_bytes, row.size_bytes);
    assert!(after.missing(), "it reads as gone");

    // The folder comes back to its root: found again.
    fs::remove_file(&root).unwrap();
    fs::rename(&moved, &root).unwrap();
    assert_eq!(f.store().resolve_content(&row.id, None).unwrap(), path);
    let back = f.store().get(&row.id, OWNER).unwrap().unwrap();
    assert!(!back.missing(), "found again");
    assert_eq!(back.sha256, sha256_hex(b"# Plan, moved and edited\n"));
}
