use super::*;
use crate::store::tests::HomeStoreGuard;
use tempfile::{TempDir, tempdir};

/// A temp home store (through the crate's guard), a sibling `work/` where the
/// projects live, and a temp database. Nothing here touches a real store.
struct World {
    _tmp: TempDir,
    _env: HomeStoreGuard,
    home: PathBuf,
    work: PathBuf,
    db: Database,
}

impl World {
    fn new() -> Self {
        let tmp = tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        let work = root.join("work");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        let env = HomeStoreGuard::set(&home);
        let db = Database::open(&root.join("db").join("test.db")).unwrap();
        Self {
            _tmp: tmp,
            _env: env,
            home,
            work,
            db,
        }
    }

    fn dir(&self, name: &str) -> PathBuf {
        let dir = self.work.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn project(&self, name: &str) -> Project {
        projects::register(&self.dir(name)).unwrap()
    }

    /// One row in each of the five path columns, and a by-product whose bytes
    /// are in the project's folder.
    fn rows_of(&self, p: &Project) {
        let root = p.root_string();
        let bytes = p.dir.join("artifacts").join("loose").join("01-notes.md");
        std::fs::create_dir_all(bytes.parent().unwrap()).unwrap();
        std::fs::write(&bytes, "notes\n").unwrap();
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO session (id, lane_key, source, workspace_id)
                     VALUES ('s-1', 'u:gui', 'gui', ?1)",
                    rusqlite::params![root],
                )?;
                conn.execute(
                    "INSERT INTO task (id, title, created_by, source_lane, workspace_id, status)
                     VALUES ('t-1', 't', 'u', 'u:gui', ?1, 'completed')",
                    rusqlite::params![root],
                )?;
                conn.execute(
                    "INSERT INTO conversation_messages
                         (lane_key, role, content, session_id, workspace_root)
                     VALUES ('u:gui', 'user', 'hi', 's-1', ?1)",
                    rusqlite::params![root],
                )?;
                conn.execute(
                    "INSERT INTO lane_followups
                         (lane_key, kind, content, principal_json, workspace_path)
                     VALUES ('u:gui', 'followup', 'next', '{}', ?1)",
                    rusqlite::params![root],
                )?;
                conn.execute(
                    "INSERT INTO file_assets (id, owner_id, sha256, filename, mime_type,
                         size_bytes, storage_path, origin, project_root, rel_path)
                     VALUES ('a-1', 'owner-1', 'sha', 'notes.md', 'text/markdown', 6, ?1,
                             'produced', ?2, 'loose/01-notes.md')",
                    rusqlite::params![bytes.to_string_lossy(), root],
                )?;
                Ok(())
            })
            .unwrap();
    }

    /// How many of the five path columns name `root`, summed.
    fn named(&self, root: &Path) -> i64 {
        let root = root.to_string_lossy().into_owned();
        self.db
            .with_connection(|conn| {
                Ok(conn.query_row(
                    "SELECT (SELECT COUNT(*) FROM session WHERE workspace_id = ?1)
                          + (SELECT COUNT(*) FROM task WHERE workspace_id = ?1)
                          + (SELECT COUNT(*) FROM conversation_messages WHERE workspace_root = ?1)
                          + (SELECT COUNT(*) FROM lane_followups WHERE workspace_path = ?1)
                          + (SELECT COUNT(*) FROM file_assets WHERE project_root = ?1)",
                    rusqlite::params![root],
                    |r| r.get(0),
                )?)
            })
            .unwrap()
    }

    fn storage_path(&self) -> String {
        self.db
            .with_connection(|conn| {
                Ok(conn.query_row(
                    "SELECT storage_path FROM file_assets WHERE id = 'a-1'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap()
    }
}

fn code(err: &anyhow::Error) -> Option<&'static str> {
    err.downcast_ref::<ProjectError>().map(ProjectError::code)
}

#[test]
fn a_re_point_renames_the_folder_and_moves_every_row() {
    let w = World::new();
    let before = w.project("app");
    w.rows_of(&before);
    let new = w.dir("app-moved");

    let done = repoint(&w.db, &before.root, &new).unwrap();

    assert_eq!(done.before, before);
    assert_eq!(
        done.after.id, before.id,
        "the id is the project's for good (ST-13)"
    );
    assert_eq!(done.after.root, new);
    assert_eq!(done.after.name, projects::folder_name(&new));
    assert_eq!(
        (
            done.after.previous_root.clone(),
            done.after.previous_name.clone()
        ),
        (None, None),
        "step 4 cleared the marker"
    );
    assert_eq!(
        done.rows,
        RebaseCounts {
            file_assets: 1,
            sessions: 1,
            tasks: 1,
            messages: 1,
            followups: 1,
        }
    );
    assert!(!before.dir.exists(), "the old folder was renamed");
    assert_eq!(projects::lookup(&new).unwrap(), Some(done.after.clone()));
    assert_eq!(projects::lookup(&before.root).unwrap(), None);
    assert_eq!(w.named(&before.root), 0);
    assert_eq!(w.named(&new), 5);
    let moved = w.storage_path();
    assert!(
        moved.starts_with(done.after.dir.to_string_lossy().as_ref()),
        "{moved}"
    );
    assert_eq!(
        std::fs::read_to_string(&moved).unwrap(),
        "notes\n",
        "the bytes are where the row says"
    );
}

#[test]
fn a_target_that_is_not_an_existing_directory_is_refused_and_nothing_changes() {
    let w = World::new();
    let before = w.project("app");
    w.rows_of(&before);
    let file = w.work.join("a-file");
    std::fs::write(&file, "x").unwrap();

    for target in [w.work.join("mistyped"), file, PathBuf::from("relative/app")] {
        let err = repoint(&w.db, &before.root, &target).unwrap_err();
        assert_eq!(
            code(&err),
            Some("WORKSPACE_NOT_A_DIRECTORY"),
            "{target:?}: {err:#}"
        );
    }
    assert!(
        !w.work.join("mistyped").exists(),
        "a mistyped path is not created"
    );
    assert_eq!(
        projects::lookup(&before.root).unwrap(),
        Some(before.clone())
    );
    assert_eq!(w.named(&before.root), 5);
}

#[test]
fn a_target_that_is_a_project_or_that_rows_name_is_refused_naming_both_ways_out() {
    let w = World::new();
    let before = w.project("app");
    let other = w.project("other");

    let err = repoint(&w.db, &before.root, &other.root).unwrap_err();
    assert_eq!(code(&err), Some("WORKSPACE_EXISTS"));
    let message = err.to_string();
    assert!(
        message.contains("delete that project's data first")
            && message.contains("re-point onto another path"),
        "ST-20: both ways out are named: {message}"
    );

    let named = w.dir("named-by-rows");
    w.db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO session (id, lane_key, source, workspace_id)
             VALUES ('s-x', 'u:gui', 'gui', ?1)",
            rusqlite::params![named.to_string_lossy()],
        )?;
        Ok(())
    })
    .unwrap();
    let err = repoint(&w.db, &before.root, &named).unwrap_err();
    assert_eq!(code(&err), Some("WORKSPACE_EXISTS"));
    assert_eq!(projects::lookup(&before.root).unwrap(), Some(before));
}

#[test]
fn the_home_store_and_the_projects_own_path_are_refused() {
    let w = World::new();
    let before = w.project("app");
    // The home store itself, and a folder that contains it.
    for target in [w.home.clone(), w.home.parent().unwrap().to_path_buf()] {
        let err = repoint(&w.db, &before.root, &target).unwrap_err();
        assert_eq!(code(&err), Some("WORKSPACE_IS_HOME"), "{target:?}");
    }
    let err = repoint(&w.db, &before.root, &before.root).unwrap_err();
    assert_eq!(code(&err), Some("INVALID_PATH"));
}

#[test]
fn a_path_no_registered_project_owns_is_refused_and_nothing_is_registered() {
    let w = World::new();
    let stray = w.dir("stray");
    let new = w.dir("new");

    let err = repoint(&w.db, &stray, &new).unwrap_err();

    assert_eq!(code(&err), Some("WORKSPACE_NOT_FOUND"));
    assert!(!w.home.join(projects::PROJECTS_DIR).exists());
}

/// R24: a re-point stopped after any of its four steps — a crash, here the
/// checkpoint — is finished by the next recovery, and a second recovery has
/// nothing to do. After step 4 there is nothing left to finish.
#[test]
fn a_re_point_stopped_after_any_step_is_finished_by_the_next_recovery() {
    for step in [
        RepointStep::Marked,
        RepointStep::RowsRewritten,
        RepointStep::Renamed,
        RepointStep::Cleared,
    ] {
        let w = World::new();
        let before = w.project("app");
        w.rows_of(&before);
        let new = w.dir("app-moved");

        let err = repoint_with(&w.db, &before.root, &new, &mut |at| at != step).unwrap_err();
        assert_eq!(
            err.downcast_ref::<RepointInterrupted>(),
            Some(&RepointInterrupted(step))
        );

        let report = recover_repoints(&w.db);
        let finished = if step == RepointStep::Cleared {
            Vec::new()
        } else {
            vec![new.clone()]
        };
        assert_eq!(
            report,
            RecoveryReport {
                finished,
                failed: Vec::new(),
            },
            "{step:?}"
        );

        let after = projects::lookup(&new)
            .unwrap()
            .unwrap_or_else(|| panic!("{step:?}: the project is found at its new path"));
        assert_eq!(after.id, before.id);
        assert_eq!(after.name, projects::folder_name(&new));
        assert_eq!(
            (after.previous_root.clone(), after.previous_name.clone()),
            (None, None)
        );
        assert!(!before.dir.exists(), "{step:?}");
        assert_eq!(w.named(&new), 5, "{step:?}");
        assert!(
            w.storage_path()
                .starts_with(after.dir.to_string_lossy().as_ref())
        );
        assert_eq!(
            recover_repoints(&w.db),
            RecoveryReport::default(),
            "{step:?}: a second boot finds nothing to do"
        );
    }
}

/// R24 (Review Focus #3): a recovery that fails is reported, keeps the marker
/// and is tried again — never half-applied. After a crash at step 1, the next
/// recovery cannot rename the folder (`projects/` is read-only): it names the
/// project by its new root and leaves `project.json` marked, and with the rows
/// already moved neither path finds the project. The recovery after that,
/// with the folder renameable again, finishes it; a third has nothing to do.
#[cfg(unix)]
#[test]
fn a_recovery_that_fails_is_reported_keeps_the_marker_and_the_next_one_finishes_it() {
    use std::os::unix::fs::PermissionsExt;

    /// `projects/` back to the mode `create_private_dir` gave it, panic or not.
    struct Writable(PathBuf);
    impl Drop for Writable {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
        }
    }

    let w = World::new();
    let before = w.project("app");
    w.rows_of(&before);
    let new = w.dir("app-moved");
    let err = repoint_with(&w.db, &before.root, &new, &mut |at| {
        at != RepointStep::Marked
    })
    .unwrap_err();
    assert_eq!(
        err.downcast_ref::<RepointInterrupted>(),
        Some(&RepointInterrupted(RepointStep::Marked))
    );

    let projects_dir = w.home.join(projects::PROJECTS_DIR);
    let writable = Writable(projects_dir.clone());
    std::fs::set_permissions(&projects_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    // A user who may rename in a read-only directory anyway (root) cannot
    // make step 3 fail this way.
    let probe = projects_dir.join(".probe");
    if std::fs::create_dir(&probe).is_ok() {
        let _ = std::fs::remove_dir(&probe);
        return;
    }

    let report = recover_repoints(&w.db);

    assert!(report.finished.is_empty(), "{report:?}");
    assert_eq!(report.failed.len(), 1, "{report:?}");
    let (root, reason) = &report.failed[0];
    assert_eq!(root, &new, "the failure names the project by its new root");
    assert!(
        reason.contains("could not rename"),
        "step 3 failed: {reason}"
    );
    let marked = projects::read_project_dir(&before.dir)
        .unwrap()
        .expect("the folder keeps its old name and a readable project.json");
    assert_eq!(marked.id, before.id);
    assert_eq!(marked.root, new);
    assert_eq!(
        marked.previous_root.as_deref(),
        Some(before.root.as_path()),
        "the marker survives for the next boot"
    );
    assert_eq!(marked.previous_name.as_deref(), Some(before.name.as_str()));
    assert_eq!(projects::lookup(&new).unwrap(), None);
    assert_eq!(projects::lookup(&before.root).unwrap(), None);
    assert_eq!(w.named(&new), 5, "step 2 ran: the rows name the new root");

    drop(writable);
    assert_eq!(
        recover_repoints(&w.db),
        RecoveryReport {
            finished: vec![new.clone()],
            failed: Vec::new(),
        },
        "the next boot tries again and finishes it"
    );
    let after = projects::lookup(&new)
        .unwrap()
        .expect("the project is found at its new path");
    assert_eq!(after.id, before.id);
    assert_eq!(
        (after.previous_root.clone(), after.previous_name.clone()),
        (None, None)
    );
    assert!(!before.dir.exists());
    assert_eq!(w.named(&new), 5);
    let moved = w.storage_path();
    assert!(
        moved.starts_with(after.dir.to_string_lossy().as_ref()),
        "{moved}"
    );
    assert_eq!(
        std::fs::read_to_string(&moved).unwrap(),
        "notes\n",
        "the bytes are where the row says"
    );
    assert_eq!(
        recover_repoints(&w.db),
        RecoveryReport::default(),
        "a third boot finds nothing to do"
    );
}

/// §6 step 2: a failure in the transaction clears the marker and reports;
/// nothing else changed, and the next boot has nothing to finish.
#[test]
fn a_failure_while_the_rows_are_rewritten_clears_the_marker_and_changes_nothing() {
    let w = World::new();
    let before = w.project("app");
    w.rows_of(&before);
    let new = w.dir("app-moved");
    w.db.with_connection(|conn| {
        conn.execute_batch(
            "CREATE TRIGGER refuse_repoint BEFORE UPDATE OF workspace_id ON task
             BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )?;
        Ok(())
    })
    .unwrap();

    let err = repoint(&w.db, &before.root, &new).unwrap_err();

    assert!(format!("{err:#}").contains("injected failure"), "{err:#}");
    assert_eq!(
        projects::lookup(&before.root).unwrap(),
        Some(before.clone()),
        "the file names the old root again, with no marker"
    );
    assert_eq!(projects::lookup(&new).unwrap(), None);
    assert_eq!(w.named(&before.root), 5, "the transaction rolled back");
    assert_eq!(recover_repoints(&w.db), RecoveryReport::default());
}

/// R6: a new path whose plain name is the folder's own (`a_b` and `a-b` both
/// map to `…-a-b`) keeps the folder — nothing to rename.
#[test]
fn a_new_path_whose_plain_name_is_the_folders_own_keeps_the_folder() {
    let w = World::new();
    let before = w.project("a_b");
    let new = w.dir("a-b");
    assert_eq!(projects::folder_name(&new), before.name);

    let done = repoint(&w.db, &before.root, &new).unwrap();

    assert_eq!(done.after.name, before.name);
    assert_eq!(done.after.dir, before.dir);
    assert_eq!(done.after.root, new);
}

/// A re-point whose rename failed (§6 step 3) leaves the project's folder
/// under its old name, recording the new root with the marker, so neither
/// path finds the project until the next start finishes it (R24). Meanwhile
/// a turn in the new root — a git repository — must not register a second
/// project there: the moved rows already address the folder name the
/// re-point is taking, and a second project in it would leave the rename
/// failing at every start. Registering is refused and creates nothing; once
/// the recovery has run, the root is the one project it was.
#[test]
fn a_root_a_re_point_has_not_finished_moving_onto_is_not_registered_again() {
    let w = World::new();
    let before = w.project("app");
    w.rows_of(&before);
    let new = w.dir("app-moved");
    std::fs::create_dir_all(new.join(".git")).unwrap();
    // The disk as a live re-point whose step 3 failed leaves it: marked,
    // rows moved, folder not renamed.
    repoint_with(&w.db, &before.root, &new, &mut |at| {
        at != RepointStep::RowsRewritten
    })
    .unwrap_err();
    assert_eq!(projects::lookup(&new).unwrap(), None);

    for err in [
        projects::register(&new).unwrap_err(),
        projects::resolve_and_register(&new).unwrap_err(),
    ] {
        let message = format!("{err:#}");
        assert!(
            message.contains("re-point") && message.contains("next start"),
            "the refusal names the unfinished re-point and its remedy: {message}"
        );
    }
    let folders = w.home.join(projects::PROJECTS_DIR);
    assert!(!folders.join(projects::folder_name(&new)).exists());
    assert!(!folders.join(projects::hashed_folder_name(&new)).exists());
    assert_eq!(projects::list().unwrap().len(), 1, "no second project");

    assert_eq!(
        recover_repoints(&w.db),
        RecoveryReport {
            finished: vec![new.clone()],
            failed: Vec::new(),
        }
    );
    let after = projects::register(&new).unwrap();
    assert_eq!(after.id, before.id, "the root is the one project it was");
    assert_eq!(after.name, projects::folder_name(&new));
    assert_eq!(w.named(&new), 5);
    assert!(
        w.storage_path()
            .starts_with(after.dir.to_string_lossy().as_ref())
    );
    assert_eq!(projects::list().unwrap().len(), 1);
}

/// A project whose recorded root no longer canonicalizes to itself — its
/// folder moved and a link left at the old path (on macOS a case-only rename
/// does the same) — while a second project is registered where the link
/// leads. A re-point naming the first by its recorded root re-points the
/// first; the second stays where it is (Stage 1 review R1).
#[cfg(unix)]
#[test]
fn a_root_that_now_links_to_another_project_re_points_its_own_project() {
    let w = World::new();
    let a = w.project("a");
    let moved = w.work.join("b");
    std::fs::rename(&a.root, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &a.root).unwrap();
    let b = projects::register(&moved).unwrap();
    assert_ne!(a.id, b.id);
    let new = w.dir("elsewhere");

    let done = repoint(&w.db, &a.root, &new).unwrap();

    assert_eq!(done.before.id, a.id, "the project its root names");
    assert_eq!(done.after.id, a.id);
    assert_eq!(done.after.root, new);
    let still = projects::lookup(&moved)
        .unwrap()
        .expect("the second project is where it was");
    assert_eq!((still.id, still.dir), (b.id, b.dir));
}

/// `projects/` made searchable but not readable until this drops — then
/// `0700` again, so a failed assertion never leaves the tempdir unremovable.
#[cfg(unix)]
struct Unlistable(PathBuf);

#[cfg(unix)]
impl Unlistable {
    /// `None` when the mode does not stop a listing (the tests run as root).
    fn projects() -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;
        let dir = projects::projects_dir().unwrap();
        let guard = Self(dir.clone());
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o300)).unwrap();
        std::fs::read_dir(&dir).is_err().then_some(guard)
    }
}

#[cfg(unix)]
impl Drop for Unlistable {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
    }
}

/// The same state while `projects/` cannot be listed (residual X1): the
/// recorded root cannot be matched, and the re-point names no project rather
/// than resolving the root canonically — which is the second project's.
#[cfg(unix)]
#[test]
fn a_recorded_root_whose_listing_fails_names_no_project_rather_than_resolving_it() {
    let w = World::new();
    let a = w.project("a");
    let moved = w.work.join("b");
    std::fs::rename(&a.root, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &a.root).unwrap();
    let b = projects::register(&moved).unwrap();
    let Some(unlistable) = Unlistable::projects() else {
        return;
    };

    let found = registered(&a.root);
    drop(unlistable);

    let err = found.expect_err("a listing that fails names no project");
    assert!(
        format!("{err:#}").contains("the projects folder could not be read"),
        "{err:#}"
    );
    assert_eq!(
        projects::lookup(&moved).unwrap().map(|p| (p.id, p.dir)),
        Some((b.id, b.dir)),
        "the second project is where it was"
    );
}

/// ST-20 against a project whose re-point has not finished (Stage 1 review
/// R9). P1 — notes, no rows, as `Use` or a memory write leaves one — stopped
/// after step 2: its folder under the old name records the new root with the
/// marker, so `lookup` finds it under neither path until the next start. A
/// re-point of another project onto either of P1's paths is refused, as onto
/// a path that is already a project; then the recovery finishes P1, and the
/// other project stays where it was.
#[test]
fn a_re_point_onto_either_path_of_a_re_point_that_has_not_finished_is_refused() {
    let w = World::new();
    let p1 = w.project("notes");
    let new = w.dir("notes-moved");
    repoint_with(&w.db, &p1.root, &new, &mut |at| {
        at != RepointStep::RowsRewritten
    })
    .unwrap_err();
    assert_eq!(projects::lookup(&new).unwrap(), None);
    assert_eq!(projects::lookup(&p1.root).unwrap(), None);
    let p2 = w.project("other");

    for target in [&new, &p1.root] {
        let err = repoint(&w.db, &p2.root, target).unwrap_err();
        assert_eq!(
            code(&err),
            Some("WORKSPACE_EXISTS"),
            "{}: {err:#}",
            target.display()
        );
    }
    assert_eq!(
        projects::lookup(&p2.root).unwrap().map(|p| p.id),
        Some(p2.id),
        "the other project did not move"
    );

    assert_eq!(
        recover_repoints(&w.db),
        RecoveryReport {
            finished: vec![new.clone()],
            failed: Vec::new(),
        }
    );
    assert_eq!(projects::lookup(&new).unwrap().map(|p| p.id), Some(p1.id));
    assert_eq!(
        projects::lookup(&p2.root).unwrap().map(|p| p.id),
        Some(p2.id)
    );
}

/// The same refusal says what ends it (residual X7): the next start of the
/// daemon finishes that re-point. It does not send the caller to delete the
/// other project's data, which is refused until then too (R11). P1 here has
/// rows, which step 2 moved to its new root, so the rows name that path as
/// well: the unfinished re-point is still the sentence given.
#[test]
fn a_re_point_onto_a_path_of_an_unfinished_re_point_names_the_restart() {
    let w = World::new();
    let p1 = w.project("notes");
    w.rows_of(&p1);
    let new = w.dir("notes-moved");
    repoint_with(&w.db, &p1.root, &new, &mut |at| {
        at != RepointStep::RowsRewritten
    })
    .unwrap_err();
    let p2 = w.project("other");

    for target in [&new, &p1.root] {
        let err = repoint(&w.db, &p2.root, target).unwrap_err();
        let message = format!("{err:#}");
        assert_eq!(code(&err), Some("WORKSPACE_EXISTS"), "{message}");
        assert!(
            message.contains("the next start of the daemon finishes that re-point"),
            "{}: {message}",
            target.display()
        );
        assert!(
            !message.contains("delete that project's data"),
            "{}: {message}",
            target.display()
        );
    }
    assert_eq!(
        projects::lookup(&p2.root).unwrap().map(|p| p.id),
        Some(p2.id),
        "the other project did not move"
    );
}

impl World {
    /// A run under `root` in `status`, as a dispatch records it.
    fn run(&self, id: &str, root: &Path, status: &str) {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO task (id, title, created_by, source_lane, workspace_id, status)
                     VALUES (?1, ?1, 'u', 'u:gui', ?2, ?3)",
                    rusqlite::params![id, root.to_string_lossy(), status],
                )?;
                Ok(())
            })
            .unwrap();
    }
}

/// §6: refused while a run is in flight. The route counts the runs before it
/// waits for the project's memory slot; a run dispatched meanwhile has
/// already resolved the project by its old root, so step 2's transaction
/// counts them again — `queued` included — and refuses: nothing moves and
/// the marker is cleared, as for any failure of step 2.
#[test]
fn a_run_in_flight_refuses_the_re_point_inside_its_transaction_and_nothing_moves() {
    for status in ["queued", "running", "paused"] {
        let w = World::new();
        let before = w.project("app");
        w.rows_of(&before);
        w.run("t-live", &before.root, status);
        let new = w.dir("app-moved");

        let err = repoint(&w.db, &before.root, &new).unwrap_err();

        assert_eq!(
            err.chain()
                .find_map(|cause| cause.downcast_ref::<RunsInFlight>()),
            Some(&RunsInFlight {
                root: before.root_string(),
                runs: 1,
            }),
            "{status}: {err:#}"
        );
        assert_eq!(
            projects::lookup(&before.root).unwrap(),
            Some(before.clone()),
            "{status}: the file names the old root again, with no marker"
        );
        assert_eq!(projects::lookup(&new).unwrap(), None, "{status}");
        assert_eq!(w.named(&before.root), 6, "{status}: no row moved");
        assert!(before.dir.is_dir(), "{status}: the folder was not renamed");
    }
}

/// R24's recovery runs before the boot sweeps the previous process's runs,
/// which still read `running` then: they must not hold a re-point that
/// already moved its rows (or is about to) back for ever.
#[test]
fn a_recovery_finishes_a_re_point_whose_old_root_still_reads_a_run_in_flight() {
    let w = World::new();
    let before = w.project("app");
    w.rows_of(&before);
    let new = w.dir("app-moved");
    repoint_with(&w.db, &before.root, &new, &mut |at| {
        at != RepointStep::Marked
    })
    .unwrap_err();
    w.run("t-dead", &before.root, "running");

    assert_eq!(
        recover_repoints(&w.db),
        RecoveryReport {
            finished: vec![new.clone()],
            failed: Vec::new(),
        }
    );
    assert_eq!(
        projects::lookup(&new).unwrap().map(|p| p.id),
        Some(before.id)
    );
    assert_eq!(w.named(&new), 6, "the dead run's row moved with the rest");
}

/// ST-23 on a case-insensitive volume (macOS's default): a project whose
/// folder was renamed by case only reads missing, and re-pointing it onto the
/// new spelling renames its folder under `projects/` to the new case on disk,
/// rewrites its five path columns and its by-product's address, and leaves
/// one project, present.
#[cfg(unix)]
#[test]
fn a_case_only_rename_reads_missing_and_is_re_pointed_to_the_new_case() {
    let w = World::new();
    if !crate::store::tests::volume_ignores_case(&w.work) {
        eprintln!(
            "skipped: {} is case-sensitive, so no folder can be renamed by case only",
            w.work.display()
        );
        return;
    }
    let p = w.project("App");
    w.rows_of(&p);
    let renamed = w.work.join("app");
    std::fs::rename(&p.root, &renamed).unwrap();
    assert_eq!(
        p.root.canonicalize().unwrap(),
        renamed,
        "ST-23's premise: the volume answers the spelling on disk"
    );
    let listed = projects::list().unwrap();
    assert_eq!(listed.len(), 1);
    assert!(
        listed[0].missing,
        "ST-23: its root no longer canonicalizes to itself"
    );

    let done = repoint(&w.db, &p.root, &renamed).unwrap();

    assert_eq!(done.after.id, p.id);
    assert_eq!(done.after.root, renamed);
    assert!(!done.after.missing);
    let name = projects::folder_name(&renamed);
    assert_eq!(done.after.name, name);
    let on_disk: Vec<String> = std::fs::read_dir(projects::projects_dir().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        on_disk,
        vec![name.clone()],
        "the folder takes the new case on disk"
    );
    assert_eq!(
        done.after.dir.file_name().and_then(|n| n.to_str()),
        Some(name.as_str())
    );
    assert_eq!(w.named(&renamed), 5);
    assert_eq!(w.named(&p.root), 0, "no row keeps the old spelling");
    assert!(
        w.storage_path()
            .starts_with(done.after.dir.to_string_lossy().as_ref())
    );
    assert_eq!(projects::list().unwrap().len(), 1);
}
