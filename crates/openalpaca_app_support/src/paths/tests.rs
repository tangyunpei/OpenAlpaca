//! The home store's paths (moved from `openalpaca_storage::store`'s tests,
//! Q05): the root's resolution, the paths under `state/`, the logs' rotation
//! and tail, the README and `.layout`, the editor's folder. Every test that
//! resolves the home store holds [`HomeStoreGuard`], this crate's own.

use super::*;
use crate::test_guard::HomeStoreGuard;
use tempfile::tempdir;

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

/// An empty home is no home: the store is never the relative `.openalpaca`
/// the process's directory would resolve. `std::env::home_dir` answers an
/// empty home field of the user database as `Some("")` (its documentation
/// at Rust 1.99.1) where `directories` (dirs-sys 0.5.0) answered `None`, so
/// since M18 the resolution itself refuses it.
#[test]
fn an_empty_home_is_no_home() {
    let err = resolve_home_root(None, Some(PathBuf::new())).unwrap_err();
    assert!(
        err.to_string().contains("home directory"),
        "unexpected error: {err}"
    );
}

/// A relative home is no home (121-1, the owner's word of 2026-11-17: "use
/// treat a non-absolute home as no home. This is a new refusal."): the
/// store is never `<relative>/.openalpaca`, which the process's directory
/// would resolve — the rule the override already keeps.
#[test]
fn a_relative_home_is_no_home() {
    let err = resolve_home_root(None, Some(PathBuf::from("relative/home"))).unwrap_err();
    assert!(
        err.to_string().contains("home directory"),
        "unexpected error: {err}"
    );
    assert_eq!(
        resolve_home_root(None, Some(PathBuf::from("/Users/someone"))).unwrap(),
        PathBuf::from("/Users/someone/.openalpaca")
    );
}

/// `HOME` set to a relative path (which `std::env::home_dir` answers as
/// given) leaves `home_root` without a home: refused, not resolved against
/// the process's directory.
#[test]
fn home_root_refuses_a_relative_home() {
    let _guard = HomeStoreGuard::for_user_home(Path::new("relative-home"), None);
    let err = home_root().unwrap_err();
    assert!(
        err.to_string().contains("home directory"),
        "unexpected error: {err}"
    );
}

/// The one filter every home lookup goes through: an absolute home is
/// kept, an empty, relative or missing one is none.
#[test]
fn only_an_absolute_home_is_usable() {
    assert_eq!(
        usable_home(Some(PathBuf::from("/Users/someone"))),
        Some(PathBuf::from("/Users/someone"))
    );
    assert_eq!(usable_home(Some(PathBuf::new())), None);
    assert_eq!(usable_home(Some(PathBuf::from("relative/home"))), None);
    assert_eq!(usable_home(Some(PathBuf::from("~"))), None);
    assert_eq!(usable_home(None), None);
}

/// `user_home` is `std::env::home_dir` through that filter: the moved
/// `HOME` when it is absolute, none when `HOME` is relative.
#[test]
fn user_home_is_the_standard_librarys_home_when_absolute() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    {
        let _guard = HomeStoreGuard::for_user_home(&home, None);
        assert_eq!(user_home(), Some(home.clone()));
    }
    let _guard = HomeStoreGuard::for_user_home(Path::new("relative-home"), None);
    assert_eq!(user_home(), None);
}

/// With no override the store is `.openalpaca` under the home
/// `std::env::home_dir` reports — `HOME`, here a temp directory.
#[test]
fn home_root_falls_back_to_the_home_the_standard_library_reports() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    let _guard = HomeStoreGuard::for_user_home(&home, None);
    assert_eq!(std::env::home_dir().as_deref(), Some(home.as_path()));
    assert_eq!(home_root().unwrap(), home.join(STORE_DIR_NAME));
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
    assert_eq!(
        log,
        tmp.path().join("state").join("logs").join("daemon.log")
    );
    assert!(
        !tmp.path().join("state").join("logs").exists(),
        "daemon_log_path() must not create the logs directory"
    );

    // …and it is the same file `logs_dir()` hands the writer.
    assert_eq!(log, logs_dir().unwrap().join("daemon.log"));
}

/// Q14: the app's own log and a dev launch's are two names beside
/// `daemon.log`, never `~/Library/Logs` (D1), and naming them creates
/// nothing.
#[test]
fn the_gui_logs_are_named_beside_the_daemon_log_and_creating_nothing() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(tmp.path());

    let logs = tmp.path().join("state").join("logs");
    assert_eq!(gui_log_path().unwrap(), logs.join("gui.log"));
    assert_eq!(gui_dev_log_path().unwrap(), logs.join("gui-dev.log"));
    assert!(
        !logs.exists(),
        "naming the GUI logs must not create the logs directory"
    );
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

/// Subagent design §2.6 K-1: the built-in skills' files live under
/// `state/cache/bundled-skills/`, the machine's, beside the memory index —
/// a path query that creates nothing, and the home README names it.
#[test]
fn the_bundled_skills_dir_is_under_state_cache_and_creates_nothing() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("home");
    let _guard = HomeStoreGuard::set(&root);

    let pack = bundled_skills_dir().unwrap();

    assert_eq!(
        pack,
        root.join("state").join("cache").join("bundled-skills")
    );
    assert!(!root.exists(), "a path query created {}", root.display());
    assert!(
        HOME_README.contains("`bundled-skills/`, the built-in skills' files"),
        "the home README's state/cache/ row names bundled-skills/"
    );
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

    let root = ensure_home_store().unwrap();
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
    ensure_home_store().unwrap();
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
        // S6text M1: the row reads as a sentence, a space before the path.
        "per owner: `owners/<owner-key>/notes/*.md`",
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
/// `cache`: the live one is `state/cache/`), and `scratch/` alone stays:
/// since Stage 2 a conversation's working folder. A deleted project's files
/// that another conversation still shows are kept under `kept/` in the root's
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
        "scratch/ is listed: a conversation with no project works there"
    );
    for kept in ["artifacts/", "uploads/"] {
        let line = row(kept).unwrap_or_else(|| panic!("the home README has a {kept} row"));
        assert!(line.contains("`kept/`"), "{line}");
    }
}

/// Storage design Stage 2 (ST-10, ST-12): in a project an agent writes and
/// edits the project's own files — real files, each write asking first — and
/// OpenAlpaca's stores still write nothing there. Final review R27's row named
/// a daemon started outside the home store as the one writer inside a project;
/// Stage 2 removed that startup-folder write (`file_write` acts in the turn's
/// working folder), so the row names the approved agent instead. A chosen
/// folder with no git is a project too, so the row promises no git history:
/// it names the pre-edit copy OpenAlpaca keeps in the conversation's log, and
/// git only where the project is a repository. Only `file_write` and
/// `file_edit` keep that copy (final review d8-docs-2): the row names them and
/// says a shell command's changes keep none — the README is written only when
/// absent, so a store keeps whatever it was seeded with.
#[test]
fn the_home_readme_projects_row_says_agents_write_the_projects_own_files_only_with_approval() {
    let projects = HOME_README
        .lines()
        .find(|line| line.starts_with("| `projects/` |"))
        .expect("the home README has a projects/ row");
    for needle in [
        "project.json",
        "by-products",
        "real files",
        "asking you first",
        "`sessions/<id>/snapshots/`",
        "before `file_write` or `file_edit` replaces or edits a file",
        "a shell command's changes keep no copy",
        "where the project is a repository",
        "never the project's own files",
    ] {
        assert!(
            projects.contains(needle),
            "the projects/ row must say {needle:?}: {projects}"
        );
    }
    for stale in [
        "`openalpaca daemon start`",
        "the directory it was started in",
        "plain files",
        "Nothing is ever written inside",
        "git keeps",
        "before an agent replaces or edits",
    ] {
        assert!(
            !projects.contains(stale),
            "the projects/ row must not say {stale:?}: {projects}"
        );
    }
}

/// Storage design Stage 2 (ST-10, R44): a conversation with no project works
/// in a folder of its own, `scratch/<conversation id>/`, made the first time a
/// tool works there and removed with the conversation — clearing or archiving
/// it keeps the folder, and nothing else sweeps it.
#[test]
fn the_home_readme_scratch_row_says_a_conversation_with_no_project_works_there_and_it_goes_with_it()
{
    let scratch = HOME_README
        .lines()
        .find(|line| line.starts_with("| `scratch/` |"))
        .expect("the home README has a scratch/ row");
    for needle in [
        "no project",
        "`scratch/<conversation id>/`",
        "shell commands",
        "removed with its conversation",
        "nothing else sweeps it",
    ] {
        assert!(
            scratch.contains(needle),
            "the scratch/ row must say {needle:?}: {scratch}"
        );
    }
    for stale in ["reserved", "swept freely"] {
        assert!(
            !scratch.contains(stale),
            "the scratch/ row must not say {stale:?}: {scratch}"
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
    let root = ensure_home_store().unwrap();
    let readme = root.join("README.md");

    let edited = format!("{HOME_README}\nMy note: keep backups.\n");
    let trimmed = HOME_README.trim_end().to_owned();
    for mine in [edited, trimmed, "# my own notes\n".to_owned()] {
        fs::write(&readme, &mine).unwrap();
        ensure_home_store().unwrap();
        assert_eq!(fs::read_to_string(&readme).unwrap(), mine);
    }
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

    ensure_home_store().unwrap();

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

    ensure_home_store().unwrap();

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
    ensure_home_store().unwrap();
    assert!(
        !tmp.path().join(".layout.tmp").exists(),
        ".layout is written through a temp file that must be renamed away"
    );
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

/// Q14: `gui.log` and `gui-dev.log` rotate under the daemon log's caps, each
/// on its own — rotating one leaves the others where they are — and a fresh
/// root rotates nothing and creates nothing.
#[test]
fn the_gui_logs_rotate_under_the_daemon_logs_caps_each_on_its_own() {
    let tmp = tempdir().unwrap();
    let _guard = HomeStoreGuard::set(tmp.path());

    rotate_gui_log().expect("a fresh root rotates nothing");
    rotate_gui_dev_log().expect("a fresh root rotates nothing");
    assert!(
        !tmp.path().join("state").exists(),
        "rotating a log that is not there must not create the store"
    );

    let logs = logs_dir().unwrap();
    for name in ["daemon.log", "gui.log", "gui-dev.log"] {
        fs::File::create(logs.join(name))
            .unwrap()
            .set_len(DAEMON_LOG_MAX_BYTES + 1)
            .unwrap();
    }

    rotate_gui_log().expect("rotation should succeed");
    assert!(!logs.join("gui.log").exists());
    assert!(logs.join("gui.log.1").exists());
    assert!(logs.join("gui-dev.log").exists(), "only gui.log moved");
    assert!(logs.join("daemon.log").exists(), "only gui.log moved");

    rotate_gui_dev_log().expect("rotation should succeed");
    assert!(!logs.join("gui-dev.log").exists());
    assert!(logs.join("gui-dev.log.1").exists());
    assert!(logs.join("daemon.log").exists(), "only gui-dev.log moved");

    // Under the cap, either is left as it is.
    fs::write(logs.join("gui.log"), "a short run\n").unwrap();
    rotate_gui_log().expect("rotation should succeed");
    assert_eq!(
        fs::read_to_string(logs.join("gui.log")).unwrap(),
        "a short run\n"
    );
}

#[test]
fn log_tail_with_fewer_lines_than_asked_is_all_of_them() {
    assert_eq!(log_tail("one\ntwo\n", 20), "one\ntwo");
}

#[test]
fn log_tail_keeps_only_the_newest_lines_and_ignores_the_trailing_newline() {
    let text = "a\nb\nc\nd\ne\n";
    assert_eq!(log_tail(text, 2), "d\ne");
    assert_eq!(
        log_tail("a\nb\nc\n\n\n", 2),
        "b\nc",
        "trailing blanks are not lines"
    );
    assert_eq!(
        log_tail("a\r\nb\r\n", 5),
        "a\nb",
        "CRLF ends a line like LF"
    );
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
// The CLI's editor buffers (N-STORE-1)
// ============================================================================

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// The CLI's editor buffers live in `state/editor/`, made 0700 with `state/`.
#[test]
fn the_editor_dir_is_a_private_folder_under_state() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap().join("home");
    let _env = HomeStoreGuard::set(&home);
    let dir = editor_dir().unwrap();
    assert_eq!(dir, home.join("state").join("editor"));
    assert!(dir.is_dir());
    #[cfg(unix)]
    {
        assert_eq!(mode_of(&dir), 0o700);
        assert_eq!(mode_of(&home.join("state")), 0o700);
    }
}
