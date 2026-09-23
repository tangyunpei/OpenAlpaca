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
    /// The user-home variables [`HomeStoreGuard::set_with_home`] replaced, to
    /// restore on drop. Empty for [`HomeStoreGuard::set`].
    prev_home: Vec<(&'static str, Option<OsString>)>,
}

/// What `directories::ProjectDirs` reads to place the legacy app dir: `HOME`
/// everywhere on unix, and `XDG_DATA_HOME` first on Linux when it is set.
const USER_HOME_VARS: [&str; 2] = ["HOME", "XDG_DATA_HOME"];

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
            prev_home: Vec::new(),
        }
    }

    /// [`HomeStoreGuard::set`], and also points the user's home directory at
    /// `home` — for any test that can reach [`legacy_root::legacy_app_dir`].
    ///
    /// `OPENALPACA_HOME_STORE` does not move the legacy root: it resolves
    /// through `directories::ProjectDirs`, which reads `HOME`, so under
    /// [`HomeStoreGuard::set`] alone such a test would resolve the real
    /// `~/Library/Application Support/OpenAlpaca`. Same lock, no second one.
    /// Panics — restoring everything on the way out — if the legacy root still
    /// resolves outside `home`: a test must never be able to reach it.
    pub(crate) fn set_with_home(path: &Path, home: &Path) -> Self {
        let mut guard = Self::set(path);
        guard.prev_home = USER_HOME_VARS
            .iter()
            .map(|var| (*var, std::env::var_os(var)))
            .collect();
        // SAFETY: still holding ENV_LOCK, taken by `set` above.
        unsafe {
            std::env::set_var("HOME", home);
            std::env::remove_var("XDG_DATA_HOME");
        }
        let legacy = legacy_root::legacy_app_dir().expect("the legacy app dir must resolve");
        assert!(
            legacy.starts_with(home),
            "HOME override did not sandbox the legacy root ({}); refusing to run",
            legacy.display()
        );
        guard
    }
}

impl Drop for HomeStoreGuard {
    fn drop(&mut self) {
        // SAFETY: as above — still holding ENV_LOCK.
        for (var, prev) in self.prev_home.drain(..) {
            match prev {
                Some(v) => unsafe { std::env::set_var(var, v) },
                None => unsafe { std::env::remove_var(var) },
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
    // L11: the embedding model's ~1 GB of weights is regenerable machine
    // state, so it belongs under `state/` — not in the daemon's CWD, which is
    // where the library puts it when nobody says otherwise.
    let embeddings = embedding_cache_dir().unwrap();

    assert_eq!(state, tmp.path().join("state"));
    assert!(state.is_dir(), "state_dir() creates the directory");
    for p in [&discovery, &lock, &db, &logs, &backups, &embeddings] {
        assert!(p.starts_with(&state), "{} is not under state/", p.display());
    }
    assert!(discovery.ends_with("discovery.json"));
    assert!(lock.ends_with("openalpacad.lock"));
    assert!(db.ends_with("openalpaca.db"));
    assert!(logs.is_dir() && logs.ends_with("logs"));
    assert!(backups.is_dir() && backups.ends_with("backups"));
    assert_eq!(embeddings, state.join("cache").join("fastembed"));
    assert!(
        embeddings.is_dir(),
        "embedding_cache_dir() creates it on demand"
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
    // `open_home_database` checks for a legacy root first, so `HOME` is
    // sandboxed too (and holds no legacy root).
    let _guard = HomeStoreGuard::set_with_home(&root, &tmp.path().join("user"));
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

/// Where a pre-D2 upload's bytes sit: `state/assets/ab/cd/<sha256>`.
///
/// A fixture, not a path the system computes any more — nothing writes there
/// and nothing moves what is there. It lives here, beside the private
/// `state_dir_path` it is spelled from, so a test that reconstructs the old
/// layout does not re-derive it.
pub(crate) fn interim_blob_path(sha256: &str) -> PathBuf {
    state_dir_path()
        .unwrap()
        .join("assets")
        .join(&sha256[0..2])
        .join(&sha256[2..4])
        .join(sha256)
}

// ============================================================================
// ensure_store
// ============================================================================

/// The README is the document this subsystem writes into the user's own
/// project, and D2 put human-named uploads there. The daemon's asset sweep
/// deletes an upload that was never attached to a message once the grace period
/// passes — bytes and row — so no retention row may promise those files are kept
/// forever. Produced artifacts are genuinely never swept; only uploads are.
#[test]
fn the_readmes_tell_the_truth_about_upload_retention() {
    for (name, text) in [("home", HOME_README), ("project", PROJECT_README)] {
        let uploads = text
            .lines()
            .find(|line| line.starts_with("| `uploads/`"))
            .unwrap_or_else(|| panic!("the {name} README has no uploads/ row"));
        assert!(
            !uploads.contains("never garbage-collected"),
            "the {name} README promises uploads survive, but the sweep deletes them: {uploads}"
        );
        assert!(
            uploads.contains("grace period"),
            "the {name} README must name the retention rule the sweep applies: {uploads}"
        );
        assert!(
            text.lines()
                .any(|line| line.starts_with("| `artifacts/`") && line.contains("never")),
            "the {name} README must keep saying produced artifacts are never swept"
        );
    }
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

#[test]
fn ensure_store_seeds_a_project_root() {
    let tmp = tempdir().unwrap();
    let project = tmp.path().to_path_buf();
    let scope = StoreScope::Project(project.clone());

    let root = ensure_store(&scope).unwrap();
    assert_eq!(root, project.join(".openalpaca"));

    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert_eq!(
        gitignore,
        "/.layout\n/uploads/\n/sessions/\n/scratch/\n/cache/\n.versions/\n"
    );
    assert!(
        fs::read_to_string(root.join("README.md"))
            .unwrap()
            .contains("project store")
    );
    assert_eq!(layout_version(&root).unwrap(), Some(1));
    assert_eq!(
        install_id(&root).unwrap(),
        None,
        "only the home root carries an install id"
    );

    // User edits stick.
    fs::write(root.join(".gitignore"), "# mine\n").unwrap();
    ensure_store(&scope).unwrap();
    assert_eq!(
        fs::read_to_string(root.join(".gitignore")).unwrap(),
        "# mine\n"
    );
}

/// `walk_up_for_marker` counts `.openalpaca` as a project marker, so a path
/// under `$HOME` with no closer marker resolves to `$HOME` — whose "project
/// store" is the home root itself. Seeded from the project branch, that put a
/// `.gitignore` in the home root and a README describing a project store. The
/// metadata follows the resolved root, not the scope variant.
#[test]
fn ensure_store_on_a_project_that_is_the_home_root_seeds_the_home_metadata() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    let _guard = HomeStoreGuard::set(&home.join(".openalpaca"));

    // `Project($HOME)` — the shape a CWD under `$HOME` with no marker resolves
    // to. Its store root is `$HOME/.openalpaca`, which *is* the home root.
    let root = ensure_store(&StoreScope::Project(home.clone())).unwrap();
    assert_eq!(root, home.join(".openalpaca"));

    assert!(
        !root.join(".gitignore").exists(),
        "no project .gitignore is written into the home root"
    );
    let readme = fs::read_to_string(root.join("README.md")).unwrap();
    assert!(
        readme.contains("Retention class") && !readme.contains("project store"),
        "the home README is the one seeded"
    );
    assert!(
        install_id(&root).unwrap().is_some(),
        "and the home root's install id with it"
    );

    // A real project under the home directory — one with a root of its own — is
    // untouched by the fold.
    let inner = home.join("work");
    let inner_root = ensure_store(&StoreScope::Project(inner.clone())).unwrap();
    assert_eq!(inner_root, inner.join(".openalpaca"));
    assert!(inner_root.join(".gitignore").exists());
}

#[test]
fn unknown_entries_names_only_what_the_store_did_not_create() {
    let tmp = tempdir().unwrap();
    let scope = StoreScope::Project(tmp.path().to_path_buf());
    let root = ensure_store(&scope).unwrap();
    // Two kinds the store created, and two names it did not.
    fs::create_dir_all(root.join("artifacts")).unwrap();
    fs::create_dir_all(root.join("uploads")).unwrap();
    fs::create_dir_all(root.join("my-notes")).unwrap();
    fs::write(root.join("todo.txt"), "mine").unwrap();

    assert_eq!(
        unknown_entries(&root, false),
        vec!["my-notes".to_string(), "todo.txt".to_string()],
        "the seeded metadata and every ContentKind are the store's own"
    );
    // A root with nothing in it, and one that does not exist, both say nothing.
    assert!(unknown_entries(&tmp.path().join("no-such-store"), false).is_empty());
}

#[test]
fn unknown_entries_treats_state_and_plugins_as_home_only_names() {
    let tmp = tempdir().unwrap();
    let scope = StoreScope::Project(tmp.path().to_path_buf());
    let root = ensure_store(&scope).unwrap();
    fs::create_dir_all(root.join("state")).unwrap();
    fs::create_dir_all(root.join("plugins")).unwrap();

    // Home-root names, so a project root does not get a pass on them: they
    // are exactly the kind of name this list exists to report.
    assert_eq!(
        unknown_entries(&root, false),
        vec!["plugins".to_string(), "state".to_string()],
        "under a project root, state/plugins are unknown like any other name"
    );
    // The same directory, read as the home root, waves both through.
    assert!(
        unknown_entries(&root, true).is_empty(),
        "the home root owns state/ and plugins/ beside its content dirs"
    );
}

/// The Minor #4 regression the round-2 findings caught: `config` is reserved
/// by the *project* README too ("`memory/`, `skills/`, `config/` — reserved;
/// not created until used"), even though it is not a [`ContentKind`] — so an
/// existing `<project>/.openalpaca/config/` must be named once, by the plan's
/// own `skills/, config/` keep line, and not a second time here.
#[test]
fn unknown_entries_treats_config_as_reserved_in_both_scopes() {
    let tmp = tempdir().unwrap();
    let scope = StoreScope::Project(tmp.path().to_path_buf());
    let root = ensure_store(&scope).unwrap();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::create_dir_all(root.join("notes")).unwrap();

    assert_eq!(
        unknown_entries(&root, false),
        vec!["notes".to_string()],
        "config/ is a reserved project name, not an unknown one — notes/ still is"
    );
    // The home root owns config/ too, but not notes/ — it is unknown to
    // either scope, so it stays in both answers.
    assert_eq!(
        unknown_entries(&root, true),
        vec!["notes".to_string()],
        "config/ is known at the home root as well"
    );
}

/// §1.3 rule 1: dot-prefixed names are reserved for store metadata forever.
/// `.versions/` lives nested under `artifacts/**/` today and never at a store
/// root, but the reservation holds regardless of whether anything currently
/// writes there.
#[test]
fn unknown_entries_reserves_dot_versions_even_though_nothing_writes_it_at_the_root() {
    let tmp = tempdir().unwrap();
    let scope = StoreScope::Project(tmp.path().to_path_buf());
    let root = ensure_store(&scope).unwrap();
    fs::create_dir_all(root.join(".versions")).unwrap();

    assert!(
        unknown_entries(&root, false).is_empty(),
        ".versions is reserved store metadata, not an unknown name"
    );
}

#[test]
fn install_id_is_appended_once_to_a_pre_existing_layout() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(tmp.path());
    fs::write(tmp.path().join(".layout"), "1\n").unwrap();

    ensure_store(&StoreScope::Home).unwrap();
    let id = install_id(tmp.path()).unwrap().unwrap();
    assert_eq!(layout_version(tmp.path()).unwrap(), Some(1));

    ensure_store(&StoreScope::Home).unwrap();
    assert_eq!(install_id(tmp.path()).unwrap().unwrap(), id);
}

#[test]
fn layout_version_reports_absence_and_rejects_garbage() {
    let tmp = tempdir().unwrap();
    assert_eq!(layout_version(tmp.path()).unwrap(), None);
    fs::write(tmp.path().join(".layout"), "not-a-number\n").unwrap();
    assert!(layout_version(tmp.path()).is_err());
}

#[test]
fn a_malformed_layout_marker_is_repaired_not_appended_to() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(tmp.path());
    fs::write(tmp.path().join(".layout"), "not-a-number\n").unwrap();

    ensure_store(&StoreScope::Home).unwrap();

    assert_eq!(layout_version(tmp.path()).unwrap(), Some(LAYOUT_VERSION));
    assert!(install_id(tmp.path()).unwrap().is_some());
}

#[test]
fn layout_lines_this_module_does_not_own_are_preserved() {
    let tmp = tempdir().unwrap();
    let project = tmp.path().to_path_buf();
    let scope = StoreScope::Project(project.clone());
    let root = ensure_store(&scope).unwrap();
    // A future project id (P-12) must survive a repair of line 1.
    fs::write(root.join(".layout"), "garbage\nproject_id=abc\n").unwrap();

    ensure_store(&scope).unwrap();

    let text = fs::read_to_string(root.join(".layout")).unwrap();
    let recorded = project.canonicalize().unwrap();
    assert_eq!(
        text,
        format!("1\nproject_id=abc\nproject_root={}\n", recorded.display()),
        "line 1 repaired, the foreign line kept, the recorded root added"
    );
}

#[test]
fn a_project_store_records_its_own_root_once() {
    let tmp = tempdir().unwrap();
    let project = tmp.path().canonicalize().unwrap();
    let scope = StoreScope::Project(project.clone());
    let root = ensure_store(&scope).unwrap();

    assert_eq!(
        recorded_project_root(&root).unwrap().as_deref(),
        Some(project.to_string_lossy().as_ref())
    );
    assert_eq!(
        recorded_project_root(&tmp.path().join("nowhere")).unwrap(),
        None,
        "a directory with no marker records nothing"
    );

    // The whole point: `ensure_store` runs on every content_dir call, and a
    // line that healed itself to the current path would erase the difference a
    // moved project is recognised by.
    set_recorded_project_root(&root, "/somewhere/else").unwrap();
    ensure_store(&scope).unwrap();
    content_dir(&scope, ContentKind::Artifacts).unwrap();
    assert_eq!(
        recorded_project_root(&root).unwrap().as_deref(),
        Some("/somewhere/else")
    );

    // And the rewrite keeps the rest of the marker intact.
    assert_eq!(layout_version(&root).unwrap(), Some(LAYOUT_VERSION));
}

#[test]
fn the_home_root_records_no_project_root() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(&tmp.path().canonicalize().unwrap());
    let root = ensure_store(&StoreScope::Home).unwrap();
    assert_eq!(recorded_project_root(&root).unwrap(), None);
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
fn a_project_content_dir_seeds_the_store_first() {
    let tmp = tempdir().unwrap();
    let project = tmp.path().to_path_buf();

    let uploads = content_dir(&StoreScope::Project(project.clone()), ContentKind::Uploads).unwrap();

    assert!(uploads.is_dir());
    assert!(
        project.join(".openalpaca").join(".gitignore").exists(),
        "uploads must never exist before the .gitignore that excludes them from git"
    );
}

#[test]
fn content_dirs_have_the_same_shape_in_both_scopes() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(&tmp.path().join("home"));
    let project = tmp.path().join("proj");

    for kind in [
        ContentKind::Artifacts,
        ContentKind::Uploads,
        ContentKind::Sessions,
        ContentKind::Memory,
        ContentKind::Skills,
        ContentKind::Scratch,
        ContentKind::Cache,
    ] {
        let home = content_dir(&StoreScope::Home, kind).unwrap();
        let proj = content_dir(&StoreScope::Project(project.clone()), kind).unwrap();
        assert_eq!(home, tmp.path().join("home").join(kind.dir_name()));
        assert_eq!(proj, project.join(".openalpaca").join(kind.dir_name()));
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
/// written — the legacy-root refusal is paragraphs — while the blank lines the
/// window would open with are not worth a line of the panel.
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
