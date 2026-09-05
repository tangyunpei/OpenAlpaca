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
}

impl HomeStoreGuard {
    pub(crate) fn set(path: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os(HOME_STORE_ENV);
        // SAFETY: serialized by ENV_LOCK; every test that reads the variable
        // holds the same guard.
        unsafe { std::env::set_var(HOME_STORE_ENV, path) };
        Self { _lock: lock, prev }
    }
}

impl Drop for HomeStoreGuard {
    fn drop(&mut self) {
        // SAFETY: as above — still holding ENV_LOCK.
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

/// The former `paths.rs::test_paths_are_consistent`, re-targeted at `state_dir()`.
#[test]
fn test_paths_are_consistent() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(tmp.path());

    let state = state_dir().unwrap();
    let discovery = discovery_path().unwrap();
    let lock = lock_path().unwrap();
    let db = database_path().unwrap();
    let assets = interim_assets_dir().unwrap();
    let logs = logs_dir().unwrap();
    let backups = backups_dir().unwrap();

    assert_eq!(state, tmp.path().join("state"));
    assert!(state.is_dir(), "state_dir() creates the directory");
    for p in [&discovery, &lock, &db, &assets, &logs, &backups] {
        assert!(p.starts_with(&state), "{} is not under state/", p.display());
    }
    assert!(discovery.ends_with("discovery.json"));
    assert!(lock.ends_with("openalpacad.lock"));
    assert!(db.ends_with("openalpaca.db"));
    assert!(assets.ends_with("assets"));
    assert!(logs.is_dir() && logs.ends_with("logs"));
    assert!(backups.is_dir() && backups.ends_with("backups"));
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
        interim_assets_dir().unwrap(),
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

#[test]
fn interim_asset_paths_are_sharded_under_state() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(tmp.path());
    let sha = "abcdef0123456789";
    let path = interim_asset_storage_path(sha).unwrap();
    assert_eq!(
        path,
        tmp.path()
            .join("state")
            .join("assets")
            .join("ab")
            .join("cd")
            .join(sha)
    );
    assert!(interim_asset_storage_path("abc").is_err());
}

// ============================================================================
// ensure_store
// ============================================================================

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
    assert!(readme.contains("factory reset"));
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
    assert_eq!(text, "1\nproject_id=abc\n");
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
