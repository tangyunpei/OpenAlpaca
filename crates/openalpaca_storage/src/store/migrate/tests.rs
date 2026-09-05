use super::*;
use crate::store::tests::HomeStoreGuard;
use std::collections::BTreeSet;
use tempfile::tempdir;

// ============================================================================
// Fixtures
// ============================================================================

fn touch(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

/// A legacy app dir with one of every ledger entry, plus a file the ledger does
/// not know about (which must be left alone).
fn populate_old(old: &Path) {
    touch(&old.join("openalpaca.db"), "db");
    touch(&old.join("openalpaca.db-wal"), "wal");
    touch(&old.join("openalpaca.db-shm"), "shm");
    touch(&old.join(".master_key"), "deadbeef");
    touch(
        &old.join("config").join("llm.toml"),
        "[providers.anthropic]",
    );
    touch(&old.join("config").join("daemon.toml"), "old-daemon");
    touch(
        &old.join("config").join("orchestrator").join("SOUL.md"),
        "soul",
    );
    touch(&old.join("plugins").join(".permissions.toml"), "approved");
    touch(
        &old.join("plugins").join("demo").join("plugin.toml"),
        "demo",
    );
    touch(
        &old.join("assets").join("ab").join("cd").join("abcd"),
        "bytes",
    );
    touch(&old.join("daemon.log"), "log line");
    touch(&old.join("discovery.json"), "{}");
    touch(&old.join("openalpacad.lock"), "");
    touch(&old.join("repl_history"), "unknown to the ledger");
}

fn assert_fully_moved(old: &Path, new: &Path) {
    let state = new.join("state");
    assert_eq!(
        fs::read_to_string(state.join("openalpaca.db")).unwrap(),
        "db"
    );
    assert_eq!(
        fs::read_to_string(state.join("openalpaca.db-wal")).unwrap(),
        "wal"
    );
    assert_eq!(
        fs::read_to_string(state.join("openalpaca.db-shm")).unwrap(),
        "shm"
    );
    assert_eq!(
        fs::read_to_string(state.join(".master_key")).unwrap(),
        "deadbeef"
    );
    assert_eq!(
        fs::read_to_string(new.join("config").join("llm.toml")).unwrap(),
        "[providers.anthropic]"
    );
    assert_eq!(
        fs::read_to_string(new.join("config").join("orchestrator").join("SOUL.md")).unwrap(),
        "soul"
    );
    assert_eq!(
        fs::read_to_string(new.join("plugins").join(".permissions.toml")).unwrap(),
        "approved"
    );
    assert_eq!(
        fs::read_to_string(new.join("plugins").join("demo").join("plugin.toml")).unwrap(),
        "demo"
    );
    assert_eq!(
        fs::read_to_string(state.join("assets").join("ab").join("cd").join("abcd")).unwrap(),
        "bytes"
    );
    assert_eq!(
        fs::read_to_string(state.join("logs").join("daemon.log")).unwrap(),
        "log line"
    );
    // Regenerated every boot — deleted, never moved.
    assert!(!old.join("discovery.json").exists());
    assert!(!old.join("openalpacad.lock").exists());
    assert!(!new.join("discovery.json").exists());
    assert!(!state.join("discovery.json").exists());
}

fn children(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .map(|it| {
            it.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

// ============================================================================
// Steps 1 & 5 — fresh install, already moved, disposal
// ============================================================================

#[test]
fn a_fresh_install_is_a_no_op() {
    let tmp = tempdir().unwrap();
    let old = tmp.path().join("legacy");
    let new = tmp.path().join("home");
    fs::create_dir_all(&new).unwrap();

    move_root(&old, &new).unwrap();

    assert!(!old.exists());
    assert!(
        !new.join("state").exists(),
        "the mover creates nothing when there is nothing to move"
    );
    assert!(children(&new).is_empty());
}

#[test]
fn an_identical_old_and_new_root_is_a_no_op() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    touch(&root.join("openalpaca.db"), "db");

    move_root(&root, &root).unwrap();

    assert_eq!(
        fs::read_to_string(root.join("openalpaca.db")).unwrap(),
        "db"
    );
    assert!(!root.join("state").exists());
}

#[test]
fn the_old_root_is_removed_when_the_ledger_empties_it() {
    let tmp = tempdir().unwrap();
    let old = tmp.path().join("legacy");
    let new = tmp.path().join("home");
    touch(&old.join("openalpaca.db"), "db");
    touch(&old.join("discovery.json"), "{}");

    move_root(&old, &new).unwrap();

    assert!(!old.exists(), "an emptied legacy root is removed");
    assert_eq!(
        fs::read_to_string(new.join("state").join("openalpaca.db")).unwrap(),
        "db"
    );
}

#[test]
fn unknown_entries_are_left_behind_and_the_old_root_survives() {
    let tmp = tempdir().unwrap();
    let old = tmp.path().join("legacy");
    let new = tmp.path().join("home");
    populate_old(&old);

    move_root(&old, &new).unwrap();

    assert_fully_moved(&old, &new);
    assert_eq!(
        children(&old),
        BTreeSet::from(["repl_history".to_string()]),
        "the store never deletes what it did not create"
    );
}

// ============================================================================
// Step 4 — idempotent resume
// ============================================================================

#[test]
fn a_kill_between_any_two_ledger_entries_resumes_cleanly() {
    let ledger_len = ledger(Path::new("/old"), Path::new("/new")).len();
    assert_eq!(
        ledger_len, 10,
        "the ledger changed — update the resume test"
    );

    for stop_after in 0..=ledger_len {
        let tmp = tempdir().unwrap();
        let old = tmp.path().join("legacy");
        let new = tmp.path().join("home");
        populate_old(&old);

        // Simulated kill: the first `stop_after` entries land, nothing else.
        move_root_inner(&old, &new, Some(stop_after)).unwrap();
        // Next boot: the mover re-runs from the top.
        move_root(&old, &new).unwrap();

        assert_fully_moved(&old, &new);
        assert_eq!(
            children(&old),
            BTreeSet::from(["repl_history".to_string()]),
            "resume after {stop_after} entries left the old root wrong"
        );
    }
}

#[test]
fn a_completed_move_is_re_runnable() {
    let tmp = tempdir().unwrap();
    let old = tmp.path().join("legacy");
    let new = tmp.path().join("home");
    populate_old(&old);

    move_root(&old, &new).unwrap();
    move_root(&old, &new).unwrap();
    move_root(&old, &new).unwrap();

    assert_fully_moved(&old, &new);
}

// ============================================================================
// Step 3 — WAL/SHM reunite with the DB before `Database::open`
// ============================================================================

#[test]
fn the_database_and_its_sidecars_reunite_before_open() {
    let tmp = tempdir().unwrap();
    let old = tmp.path().join("legacy");
    let new = tmp.path().join("home");
    fs::create_dir_all(&old).unwrap();

    // A real database with a row in it, closed cleanly, then given sidecars.
    let db_path = old.join("openalpaca.db");
    {
        let db = Database::open(&db_path).unwrap();
        insert_asset(&db, "asset-1", "/somewhere/ab/cd/abcd");
    }
    touch(&old.join("openalpaca.db-wal"), "");
    touch(&old.join("openalpaca.db-shm"), "");

    // Killed right after the WAL moved: the halves are split on disk.
    move_root_inner(&old, &new, Some(1)).unwrap();
    let state = new.join("state");
    assert!(state.join("openalpaca.db-wal").exists());
    assert!(!state.join("openalpaca.db").exists());
    assert!(old.join("openalpaca.db").exists());

    // Next boot: the mover completes before anything opens the database.
    move_root(&old, &new).unwrap();
    for name in ["openalpaca.db", "openalpaca.db-wal", "openalpaca.db-shm"] {
        assert!(state.join(name).exists(), "{name} did not land in state/");
    }

    let db = Database::open(&state.join("openalpaca.db")).unwrap();
    assert_eq!(asset_paths(&db), vec!["/somewhere/ab/cd/abcd".to_string()]);
}

// ============================================================================
// Step 3 — per-child merge
// ============================================================================

#[test]
fn a_gui_pre_created_config_dir_is_merged_child_by_child() {
    let tmp = tempdir().unwrap();
    let old = tmp.path().join("legacy");
    let new = tmp.path().join("home");
    populate_old(&old);

    // A rebuilt GUI creates home_root()/config before spawning the daemon, and
    // may already have seeded a file there.
    touch(&new.join("config").join("daemon.toml"), "gui-daemon");

    move_root(&old, &new).unwrap();

    // Absent at the destination → moved.
    assert_eq!(
        fs::read_to_string(new.join("config").join("llm.toml")).unwrap(),
        "[providers.anthropic]"
    );
    assert!(
        new.join("config")
            .join("orchestrator")
            .join("SOUL.md")
            .exists()
    );
    // Present at the destination → kept, and the legacy copy stays put.
    assert_eq!(
        fs::read_to_string(new.join("config").join("daemon.toml")).unwrap(),
        "gui-daemon"
    );
    assert_eq!(
        fs::read_to_string(old.join("config").join("daemon.toml")).unwrap(),
        "old-daemon"
    );
    assert_eq!(
        children(&old.join("config")),
        BTreeSet::from(["daemon.toml".to_string()])
    );
    // Everything else still landed.
    assert!(new.join("plugins").join(".permissions.toml").exists());
    assert!(new.join("state").join("openalpaca.db").exists());
}

// ============================================================================
// Step 2 — live-daemon guard
// ============================================================================

#[cfg(unix)]
#[test]
fn a_live_daemon_in_the_old_root_aborts_the_move() {
    let tmp = tempdir().unwrap();
    let old = tmp.path().join("legacy");
    let new = tmp.path().join("home");
    populate_old(&old);

    let _holder = lock_holder::LockHolder::spawn(&old.join("openalpacad.lock"));

    let err = move_root(&old, &new).unwrap_err();
    assert!(
        err.to_string().contains("still running"),
        "unexpected error: {err}"
    );
    assert!(
        !new.join("state").exists(),
        "the guard must abort before a single rename"
    );
    assert!(old.join("openalpaca.db").exists());
}

#[cfg(unix)]
mod lock_holder {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    /// A forked child holding a POSIX write lock on `path`.
    ///
    /// `file_lock` uses `fcntl(F_SETLK)`, whose locks are per-process: taking the
    /// lock on another thread of this process would succeed and prove nothing.
    /// The child only makes async-signal-safe calls, and is killed on drop.
    pub(super) struct LockHolder {
        pid: libc::pid_t,
    }

    impl LockHolder {
        pub(super) fn spawn(path: &Path) -> Self {
            let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();

            let mut fds = [0 as libc::c_int; 2];
            assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe failed");
            let (read_fd, write_fd) = (fds[0], fds[1]);

            let pid = unsafe { libc::fork() };
            assert!(pid >= 0, "fork failed");
            if pid == 0 {
                unsafe {
                    libc::close(read_fd);
                    let fd = libc::open(c_path.as_ptr(), libc::O_RDWR);
                    if fd < 0 {
                        libc::_exit(2);
                    }
                    let mut fl: libc::flock = std::mem::zeroed();
                    fl.l_type = libc::F_WRLCK as libc::c_short;
                    fl.l_whence = libc::SEEK_SET as libc::c_short;
                    fl.l_start = 0;
                    fl.l_len = 0;
                    if libc::fcntl(fd, libc::F_SETLK, &mut fl as *mut libc::flock) < 0 {
                        libc::_exit(3);
                    }
                    let ready = [1u8];
                    libc::write(write_fd, ready.as_ptr() as *const libc::c_void, 1);
                    // Bounded, so a leaked child cannot outlive the test run by much.
                    libc::sleep(60);
                    libc::_exit(0);
                }
            }

            unsafe { libc::close(write_fd) };
            let mut buf = [0u8; 1];
            let n = unsafe { libc::read(read_fd, buf.as_mut_ptr() as *mut libc::c_void, 1) };
            unsafe { libc::close(read_fd) };
            assert_eq!(n, 1, "the lock holder failed to take the lock");
            Self { pid }
        }
    }

    impl Drop for LockHolder {
        fn drop(&mut self) {
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
                let mut status: libc::c_int = 0;
                libc::waitpid(self.pid, &mut status, 0);
            }
        }
    }
}

// ============================================================================
// Step 6 — rebase_asset_paths
// ============================================================================

fn insert_asset(db: &Database, id: &str, storage_path: &str) {
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO file_assets (id, owner_id, sha256, filename, mime_type, size_bytes, storage_path)
             VALUES (?1, 'owner', ?1, 'f.bin', 'application/octet-stream', 3, ?2)",
            rusqlite::params![id, storage_path],
        )?;
        Ok(())
    })
    .unwrap();
}

fn asset_paths(db: &Database) -> Vec<String> {
    db.with_connection(|conn| {
        let mut stmt = conn.prepare("SELECT storage_path FROM file_assets ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .unwrap()
}

#[test]
fn rebase_rewrites_moved_asset_paths_once() {
    let tmp = tempdir().unwrap();
    let old = tmp.path().join("legacy");
    let new = tmp.path().join("home");
    let db = Database::open(&tmp.path().join("t.db")).unwrap();

    let moved = old.join("assets").join("ab").join("cd").join("abcd");
    insert_asset(&db, "a-moved", &moved.to_string_lossy());
    insert_asset(&db, "b-elsewhere", "/somewhere/else/blob");

    let changed = rebase_asset_paths_between(&db, &old, &new).unwrap();
    assert_eq!(changed, 1);
    assert_eq!(
        asset_paths(&db),
        vec![
            new.join("state")
                .join("assets")
                .join("ab")
                .join("cd")
                .join("abcd")
                .to_string_lossy()
                .into_owned(),
            "/somewhere/else/blob".to_string(),
        ]
    );

    // Second boot: zero rows.
    assert_eq!(rebase_asset_paths_between(&db, &old, &new).unwrap(), 0);
}

#[test]
fn rebase_is_anchored_at_the_prefix() {
    let tmp = tempdir().unwrap();
    // `_` is a LIKE wildcard and common in home directory names.
    let old = tmp.path().join("my_root");
    let new = tmp.path().join("home");
    let decoy = tmp.path().join("myXroot").join("assets").join("blob");
    let db = Database::open(&tmp.path().join("t.db")).unwrap();

    insert_asset(&db, "a-decoy", &decoy.to_string_lossy());

    assert_eq!(rebase_asset_paths_between(&db, &old, &new).unwrap(), 0);
    assert_eq!(asset_paths(&db), vec![decoy.to_string_lossy().into_owned()]);
}

#[test]
fn rebase_through_the_boot_entry_point_is_a_no_op_without_a_legacy_root() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(tmp.path());
    let db = Database::open(&tmp.path().join("t.db")).unwrap();
    insert_asset(&db, "a", "/somewhere/else/blob");

    // Exercises the boot wrapper (legacy root + home_root resolution).
    rebase_asset_paths(&db);

    assert_eq!(asset_paths(&db), vec!["/somewhere/else/blob".to_string()]);
}
