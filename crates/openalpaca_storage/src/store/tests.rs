use super::*;
use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard};
use tempfile::tempdir;

// ============================================================================
// Env harness
// ============================================================================

/// `home_root()` reads `OPENALPACA_HOME_STORE` on every call, so tests that set
/// it must not run concurrently with each other. Every test that can reach a
/// path accessor takes this guard and points the process at a temp dir first —
/// no test ever touches the real `~/.openalpaca`.
static ENV_LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct HomeStoreGuard {
    _lock: MutexGuard<'static, ()>,
    prev: Option<OsString>,
    /// `HOME` as it was, when [`HomeStoreGuard::for_user_home`] moved it.
    prev_home: Option<Option<OsString>>,
}

impl HomeStoreGuard {
    pub(crate) fn set(path: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os(HOME_STORE_ENV);
        // SAFETY: serialized by ENV_LOCK; every test that reads the variable
        // holds the same guard.
        unsafe { std::env::set_var(HOME_STORE_ENV, path) };
        Self {
            _lock: lock,
            prev,
            prev_home: None,
        }
    }

    /// Moves the user's home directory — `HOME`, which `directories::BaseDirs`
    /// reads, so `home_root()`'s fallback becomes `<home>/.openalpaca` — and
    /// sets `OPENALPACA_HOME_STORE` to `store`, or removes it for `None`. For
    /// the test-build guard of `store::projects::register`: its "unset" and
    /// "the real store" cases then name a temp directory, so even a guard that
    /// failed to fire could write nothing into the developer's store. Same
    /// lock, no second one. Panics, restoring both variables, if `BaseDirs`
    /// does not answer with `home`.
    pub(crate) fn for_user_home(home: &Path, store: Option<&Path>) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let guard = Self {
            _lock: lock,
            prev: std::env::var_os(HOME_STORE_ENV),
            prev_home: Some(std::env::var_os("HOME")),
        };
        // SAFETY: serialized by ENV_LOCK, which `guard` holds until it drops
        // and restores both variables.
        unsafe {
            std::env::set_var("HOME", home);
            match store {
                Some(store) => std::env::set_var(HOME_STORE_ENV, store),
                None => std::env::remove_var(HOME_STORE_ENV),
            }
        }
        let resolved = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf());
        assert_eq!(
            resolved.as_deref(),
            Some(home),
            "HOME did not move the user's home directory; refusing to run"
        );
        guard
    }
}

impl Drop for HomeStoreGuard {
    fn drop(&mut self) {
        // SAFETY: as above — still holding ENV_LOCK.
        if let Some(prev_home) = self.prev_home.take() {
            match prev_home {
                Some(v) => unsafe { std::env::set_var("HOME", v) },
                None => unsafe { std::env::remove_var("HOME") },
            }
        }
        match self.prev.take() {
            Some(v) => unsafe { std::env::set_var(HOME_STORE_ENV, v) },
            None => unsafe { std::env::remove_var(HOME_STORE_ENV) },
        }
    }
}

// ============================================================================
// Root resolution
// ============================================================================

#[test]
fn absolute_override_wins_over_home() {
    let resolved = resolve_home_root(
        Some(PathBuf::from("/tmp/oa-store")),
        Some(PathBuf::from("/Users/someone")),
    )
    .unwrap();
    assert_eq!(resolved, PathBuf::from("/tmp/oa-store"));
}

#[test]
fn relative_override_is_rejected() {
    let err = resolve_home_root(
        Some(PathBuf::from("relative/store")),
        Some(PathBuf::from("/Users/someone")),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("absolute"),
        "unexpected error: {err}"
    );
}

#[test]
fn empty_override_is_rejected() {
    assert!(
        resolve_home_root(Some(PathBuf::new()), Some(PathBuf::from("/Users/someone"))).is_err()
    );
}

#[test]
fn default_root_is_dot_openalpaca_under_home() {
    let resolved = resolve_home_root(None, Some(PathBuf::from("/Users/someone"))).unwrap();
    assert_eq!(resolved, PathBuf::from("/Users/someone/.openalpaca"));
}

#[test]
fn missing_home_is_an_error() {
    assert!(resolve_home_root(None, None).is_err());
}

#[test]
fn home_root_honours_the_env_override() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(tmp.path());
    assert_eq!(home_root().unwrap(), tmp.path());
}

/// One name for the daemon log, shared by the CLI that writes it and the
/// status route that reports it — and naming it must not create `state/logs`,
/// because `GET /v1/status` asks whether the file *exists*.
#[test]
fn the_daemon_log_is_named_once_and_creating_nothing() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(tmp.path());

    let log = daemon_log_path().unwrap();
    assert_eq!(log, tmp.path().join("state").join("logs").join("daemon.log"));
    assert!(
        !tmp.path().join("state").join("logs").exists(),
        "daemon_log_path() must not create the logs directory"
    );

    // …and it is the same file `logs_dir()` hands the writer.
    assert_eq!(log, logs_dir().unwrap().join("daemon.log"));
}

/// The former `paths.rs::test_paths_are_consistent`, re-targeted at `state_dir()`.
#[test]
fn test_paths_are_consistent() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(tmp.path());

    let state = state_dir().unwrap();
    let discovery = discovery_path().unwrap();
    let lock = lock_path().unwrap();
    let db = database_path().unwrap();
    let logs = logs_dir().unwrap();
    let backups = backups_dir().unwrap();
    // The memory search index's directory: regenerable machine state, so it
    // is under `state/` too — and a path query only, created by the index.
    let cache = cache_dir().unwrap();

    assert_eq!(state, tmp.path().join("state"));
    assert!(state.is_dir(), "state_dir() creates the directory");
    for p in [&discovery, &lock, &db, &logs, &backups, &cache] {
        assert!(p.starts_with(&state), "{} is not under state/", p.display());
    }
    assert!(discovery.ends_with("discovery.json"));
    assert!(lock.ends_with("openalpacad.lock"));
    assert!(db.ends_with("openalpaca.db"));
    assert!(logs.is_dir() && logs.ends_with("logs"));
    assert!(backups.is_dir() && backups.ends_with("backups"));
    assert_eq!(cache, state.join("cache"));
    assert!(
        !cache.exists(),
        "cache_dir() is a path query and creates nothing"
    );
    assert_eq!(master_key_dir().unwrap(), state);

    // The human's half of the root sits beside state/, not inside it.
    let plugins = plugins_dir().unwrap();
    let config = ensure_runtime_config_dir().unwrap();
    assert_eq!(plugins, tmp.path().join("plugins"));
    assert_eq!(config, runtime_config_dir().unwrap());
    assert_eq!(config, tmp.path().join("config"));
    assert!(plugins.is_dir() && config.is_dir());
}

#[test]
fn path_queries_do_not_create_the_store() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("home");
    let _guard = HomeStoreGuard::set(&root);

    // Reading discovery must not materialise a store — the CLI and GUI call it
    // just to ask whether a daemon is running.
    for path in [
        home_root().unwrap(),
        database_path().unwrap(),
        discovery_path().unwrap(),
        lock_path().unwrap(),
        runtime_config_dir().unwrap(),
    ] {
        assert!(
            path.starts_with(&root),
            "{} escaped the root",
            path.display()
        );
    }
    assert!(!root.exists(), "a path query created {}", root.display());
}

/// The one guarantee `open_home_database` exists for: a process that opens the
/// database before any daemon has run still gets a private `state/`, rather
/// than one SQLite made at the process umask beside `.master_key`.
#[cfg(unix)]
#[test]
fn open_home_database_creates_the_state_directory_at_0700() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempdir().unwrap();
    let root = tmp.path().join("home");
    let _guard = HomeStoreGuard::set(&root);
    assert!(!root.join("state").exists());

    let db = open_home_database().unwrap();
    drop(db);

    let mode = fs::metadata(root.join("state"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700, "state/ is {:o}", mode & 0o777);
    assert!(root.join("state").join("openalpaca.db").exists());
    assert_eq!(
        database_path().unwrap(),
        root.join("state").join("openalpaca.db")
    );
}

// ============================================================================
// ensure_store
// ============================================================================

/// The home README is the document a person reads to learn what each entry
/// keeps, and D2 gave uploads human-readable names. The daemon's asset sweep
/// deletes an upload that was never attached to a message once the grace period
/// passes — bytes and row — so no retention row may promise those files are kept
/// forever. Produced artifacts are genuinely never swept; only uploads are.
#[test]
fn the_home_readme_tells_the_truth_about_upload_retention() {
    let uploads = HOME_README
        .lines()
        .find(|line| line.starts_with("| `uploads/`"))
        .expect("the home README has an uploads/ row");
    assert!(
        !uploads.contains("never garbage-collected"),
        "the home README promises uploads survive, but the sweep deletes them: {uploads}"
    );
    assert!(
        uploads.contains("grace period"),
        "the home README must name the retention rule the sweep applies: {uploads}"
    );
    assert!(
        HOME_README
            .lines()
            .any(|line| line.starts_with("| `artifacts/`") && line.contains("never")),
        "the home README must keep saying produced artifacts are never swept"
    );
}

#[test]
fn ensure_store_seeds_the_home_root() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(&tmp.path().join("home"));

    let root = ensure_store(&StoreScope::Home).unwrap();
    assert_eq!(root, tmp.path().join("home"));

    let readme = fs::read_to_string(root.join("README.md")).unwrap();
    assert!(
        readme.contains("Retention class"),
        "README lacks the retention-class column"
    );
    assert!(readme.contains("openalpaca config reset --factory"));
    // Taken literally, "deleting `state/` is a factory reset" destroys the
    // embedding model and the master key; the verb deletes the database only.
    assert!(
        !readme.contains("is a factory reset"),
        "the README must not equate deleting state/ with a factory reset"
    );
    assert!(
        !root.join(".gitignore").exists(),
        "the home root carries no .gitignore"
    );

    assert_eq!(layout_version(&root).unwrap(), Some(LAYOUT_VERSION));
    let id = install_id(&root)
        .unwrap()
        .expect("home root carries an install id");
    assert_eq!(uuid::Uuid::parse_str(&id).unwrap().get_version_num(), 4);

    // Idempotent: the id is written once and never rewritten.
    ensure_store(&StoreScope::Home).unwrap();
    assert_eq!(install_id(&root).unwrap().as_deref(), Some(id.as_str()));
}

/// The memory redesign's rows (plan 3.7): the home README names the control
/// snapshots and the search index under `state/` and the notes under
/// `memory/`, and says nothing of an embedding model.
#[test]
fn the_home_readme_describes_markdown_memory() {
    for needle in [
        "| `state/memory-control/` |",
        "`memory-index-v1.sqlite`",
        "| `memory/` |",
        "`owners/<owner-key>/",
    ] {
        assert!(
            HOME_README.contains(needle),
            "the home README must contain {needle:?}"
        );
    }
    for stale in ["embedding", "fastembed", "~1 GB", "`memory/`, `skills/`"] {
        assert!(
            !HOME_README.contains(stale),
            "the home README must not say {stale:?}"
        );
    }
}

/// Storage design R1–R3: the home README lists `projects/` — one folder per
/// project, the user's like everything at the root but `state/` — and the
/// CLI's `repl_history` in `state/`. The two reserved kinds that never held
/// content are gone from it (`skills`: the live one is `config/skills/`;
/// `cache`: the live one is `state/cache/`), and `scratch/` alone stays
/// reserved until Stage 2. A deleted project's files that another
/// conversation still shows are kept under `kept/` in the root's
/// `artifacts/` and `uploads/`, and those two rows say so.
#[test]
fn the_home_readme_lists_projects_and_repl_history_and_only_live_kinds() {
    let row = |entry: &str| {
        let head = format!("| `{entry}` |");
        HOME_README
            .lines()
            .find(|line| line.starts_with(&head))
            .map(str::to_owned)
    };
    let projects = row("projects/").expect("the home README has a projects/ row");
    for needle in ["project.json", "artifacts/", "uploads/", "memory/", "yours"] {
        assert!(
            projects.contains(needle),
            "the projects/ row must name {needle:?}: {projects}"
        );
    }
    let state = row("state/").expect("the home README has a state/ row");
    assert!(state.contains("`repl_history`"), "{state}");
    for gone in ["skills/", "cache/", "scratch/`, `cache/"] {
        assert!(
            row(gone).is_none(),
            "the home README must not list {gone:?} as an entry"
        );
    }
    assert!(
        row("scratch/").is_some(),
        "scratch/ stays reserved until Stage 2"
    );
    for kept in ["artifacts/", "uploads/"] {
        let line = row(kept).unwrap_or_else(|| panic!("the home README has a {kept} row"));
        assert!(line.contains("`kept/`"), "{line}");
    }
}

/// Final review R27: the stores write nothing inside a project's own folder,
/// but on a turn with a project `file_write` still writes a plain file into
/// the daemon's starting directory whenever that is not the home store
/// (`tools/builtins/file_ops.rs`) — a daemon started by hand inside a
/// repository writes into it. So the `projects/` row may not say nothing is
/// ever written there; it names the one daemon that still does.
#[test]
fn the_home_readme_projects_row_names_the_write_a_daemon_started_elsewhere_still_makes() {
    let projects = HOME_README
        .lines()
        .find(|line| line.starts_with("| `projects/` |"))
        .expect("the home README has a projects/ row");
    assert!(
        !projects.contains("Nothing is ever written inside"),
        "the projects/ row must not promise that nothing is ever written in a project: {projects}"
    );
    for needle in ["`file_write`", "`openalpaca daemon start`"] {
        assert!(
            projects.contains(needle),
            "the projects/ row must name {needle:?}: {projects}"
        );
    }
}

/// The README is the user's the moment it exists: it is written only when
/// absent, so an edited text — even by one byte — and any text of their own
/// are left alone.
#[test]
fn ensure_store_leaves_an_edited_home_readme_alone() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(&tmp.path().join("home"));
    let root = ensure_store(&StoreScope::Home).unwrap();
    let readme = root.join("README.md");

    let edited = format!("{HOME_README}\nMy note: keep backups.\n");
    let trimmed = HOME_README.trim_end().to_owned();
    for mine in [edited, trimmed, "# my own notes\n".to_owned()] {
        fs::write(&readme, &mine).unwrap();
        ensure_store(&StoreScope::Home).unwrap();
        assert_eq!(fs::read_to_string(&readme).unwrap(), mine);
    }
}

/// ST-1: a project's store is its folder under the home store's
/// `projects/`, found by `projects::lookup`; `content_dir` keeps its shape
/// beneath it, and nothing is written into the project's own folder — no
/// README, no `.gitignore`, no `.layout` either (R3).
#[test]
fn a_project_store_is_its_folder_under_projects() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let _guard = HomeStoreGuard::set(&root.join("home"));
    let project = root.join("work").join("app");
    fs::create_dir_all(&project).unwrap();
    let registered = projects::register(&project).unwrap();
    let scope = StoreScope::Project(project.clone());

    let store = store_root(&scope).unwrap();
    assert_eq!(store, registered.dir);
    assert_eq!(
        store,
        root.join("home")
            .join(projects::PROJECTS_DIR)
            .join(projects::folder_name(&project))
    );
    assert_eq!(ensure_store(&scope).unwrap(), store);
    assert_eq!(
        content_dir(&scope, ContentKind::Uploads).unwrap(),
        store.join("uploads")
    );
    let mut entries: Vec<String> = fs::read_dir(&store)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        [projects::PROJECT_FILE, "uploads"],
        "project.json and content only (R3)"
    );
    assert_eq!(
        fs::read_dir(&project).unwrap().count(),
        0,
        "nothing in the project's own folder (ST-1)"
    );
}

/// R12: a root with no folder under `projects/` has no store. `store_root`,
/// `ensure_store` and `content_dir` answer the typed `NotRegistered` error,
/// which `projects::is_not_registered` recognises, and create nothing — not
/// under the home store, not in the folder.
#[test]
fn an_unregistered_project_root_has_no_store_and_nothing_is_created() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let _guard = HomeStoreGuard::set(&root.join("home"));
    let repo = root.join("work").join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let scope = StoreScope::Project(repo.clone());

    for err in [
        store_root(&scope).unwrap_err(),
        ensure_store(&scope).unwrap_err(),
        content_dir(&scope, ContentKind::Artifacts).unwrap_err(),
    ] {
        assert!(projects::is_not_registered(&err), "{err:#}");
        assert_eq!(
            err.downcast_ref::<projects::ProjectError>()
                .map(projects::ProjectError::code),
            Some("WORKSPACE_NOT_FOUND")
        );
    }
    assert!(!root.join("home").exists(), "nothing under the home store");
    assert_eq!(
        fs::read_dir(&repo).unwrap().count(),
        1,
        "the folder holds its .git and nothing else"
    );
}

#[test]
fn layout_version_reports_absence_and_rejects_garbage() {
    let tmp = tempdir().unwrap();
    assert_eq!(layout_version(tmp.path()).unwrap(), None);
    fs::write(tmp.path().join(".layout"), "not-a-number\n").unwrap();
    assert!(layout_version(tmp.path()).is_err());
}

/// Line 1 is repaired in place; nothing is added to a marker that exists, so
/// one with no install id keeps having none.
#[test]
fn a_malformed_layout_marker_is_repaired_not_appended_to() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(tmp.path());
    fs::write(tmp.path().join(".layout"), "not-a-number\n").unwrap();

    ensure_store(&StoreScope::Home).unwrap();

    assert_eq!(
        fs::read_to_string(tmp.path().join(".layout")).unwrap(),
        format!("{LAYOUT_VERSION}\n")
    );
    assert_eq!(install_id(tmp.path()).unwrap(), None);
}

#[test]
fn layout_lines_this_module_does_not_own_are_preserved() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(tmp.path());
    fs::write(
        tmp.path().join(".layout"),
        "garbage\ninstall_id=abc\nproject_id=def\n",
    )
    .unwrap();

    ensure_store(&StoreScope::Home).unwrap();

    assert_eq!(
        fs::read_to_string(tmp.path().join(".layout")).unwrap(),
        "1\ninstall_id=abc\nproject_id=def\n",
        "line 1 repaired, every other line kept, nothing added"
    );
    assert_eq!(install_id(tmp.path()).unwrap().as_deref(), Some("abc"));
}

#[test]
fn no_temp_file_survives_a_layout_write() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(tmp.path());
    ensure_store(&StoreScope::Home).unwrap();
    assert!(
        !tmp.path().join(".layout.tmp").exists(),
        ".layout is written through a temp file that must be renamed away"
    );
}

// ============================================================================
// Content stores
// ============================================================================

#[test]
fn content_dirs_have_the_same_shape_in_both_scopes() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(&tmp.path().join("home"));
    let project = tmp.path().join("proj");
    fs::create_dir_all(&project).unwrap();
    let project = project.canonicalize().unwrap();
    projects::register(&project).unwrap();
    let scope = StoreScope::Project(project);

    for kind in [
        ContentKind::Artifacts,
        ContentKind::Uploads,
        ContentKind::Sessions,
        ContentKind::Memory,
        ContentKind::Scratch,
    ] {
        let home = content_dir(&StoreScope::Home, kind).unwrap();
        let proj = content_dir(&scope, kind).unwrap();
        assert_eq!(home, tmp.path().join("home").join(kind.dir_name()));
        assert_eq!(proj, store_root(&scope).unwrap().join(kind.dir_name()));
        assert!(home.is_dir() && proj.is_dir(), "content_dir creates on use");
    }

    assert_eq!(
        sessions_dir().unwrap(),
        content_dir(&StoreScope::Home, ContentKind::Sessions).unwrap()
    );
}

#[test]
fn a_relative_project_root_is_rejected() {
    assert!(store_root(&StoreScope::Project(PathBuf::from("relative/proj"))).is_err());
}

// ============================================================================
// Daemon log: rotation and tail (moved from the CLI's process manager, which
// was its only launcher; the GUI sidecar now shares it)
// ============================================================================

/// Below the threshold the log is left exactly as it is: rotating a small
/// file would throw away the only copy of a short run's output.
#[test]
fn a_log_under_the_cap_is_not_rotated() {
    let root = tempdir().unwrap();
    let log = root.path().join("daemon.log");
    fs::write(&log, b"one short run\n").unwrap();

    rotate_log(&log, DAEMON_LOG_MAX_BYTES, DAEMON_LOG_KEEP).expect("rotation should succeed");

    assert_eq!(fs::read(&log).unwrap(), b"one short run\n");
    assert!(!root.path().join("daemon.log.1").exists());
}

/// A missing log is the ordinary first start, not an error.
#[test]
fn a_missing_log_is_not_an_error() {
    let root = tempdir().unwrap();
    rotate_log(
        &root.path().join("daemon.log"),
        DAEMON_LOG_MAX_BYTES,
        DAEMON_LOG_KEEP,
    )
    .expect("a first start rotates nothing");
}

/// The real 16 MB threshold, exercised with a sparse file so the test does
/// not write 16 MB: past it, `daemon.log` becomes `daemon.log.1` and the live
/// name is free for a fresh file.
#[test]
fn a_log_over_sixteen_megabytes_is_rotated_to_dot_one() {
    let root = tempdir().unwrap();
    let log = root.path().join("daemon.log");
    fs::File::create(&log)
        .unwrap()
        .set_len(DAEMON_LOG_MAX_BYTES + 1)
        .unwrap();

    rotate_log(&log, DAEMON_LOG_MAX_BYTES, DAEMON_LOG_KEEP).expect("rotation should succeed");

    assert!(!log.exists(), "the live name is free after a rotation");
    let rotated = root.path().join("daemon.log.1");
    assert_eq!(
        fs::metadata(&rotated).unwrap().len(),
        DAEMON_LOG_MAX_BYTES + 1
    );
}

/// Keep three: every generation shifts down one and the fourth is dropped, so
/// the log costs at most four files however long the daemon runs.
#[test]
fn rotation_keeps_three_generations_and_drops_the_oldest() {
    let root = tempdir().unwrap();
    let log = root.path().join("daemon.log");
    for (name, body) in [
        ("daemon.log", "live"),
        ("daemon.log.1", "gen1"),
        ("daemon.log.2", "gen2"),
        ("daemon.log.3", "gen3"),
    ] {
        fs::write(root.path().join(name), body).unwrap();
    }

    // A tiny cap: the keep rule is what is under test, not the threshold.
    rotate_log(&log, 2, DAEMON_LOG_KEEP).expect("rotation should succeed");

    assert!(!log.exists());
    let read = |name: &str| fs::read_to_string(root.path().join(name)).unwrap();
    assert_eq!(read("daemon.log.1"), "live");
    assert_eq!(read("daemon.log.2"), "gen1");
    assert_eq!(read("daemon.log.3"), "gen2");
    assert!(
        !root.path().join("daemon.log.4").exists(),
        "the fourth generation is dropped, never accumulated"
    );

    // And again, to prove the shift is not a one-off.
    fs::write(&log, "live-2").unwrap();
    rotate_log(&log, 2, DAEMON_LOG_KEEP).expect("rotation should succeed");
    assert_eq!(read("daemon.log.1"), "live-2");
    assert_eq!(read("daemon.log.2"), "live");
    assert_eq!(read("daemon.log.3"), "gen1");
    assert!(!root.path().join("daemon.log.4").exists());
}

/// The shared entry point both launchers call resolves the log through the
/// store root — the sandboxed one here — and, like the name it rotates,
/// creates nothing when there is nothing to rotate.
#[test]
fn rotate_daemon_log_rotates_the_store_log_and_creates_nothing_on_a_fresh_root() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(tmp.path());

    rotate_daemon_log().expect("a fresh root rotates nothing");
    assert!(
        !tmp.path().join("state").exists(),
        "rotating a log that is not there must not create the store"
    );

    let log = logs_dir().unwrap().join("daemon.log");
    fs::File::create(&log)
        .unwrap()
        .set_len(DAEMON_LOG_MAX_BYTES + 1)
        .unwrap();
    rotate_daemon_log().expect("rotation should succeed");
    assert!(!log.exists());
    assert!(logs_dir().unwrap().join("daemon.log.1").exists());
}

#[test]
fn log_tail_with_fewer_lines_than_asked_is_all_of_them() {
    assert_eq!(log_tail("one\ntwo\n", 20), "one\ntwo");
}

#[test]
fn log_tail_keeps_only_the_newest_lines_and_ignores_the_trailing_newline() {
    let text = "a\nb\nc\nd\ne\n";
    assert_eq!(log_tail(text, 2), "d\ne");
    assert_eq!(log_tail("a\nb\nc\n\n\n", 2), "b\nc", "trailing blanks are not lines");
    assert_eq!(log_tail("a\r\nb\r\n", 5), "a\nb", "CRLF ends a line like LF");
}

/// Verbatim is the rule: a blank line *inside* the window is part of what was
/// written — a message may be paragraphs — while the blank lines the window
/// would open with are not worth a line of the panel.
#[test]
fn log_tail_keeps_inner_blank_lines_and_drops_the_leading_ones() {
    let refusal = "FATAL: first paragraph\n\n  Older install:  /x\n\nTo discard it, rename /x.\n";
    assert_eq!(
        log_tail(refusal, 20),
        "FATAL: first paragraph\n\n  Older install:  /x\n\nTo discard it, rename /x."
    );
    // The window of three opens on a blank line, which is dropped rather than
    // shown as an empty first row.
    assert_eq!(
        log_tail(refusal, 3),
        "  Older install:  /x\n\nTo discard it, rename /x."
    );
}

#[test]
fn log_tail_of_empty_or_blank_input_is_empty() {
    assert_eq!(log_tail("", 20), "");
    assert_eq!(log_tail("\n\n  \n", 20), "");
    assert_eq!(log_tail("one\n", 0), "");
}

/// `tracing` writes colour even into a file; the panel shows text.
#[test]
fn log_tail_removes_ansi_colour_escapes_and_nothing_else() {
    let line = "\u{1b}[2m2026-09-22T10:00:00Z\u{1b}[0m \u{1b}[31mERROR\u{1b}[0m \u{1b}[2mopenalpacad\u{1b}[0m\u{1b}[2m:\u{1b}[0m FATAL: [brackets] stay";
    assert_eq!(
        log_tail(line, 1),
        "2026-09-22T10:00:00Z ERROR openalpacad: FATAL: [brackets] stay"
    );
}

#[test]
fn read_log_tail_of_a_missing_file_is_empty() {
    let root = tempdir().unwrap();
    assert_eq!(
        read_log_tail(&root.path().join("daemon.log"), 0, 20).unwrap(),
        ""
    );
}

/// A run that wrote nothing reads as nothing — never as the previous run's
/// last words presented as this one's.
#[test]
fn read_log_tail_reads_only_what_the_run_appended() {
    let root = tempdir().unwrap();
    let log = root.path().join("daemon.log");
    fs::write(&log, "previous run: ERROR something old\n").unwrap();
    let from = fs::metadata(&log).unwrap().len();

    assert_eq!(read_log_tail(&log, from, 20).unwrap(), "");

    let mut file = fs::OpenOptions::new().append(true).open(&log).unwrap();
    std::io::Write::write_all(&mut file, b"this run: FATAL: why it stopped\n").unwrap();
    assert_eq!(
        read_log_tail(&log, from, 20).unwrap(),
        "this run: FATAL: why it stopped"
    );
    assert_eq!(
        read_log_tail(&log, 0, 20).unwrap(),
        "previous run: ERROR something old\nthis run: FATAL: why it stopped"
    );
    // A `from` past the end means the file was rotated under us: all of what
    // is there now is the run's.
    assert_eq!(
        read_log_tail(&log, u64::MAX, 1).unwrap(),
        "this run: FATAL: why it stopped"
    );
}

/// A large log costs one bounded read, and the fragment the window lands in
/// the middle of is dropped rather than shown cut mid-line.
#[test]
fn read_log_tail_of_a_large_log_never_shows_a_line_cut_in_half() {
    let root = tempdir().unwrap();
    let log = root.path().join("daemon.log");
    let line = format!("{}\n", "x".repeat(99));
    let body = line.repeat(2_000) + "the last line\n";
    fs::write(&log, &body).unwrap();
    assert!(body.len() as u64 > LOG_TAIL_READ_BYTES);

    let tail = read_log_tail(&log, 0, 100_000).unwrap();
    assert!(tail.ends_with("the last line"));
    for kept in tail.lines().filter(|l| *l != "the last line") {
        assert_eq!(kept.len(), 99, "a line was cut: {kept:?}");
    }
    assert!(tail.len() as u64 <= LOG_TAIL_READ_BYTES);
}

// ============================================================================
// create_below (R-F3)
// ============================================================================

#[test]
fn create_below_makes_each_missing_level_and_keeps_the_ones_there() {
    let tmp = tempdir().unwrap();
    let base = tmp.path().join("projects").join("-repo");
    fs::create_dir_all(base.join("uploads")).unwrap();
    fs::write(base.join("uploads").join("kept.txt"), b"kept").unwrap();

    let dir = create_below(&base, Path::new("uploads/2026-10-08")).unwrap();

    assert_eq!(dir, base.join("uploads").join("2026-10-08"));
    assert!(dir.is_dir());
    assert_eq!(
        fs::read(base.join("uploads").join("kept.txt")).unwrap(),
        b"kept"
    );
    assert_eq!(
        create_below(&base, Path::new("uploads/2026-10-08")).unwrap(),
        dir,
        "a second call finds every level there"
    );
    assert_eq!(create_below(&base, Path::new("")).unwrap(), base);
}

#[test]
fn create_below_a_base_that_is_gone_is_not_found_and_creates_nothing() {
    let tmp = tempdir().unwrap();
    let projects = tmp.path().join("projects");
    fs::create_dir_all(&projects).unwrap();
    let gone = projects.join("-repo");

    for rel in ["", "uploads", "artifacts/loose/2026-10-08"] {
        let err = create_below(&gone, Path::new(rel)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "{rel:?}: {err}");
    }
    assert!(!gone.exists(), "the base was re-created");
    assert_eq!(fs::read_dir(&projects).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn create_below_refuses_a_level_that_is_a_link_or_a_file() {
    let tmp = tempdir().unwrap();
    let base = tmp.path().join("base");
    let elsewhere = tmp.path().join("elsewhere");
    fs::create_dir_all(&base).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, base.join("uploads")).unwrap();
    fs::write(base.join("artifacts"), b"a file").unwrap();
    let linked_base = tmp.path().join("linked-base");
    std::os::unix::fs::symlink(&base, &linked_base).unwrap();

    for (b, rel) in [
        (&base, "uploads/2026-10-08"),
        (&base, "artifacts/loose"),
        (&linked_base, "memory"),
    ] {
        let err = create_below(b, Path::new(rel)).unwrap_err();
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::NotADirectory,
            "{}/{rel}: {err}",
            b.display()
        );
    }
    assert_eq!(
        fs::read_dir(&elsewhere).unwrap().count(),
        0,
        "followed a link"
    );
    assert!(!base.join("memory").exists());
}

#[test]
fn create_below_takes_plain_names_only() {
    let tmp = tempdir().unwrap();
    for rel in ["../escape", "/abs", "a/../b"] {
        let err = create_below(tmp.path(), Path::new(rel)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput, "{rel}: {err}");
    }
    assert!(!tmp.path().join("a").exists());
    assert!(!tmp.path().parent().unwrap().join("escape").exists());
}
