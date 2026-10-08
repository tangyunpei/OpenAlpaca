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
