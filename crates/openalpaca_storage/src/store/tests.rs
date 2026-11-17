use super::*;
use openalpaca_app_support::paths::HOME_STORE_ENV;
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

    /// Moves the user's home directory — `HOME`, which `std::env::home_dir`
    /// reads, so `home_root()`'s fallback becomes `<home>/.openalpaca` — and
    /// sets `OPENALPACA_HOME_STORE` to `store`, or removes it for `None`. For
    /// the test-build guard of `store::projects::register`: its "unset" and
    /// "the real store" cases then name a temp directory, so even a guard that
    /// failed to fire could write nothing into the developer's store. Same
    /// lock, no second one. Panics, restoring both variables, if
    /// `std::env::home_dir` does not answer with `home`.
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
        let resolved = std::env::home_dir();
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

/// Whether the volume holding `dir` ignores case: a file made as
/// `case-probe-Ab` is found by a stat of `CASE-PROBE-AB`. macOS's default
/// volume does; Linux's tmpfs and ext4 do not, so a test of a rename by
/// case only returns early there (storage design ST-23). `dir` is a test's
/// own temp folder; the probe is removed before this returns. Unix only, as
/// every test that calls it.
#[cfg(unix)]
pub(crate) fn volume_ignores_case(dir: &Path) -> bool {
    let made = dir.join("case-probe-Ab");
    std::fs::write(&made, b"").unwrap();
    let ignores = std::fs::symlink_metadata(dir.join("CASE-PROBE-AB")).is_ok();
    std::fs::remove_file(&made).unwrap();
    ignores
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

/// The home scope's `ensure_store` is support's `ensure_home_store`: the
/// root created and seeded with its `README.md` and `.layout`, the root
/// returned.
#[test]
fn ensuring_the_home_store_seeds_its_root() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("home");
    let _guard = HomeStoreGuard::set(&root);
    assert!(!root.exists());

    assert_eq!(ensure_store(&StoreScope::Home).unwrap(), root);
    assert!(root.join("README.md").is_file());
    assert!(root.join(".layout").is_file());
    assert_eq!(ensure_home_store().unwrap(), root);
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

// ============================================================================
// Open's copies and the CLI's editor buffers (N-STORE-1)
// ============================================================================

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Open's copy lands at `state/cache/open/<file id>/<name>` in 0700 folders;
/// a second Open of the file replaces the first copy, under whatever name;
/// neither the id nor the name can lead out of the folder; and removing by
/// id takes only that folder.
#[test]
fn an_open_copy_is_staged_in_its_own_private_folder_and_stays_inside_it() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap().join("home");
    let _env = HomeStoreGuard::set(&home);
    let source = tmp.path().join("03-report.bin");
    fs::write(&source, b"v1").unwrap();
    let open = home.join("state").join("cache").join("open");

    let first = stage_open_copy(&source, "f-1", "Report.pdf").unwrap();
    assert_eq!(first, open.join("f-1").join("Report.pdf"));
    assert_eq!(fs::read(&first).unwrap(), b"v1");
    #[cfg(unix)]
    {
        assert_eq!(mode_of(&open), 0o700);
        assert_eq!(mode_of(&open.join("f-1")), 0o700);
    }
    fs::write(&source, b"v2").unwrap();
    let second = stage_open_copy(&source, "f-1", "Report (2).pdf").unwrap();
    assert!(!first.exists(), "the earlier copy goes with the fresh one");
    assert_eq!(fs::read(&second).unwrap(), b"v2");

    for (id, name) in [
        ("../state", "x.pdf"),
        ("a/b", "x.pdf"),
        ("", "x.pdf"),
        ("..", "x.pdf"),
        ("f-2", "../x.pdf"),
        ("f-2", "a/x.pdf"),
        ("f-2", ".."),
    ] {
        assert!(
            stage_open_copy(&source, id, name).is_err(),
            "{id:?} / {name:?} was staged"
        );
    }
    assert!(!open.join("f-2").exists());
    assert!(!home.join("state").join("x.pdf").exists());

    assert!(
        !remove_open_copy("../cache").unwrap(),
        "not one name: nothing"
    );
    assert!(open.exists(), "the folder of copies stays");
    assert!(remove_open_copy("f-1").unwrap());
    assert!(!open.join("f-1").exists());
    assert!(!remove_open_copy("f-1").unwrap(), "already gone");
    assert_eq!(sweep_open_copies().unwrap(), 0);
}

/// `state/cache/open` left as a link to another folder — by hand, or by an
/// approved shell command — is never followed: the boot sweep deletes nothing
/// in the folder it leads to, and Open refuses to stage there (nor makes that
/// folder 0700), naming the link.
#[cfg(unix)]
#[test]
fn a_link_at_the_folder_of_open_copies_is_never_followed() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap().join("home");
    let _env = HomeStoreGuard::set(&home);
    let elsewhere = tmp.path().join("elsewhere");
    fs::create_dir_all(elsewhere.join("a folder")).unwrap();
    fs::write(elsewhere.join("keep.txt"), b"not Open's").unwrap();
    fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o755)).unwrap();
    let cache = home.join("state").join("cache");
    fs::create_dir_all(&cache).unwrap();
    let open = cache.join("open");
    std::os::unix::fs::symlink(&elsewhere, &open).unwrap();

    assert_eq!(sweep_open_copies().unwrap(), 0, "the sweep took nothing");
    assert_eq!(
        fs::read(elsewhere.join("keep.txt")).unwrap(),
        b"not Open's",
        "the sweep deleted a file the link leads to"
    );
    assert!(elsewhere.join("a folder").is_dir());

    let source = tmp.path().join("03-report.bin");
    fs::write(&source, b"v1").unwrap();
    let refused = stage_open_copy(&source, "f-1", "Report.pdf")
        .expect_err("Open staged through a link")
        .to_string();
    assert!(refused.contains("is a link"), "{refused}");
    assert!(!elsewhere.join("f-1").exists());
    assert_eq!(mode_of(&elsewhere), 0o755, "the link's target was chmodded");
    // Nor does a project's delete reach through it by a file id.
    assert!(!remove_open_copy("a folder").unwrap());
    assert!(elsewhere.join("a folder").is_dir());
    assert!(
        fs::symlink_metadata(&open)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link is the user's to remove"
    );
}
