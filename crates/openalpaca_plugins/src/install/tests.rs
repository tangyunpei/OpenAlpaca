//! The on-disk half of GAP-24: what a copy, a replace and a trash-move
//! guarantee when the daemon dies in the middle of one.

use std::path::{Path, PathBuf};

use super::*;

/// A plugins root with `.staging/`, `.trash/` and nothing else.
fn roots() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let plugins = tmp.path().join("plugins");
    let sources = tmp.path().join("sources");
    std::fs::create_dir_all(&plugins).expect("plugins root");
    std::fs::create_dir_all(&sources).expect("sources root");
    (tmp, plugins, sources)
}

/// A source directory holding a manifest and one nested file.
fn source(sources: &Path, dir_name: &str, manifest_name: &str) -> PathBuf {
    let dir = sources.join(dir_name);
    std::fs::create_dir_all(dir.join("lib")).expect("source tree");
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            "[plugin]\nname = \"{manifest_name}\"\nversion = \"1.0.0\"\nentry = \"./run.sh\"\n\
             [types]\ntools = true\n\
             [capabilities]\nprovides = [\"notes_write\"]\n\
             [config.token]\ntype = \"secret\"\nrequired = true\nsensitive = true\ndescription = \"API token\"\n"
        ),
    )
    .expect("manifest");
    std::fs::write(dir.join("run.sh"), "#!/bin/sh\n").expect("entry");
    std::fs::write(dir.join("lib/helper.js"), "// helper\n").expect("nested file");
    dir
}

// ── inspect_source ───────────────────────────────────────────────────────

#[test]
fn a_source_directory_is_summarised_before_anything_is_copied() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");

    let (name, summary) = inspect_source_blocking(&dir, &plugins).expect("a valid source");

    assert_eq!(name, "notion", "the directory name is the extension id");
    assert_eq!(summary.version, "1.0.0");
    assert_eq!(summary.entry, "./run.sh");
    assert_eq!(summary.capabilities, vec!["notes_write".to_string()]);
    assert_eq!(summary.types.get("tool"), Some(&true));
    assert_eq!(summary.required_config_keys, vec!["token".to_string()]);
    assert_eq!(summary.sensitive_config_keys, vec!["token".to_string()]);
    assert!(
        !plugins.join("notion").exists(),
        "inspecting must copy nothing"
    );
}

#[test]
fn a_relative_source_is_refused_before_the_filesystem_is_touched() {
    let (_tmp, plugins, _sources) = roots();
    let error = inspect_source_blocking(Path::new("some/plugin"), &plugins).unwrap_err();
    assert_eq!(error.code(), "invalid_path");
}

/// "Load in place from an arbitrary path" stays declined, and installing a
/// directory that is *already* under the plugins root would either be a no-op
/// or a self-copy — both are caller mistakes, not installs.
#[test]
fn a_source_inside_the_plugins_root_is_refused() {
    let (_tmp, plugins, _sources) = roots();
    let inside = source(&plugins, "notion", "notion");
    let error = inspect_source_blocking(&inside, &plugins).unwrap_err();
    assert_eq!(error.code(), "invalid_path");
}

#[test]
fn a_missing_directory_is_source_not_found() {
    let (_tmp, plugins, sources) = roots();
    let error = inspect_source_blocking(&sources.join("nope"), &plugins).unwrap_err();
    assert_eq!(error.code(), "source_not_found");
}

#[test]
fn a_directory_with_no_manifest_is_invalid_manifest() {
    let (_tmp, plugins, sources) = roots();
    let bare = sources.join("bare");
    std::fs::create_dir_all(&bare).unwrap();
    assert_eq!(
        inspect_source_blocking(&bare, &plugins).unwrap_err().code(),
        "invalid_manifest"
    );

    std::fs::write(bare.join("plugin.toml"), "this is not = = toml [[[").unwrap();
    assert_eq!(
        inspect_source_blocking(&bare, &plugins).unwrap_err().code(),
        "invalid_manifest"
    );
}

/// Design §2.2: the plugin key is the **directory** name. A manifest that
/// disagrees can never load — `reconcile_dir` parks it `Failed{ConfigInvalid}`
/// — so the install refuses it rather than landing a directory that is dead on
/// arrival.
#[test]
fn a_manifest_that_renames_itself_is_refused_at_install_time() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion-for-openalpaca");
    let error = inspect_source_blocking(&dir, &plugins).unwrap_err();
    assert_eq!(error.code(), "invalid_manifest");
    assert!(
        error.to_string().contains("notion-for-openalpaca"),
        "the refusal names both halves: {error}"
    );
}

/// The dot directories are the store's own: `.staging`, `.trash`, `.data`,
/// `.config` and `.permissions.toml` all live at the plugins root.
#[test]
fn a_dot_directory_can_never_be_installed_as_a_plugin() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, ".config", ".config");
    assert_eq!(
        inspect_source_blocking(&dir, &plugins).unwrap_err().code(),
        "invalid_path"
    );
}

/// **The hang, one path over.** `copy_tree` refuses a FIFO because opening one
/// with no writer never returns — but the manifest is read *before* the copy,
/// and `plugin.toml` itself is an entry in the source directory. A
/// `mkfifo plugin.toml` used to reach `read_to_string` with the type check
/// living downstream in a `copy_tree` that is never reached, so the inspection
/// parked forever. The type is checked with `symlink_metadata` before the open;
/// the timeout is what makes the assertion honest.
#[cfg(unix)]
#[tokio::test]
async fn a_fifo_plugin_toml_is_refused_rather_than_opened() {
    let (_tmp, plugins, sources) = roots();
    let dir = sources.join("notion");
    std::fs::create_dir_all(&dir).expect("source dir");
    let made = std::process::Command::new("mkfifo")
        .arg(dir.join("plugin.toml"))
        .status()
        .expect("mkfifo");
    assert!(made.success(), "mkfifo failed");

    let error = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        inspect_source(&dir, &plugins),
    )
    .await
    .expect("the inspection must refuse a FIFO manifest, not block on opening it")
    .unwrap_err();

    assert_eq!(error.code(), "invalid_manifest");
    assert!(
        error.to_string().contains("plugin.toml") && error.to_string().contains("regular file"),
        "the refusal names the file and what is wrong with it: {error}"
    );
}

/// The update path reads the manifest through its own second call site
/// (`inspect_update_source`'s fallback for a source directory whose name is not
/// the plugin's), so it gets the same refusal rather than the same hang.
#[cfg(unix)]
#[tokio::test]
async fn a_fifo_plugin_toml_is_refused_on_the_update_path_too() {
    let (_tmp, plugins, sources) = roots();
    let dir = sources.join("build-output");
    std::fs::create_dir_all(&dir).expect("source dir");
    let made = std::process::Command::new("mkfifo")
        .arg(dir.join("plugin.toml"))
        .status()
        .expect("mkfifo");
    assert!(made.success(), "mkfifo failed");

    let error = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        inspect_update_source(&dir, &plugins, "notion"),
    )
    .await
    .expect("the update inspection must refuse a FIFO manifest too")
    .unwrap_err();

    assert_eq!(error.code(), "invalid_manifest");
}

/// The read is `std::fs` and the route runs its verb in a `tokio::spawn`ed task
/// — i.e. on a runtime worker. On a **current-thread** runtime, an inspection
/// done inline would return `Ready` without ever yielding, so the task spawned
/// just before it could not have run; this asserts one did, which is only true
/// if the read went to `spawn_blocking`.
#[tokio::test(flavor = "current_thread")]
async fn the_manifest_read_runs_off_the_runtime_worker() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");

    let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&ran);
    tokio::spawn(async move { flag.store(true, std::sync::atomic::Ordering::SeqCst) });

    let (name, _) = inspect_source(&dir, &plugins).await.expect("inspect");

    assert!(
        ran.load(std::sync::atomic::Ordering::SeqCst),
        "the runtime polled another task while the manifest was read; it was not done inline"
    );
    assert_eq!(name, "notion", "and it inspected the right directory");
}

// ── stage → commit ───────────────────────────────────────────────────────

#[tokio::test]
async fn staging_copies_the_whole_tree_and_commit_renames_it_into_place() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");

    let staged = stage(&dir, &plugins, "notion").await.expect("stage");
    assert!(staged.path().join("lib/helper.js").is_file());
    assert!(
        !plugins.join("notion").exists(),
        "nothing is visible under the plugins root until the rename"
    );

    staged.commit(&plugins.join("notion")).expect("commit");
    assert!(plugins.join("notion/plugin.toml").is_file());
    assert!(plugins.join("notion/lib/helper.js").is_file());
    assert!(
        std::fs::read_dir(plugins.join(STAGING_DIR))
            .map(|mut d| d.next().is_none())
            .unwrap_or(true),
        "the staging area is left clean"
    );
}

/// **The crash-injection point.** A staged copy that is never committed — the
/// daemon died, the caller returned early — leaves the plugins root exactly as
/// it was. A half-copied `plugins/<name>` would be scanned at the next boot as
/// a real plugin.
#[tokio::test]
async fn a_staged_copy_that_is_never_committed_leaves_no_half_written_plugin() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");

    {
        let staged = stage(&dir, &plugins, "notion").await.expect("stage");
        assert!(staged.path().exists());
        // …and here the process dies / the caller bails out.
    }

    assert!(!plugins.join("notion").exists());
    let leftovers: Vec<_> = std::fs::read_dir(plugins.join(STAGING_DIR))
        .expect("staging dir")
        .flatten()
        .collect();
    assert!(
        leftovers.is_empty(),
        "the abandoned staging copy is swept: {leftovers:?}"
    );
}

/// A staged copy is a *sibling* of the destination, so the commit is a rename
/// within one filesystem — never a copy that could be observed half-done.
#[tokio::test]
async fn the_staging_area_is_a_sibling_of_the_destination() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");
    let staged = stage(&dir, &plugins, "notion").await.expect("stage");
    assert_eq!(
        staged.path().parent().and_then(|p| p.file_name()),
        Some(std::ffi::OsStr::new(STAGING_DIR)),
    );
    assert_eq!(staged.path().parent().and_then(|p| p.parent()), Some(&*plugins));
}

// ── symlinks ─────────────────────────────────────────────────────────────

#[cfg(unix)]
#[tokio::test]
async fn a_symlink_pointing_out_of_the_source_is_refused() {
    let (tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");
    let secret = tmp.path().join("id_rsa");
    std::fs::write(&secret, "PRIVATE KEY").unwrap();
    std::os::unix::fs::symlink(&secret, dir.join("key")).unwrap();

    let error = stage(&dir, &plugins, "notion").await.unwrap_err();
    assert_eq!(error.code(), "escaping_symlink");
    assert!(
        std::fs::read_dir(plugins.join(STAGING_DIR))
            .map(|mut d| d.next().is_none())
            .unwrap_or(true),
        "the refused copy leaves nothing behind"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlink_inside_the_source_is_copied_as_its_target() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");
    std::os::unix::fs::symlink(dir.join("lib/helper.js"), dir.join("main.js")).unwrap();

    let staged = stage(&dir, &plugins, "notion").await.expect("stage");
    let copied = staged.path().join("main.js");
    assert!(
        !copied.symlink_metadata().unwrap().file_type().is_symlink(),
        "the copy dereferences it — the install never links out of the store"
    );
    assert_eq!(std::fs::read_to_string(copied).unwrap(), "// helper\n");
}

// ── the walk's bounds ────────────────────────────────────────────────────

/// **The multiplier.** `link -> .` canonicalizes to the source root, passes the
/// `starts_with(root)` check and re-enters the walk on the whole tree, so the
/// depth cap bounds the recursion but not the work: with two such links per
/// level the branching factor is 2 and the bound is `2^32` directories. The
/// disk fills long before the depth cap fires. A visited set of canonical
/// directories refuses the repeat instead.
#[cfg(unix)]
#[tokio::test]
async fn a_symlink_cycle_is_refused_rather_than_walked() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");
    std::os::unix::fs::symlink(".", dir.join("loop")).unwrap();

    let error = stage(&dir, &plugins, "notion").await.unwrap_err();

    assert_eq!(error.code(), "invalid_path");
    assert!(
        error.to_string().contains("more than once"),
        "the refusal says the walk would repeat a directory: {error}"
    );
    assert!(
        std::fs::read_dir(plugins.join(STAGING_DIR))
            .map(|mut d| d.next().is_none())
            .unwrap_or(true),
        "and nothing duplicated is left behind"
    );
}

/// Two links to one inner directory are the same shape as a cycle — the copy
/// would duplicate that subtree — so they are refused by the same rule.
#[cfg(unix)]
#[tokio::test]
async fn a_directory_reached_twice_is_refused_by_the_same_rule() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");
    std::os::unix::fs::symlink(dir.join("lib"), dir.join("also-lib")).unwrap();

    let error = stage(&dir, &plugins, "notion").await.unwrap_err();
    assert_eq!(error.code(), "invalid_path");
    assert!(error.to_string().contains("more than once"), "{error}");
}

/// The entry budget, tripped on a synthetic tree. The refusal names which
/// budget it was.
#[test]
fn the_entry_budget_stops_a_tree_with_too_many_files() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");
    for i in 0..20 {
        std::fs::write(dir.join(format!("lib/file-{i}.js")), "// x\n").unwrap();
    }

    let budget = CopyBudget {
        max_entries: 8,
        ..CopyBudget::default()
    };
    let error = stage_blocking(&dir, &plugins, "notion", budget).unwrap_err();

    assert_eq!(error.code(), "invalid_path");
    assert!(
        error.to_string().contains("8 entries"),
        "the refusal names the budget that tripped: {error}"
    );
    assert!(
        std::fs::read_dir(plugins.join(STAGING_DIR))
            .map(|mut d| d.next().is_none())
            .unwrap_or(true),
        "the partial copy is swept"
    );
}

/// The byte budget, likewise — and it is checked *before* the file is copied,
/// so the budget is a bound on what lands rather than a report on it.
#[test]
fn the_byte_budget_stops_a_tree_that_is_too_large() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");
    std::fs::write(dir.join("blob.bin"), vec![0u8; 4096]).unwrap();

    let budget = CopyBudget {
        max_bytes: 1024,
        ..CopyBudget::default()
    };
    let error = stage_blocking(&dir, &plugins, "notion", budget).unwrap_err();

    assert_eq!(error.code(), "invalid_path");
    assert!(
        error.to_string().contains("1024 bytes"),
        "the refusal names the budget that tripped: {error}"
    );
}

/// The defaults are the ones a real install runs with, and an ordinary plugin
/// tree is nowhere near them.
#[test]
fn the_default_budgets_pass_an_ordinary_plugin() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");
    let staged = stage_blocking(&dir, &plugins, "notion", CopyBudget::default()).expect("stage");
    assert!(staged.path().join("lib/helper.js").is_file());
}

// ── file types ───────────────────────────────────────────────────────────

/// **The hang.** `fs::copy` opens the source for reading, and opening a FIFO
/// with no writer never returns — so a `mkfifo` inside an unpacked third-party
/// plugin directory used to park the copy forever, with no timeout anywhere on
/// the path. The type is checked before the open; the timeout here is what
/// makes the assertion honest rather than decorative.
#[cfg(unix)]
#[tokio::test]
async fn a_fifo_in_the_source_is_refused_rather_than_opened() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");
    let fifo = dir.join("pipe");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo");
    assert!(made.success(), "mkfifo failed");

    let error = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        stage(&dir, &plugins, "notion"),
    )
    .await
    .expect("the copy must refuse a FIFO, not block on opening it")
    .unwrap_err();

    assert_eq!(error.code(), "invalid_path");
    assert!(
        error.to_string().contains("pipe") && error.to_string().contains("named pipe"),
        "the refusal names the entry and what it is: {error}"
    );
    assert!(
        std::fs::read_dir(plugins.join(STAGING_DIR))
            .map(|mut d| d.next().is_none())
            .unwrap_or(true),
        "the refused copy leaves nothing behind"
    );
}

/// The copy is `std::fs` throughout and the route runs its verb in a
/// `tokio::spawn`ed task — i.e. on a runtime worker. On a **current-thread**
/// runtime, a copy done inline would give no other task a chance to run; this
/// asserts one did, which is only true if the copy went to `spawn_blocking`.
#[tokio::test(flavor = "current_thread")]
async fn the_copy_runs_off_the_runtime_worker() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");
    for i in 0..300 {
        std::fs::write(dir.join(format!("lib/file-{i}.js")), "// x\n").unwrap();
    }

    let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&ran);
    tokio::spawn(async move { flag.store(true, std::sync::atomic::Ordering::SeqCst) });

    let staged = stage(&dir, &plugins, "notion").await.expect("stage");

    assert!(
        ran.load(std::sync::atomic::Ordering::SeqCst),
        "the runtime polled another task while the copy ran; it was not done inline"
    );
    assert!(staged.path().join("lib/file-299.js").is_file(), "and it copied everything");
}

// ── trash ────────────────────────────────────────────────────────────────

/// §1.3 rule 3: never `rm -rf` a directory the owner dropped in.
#[tokio::test]
async fn trashing_moves_the_directory_rather_than_deleting_it() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");
    let staged = stage(&dir, &plugins, "notion").await.expect("stage");
    staged.commit(&plugins.join("notion")).expect("commit");

    let moved = trash(&plugins, &plugins.join("notion"), "notion").expect("trash");

    assert!(!plugins.join("notion").exists());
    assert!(moved.join("plugin.toml").is_file(), "the tree is intact");
    assert!(moved.starts_with(plugins.join(TRASH_DIR)));
    assert!(
        moved
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("notion-")),
        "the trashed copy is stamped: {moved:?}"
    );
}

/// Two uninstalls of the same name in the same second must not collide.
#[tokio::test]
async fn two_trashed_copies_of_one_name_never_overwrite_each_other() {
    let (_tmp, plugins, sources) = roots();
    let mut seen = Vec::new();
    for _ in 0..2 {
        let dir = source(&sources, "notion", "notion");
        stage(&dir, &plugins, "notion").await
            .expect("stage")
            .commit(&plugins.join("notion"))
            .expect("commit");
        seen.push(trash(&plugins, &plugins.join("notion"), "notion").expect("trash"));
    }
    assert_ne!(seen[0], seen[1]);
    assert!(seen.iter().all(|p| p.join("plugin.toml").is_file()));
}

/// The scan takes every *subdirectory of the root* that holds a `plugin.toml`.
/// A staged or trashed copy is one level deeper, under a dot directory that
/// holds no manifest of its own, so neither is ever scanned as a plugin.
#[test]
fn the_store_directories_are_invisible_to_the_scan() {
    let (_tmp, plugins, _sources) = roots();
    for dir in [STAGING_DIR, TRASH_DIR, DATA_DIR] {
        std::fs::create_dir_all(plugins.join(dir).join("notion")).unwrap();
        std::fs::write(plugins.join(dir).join("notion/plugin.toml"), "").unwrap();
        assert!(
            !plugins.join(dir).join("plugin.toml").exists(),
            "{dir} must never itself look like a plugin"
        );
    }
}

// ── the swap (N-PLUG-4) ──────────────────────────────────────────────────

/// An installed `notion` at version `1.0.0`, plus a staged replacement.
fn installed_and_staged(plugins: &Path, sources: &Path) -> Staged {
    let first = source(sources, "notion", "notion");
    stage_blocking(&first, plugins, "notion", CopyBudget::default())
        .expect("stage")
        .commit(&plugins.join("notion"))
        .expect("commit");
    let next = sources.join("next");
    std::fs::create_dir_all(&next).unwrap();
    std::fs::write(next.join("plugin.toml"), "version two").unwrap();
    stage_blocking(&next, plugins, "notion", CopyBudget::default()).expect("stage")
}

fn trash_entries(plugins: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(plugins.join(TRASH_DIR))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// A replace that commits moves the incumbent to the trash, puts the staged
/// tree in its name and leaves no marker.
#[test]
fn a_replace_that_commits_trashes_the_incumbent_and_drops_its_marker() {
    let (_tmp, plugins, sources) = roots();
    let staged = installed_and_staged(&plugins, &sources);

    replace(&plugins, "notion", staged).expect("replace");

    assert_eq!(
        std::fs::read_to_string(plugins.join("notion/plugin.toml")).unwrap(),
        "version two"
    );
    let trashed = trash_entries(&plugins);
    assert_eq!(trashed.len(), 1);
    assert!(
        trashed[0].join("lib/helper.js").is_file(),
        "kept, never deleted"
    );
    assert!(!swap_marker(&plugins, "notion").exists());
}

/// The commit step fails: the incumbent is renamed back before the error
/// returns, and neither the marker nor the staged copy is left.
#[test]
fn a_replace_whose_commit_fails_puts_the_incumbent_back() {
    let (_tmp, plugins, sources) = roots();
    let staged = installed_and_staged(&plugins, &sources);
    let mut marker_seen = false;

    let error = replace_with(&plugins, "notion", staged, |staged, _dest| {
        // The marker is on disk while the commit runs.
        marker_seen = swap_marker(&plugins, "notion").is_file();
        drop(staged);
        Err(InstallError::Io("cannot move into place: refused".into()))
    })
    .unwrap_err();

    assert!(marker_seen, "the swap is recorded before the commit");
    assert_eq!(error.error.code(), "copy_failed");
    assert!(error.incumbent_in_place, "and the failure says it is back");
    assert!(
        plugins.join("notion/lib/helper.js").is_file(),
        "the incumbent is back"
    );
    assert!(trash_entries(&plugins).is_empty());
    assert!(!swap_marker(&plugins, "notion").exists());
    assert!(
        std::fs::read_dir(plugins.join(STAGING_DIR))
            .unwrap()
            .next()
            .is_none(),
        "no staging copy and no marker"
    );
}

/// Something took the name between the move and the commit: the incumbent
/// cannot go back over it, so it stays in the trash, the error says where —
/// and the marker goes, so no later start puts the tree back over whatever
/// then frees the name (an uninstall of what took it).
#[test]
fn a_name_taken_during_the_swap_keeps_the_incumbent_and_drops_the_marker() {
    let (_tmp, plugins, sources) = roots();
    let staged = installed_and_staged(&plugins, &sources);

    let failed = replace_with(&plugins, "notion", staged, |staged, dest| {
        drop(staged);
        // An out-of-process write lands a tree under the vacated name.
        std::fs::create_dir_all(dest).unwrap();
        std::fs::write(dest.join("plugin.toml"), "by hand").unwrap();
        Err(InstallError::Io("cannot move into place: taken".into()))
    })
    .unwrap_err();

    assert!(!failed.incumbent_in_place);
    let trashed = trash_entries(&plugins);
    assert_eq!(trashed.len(), 1);
    assert!(
        trashed[0].join("lib/helper.js").is_file(),
        "kept, never deleted"
    );
    assert!(
        failed
            .error
            .to_string()
            .contains(&trashed[0].display().to_string()),
        "the error says where the tree was kept: {}",
        failed.error
    );
    assert_eq!(
        std::fs::read_to_string(plugins.join("notion/plugin.toml")).unwrap(),
        "by hand"
    );
    assert!(
        !swap_marker(&plugins, "notion").exists(),
        "no marker is left to resurrect the tree later"
    );
}

/// The uninstall and the orphan row's removal drop a marker an interrupted
/// update left, and only that: the trashed tree stays where it is.
#[test]
fn dropping_a_swap_marker_moves_nothing() {
    let (_tmp, plugins, _sources) = roots();
    let trashed = plugins
        .join(TRASH_DIR)
        .join("notion-20261017T000000.000000000Z");
    std::fs::create_dir_all(&trashed).unwrap();
    std::fs::create_dir_all(plugins.join(STAGING_DIR)).unwrap();
    std::fs::write(
        swap_marker(&plugins, "notion"),
        ".trash/notion-20261017T000000.000000000Z\n",
    )
    .unwrap();

    drop_swap_marker(&plugins, "notion");
    drop_swap_marker(&plugins, "absent");

    assert!(!swap_marker(&plugins, "notion").exists());
    assert!(trashed.is_dir(), "the tree stays in the trash");
    assert!(swap_markers(&plugins).is_empty());
}

/// A marker whose commit landed — the name is taken — moves nothing: the
/// old tree stays in the trash and the new one in place. Recovery never drags
/// an old tree back over a finished update.
#[test]
fn recovery_never_moves_the_old_tree_back_over_a_landed_commit() {
    let (_tmp, plugins, sources) = roots();
    let staged = installed_and_staged(&plugins, &sources);
    replace(&plugins, "notion", staged).expect("replace");
    let trashed = trash_entries(&plugins).remove(0);
    // The crash came after the commit and before the marker was removed.
    write_marker(
        &swap_marker(&plugins, "notion"),
        &Path::new(TRASH_DIR).join(trashed.file_name().unwrap()),
    )
    .unwrap();

    assert_eq!(swap_markers(&plugins), vec!["notion".to_string()]);
    assert_eq!(
        recover_swap(&plugins, "notion").unwrap(),
        SwapRecovery::Settled
    );

    assert_eq!(
        std::fs::read_to_string(plugins.join("notion/plugin.toml")).unwrap(),
        "version two"
    );
    assert!(
        trashed.join("lib/helper.js").is_file(),
        "the old tree stays kept"
    );
    assert!(!swap_marker(&plugins, "notion").exists());
}

/// A marker that names anything but a directory inside `.trash/` moves
/// nothing and is left for a person to read.
#[test]
fn a_marker_that_leaves_the_trash_is_left_alone() {
    let (_tmp, plugins, _sources) = roots();
    std::fs::create_dir_all(plugins.join(STAGING_DIR)).unwrap();
    for line in ["../elsewhere", "/abs/path", ".trash/../../x", "notion-2026"] {
        std::fs::write(swap_marker(&plugins, "notion"), format!("{line}\n")).unwrap();
        assert!(
            matches!(
                recover_swap(&plugins, "notion").unwrap(),
                SwapRecovery::Unreadable { .. }
            ),
            "{line}"
        );
        assert!(
            swap_marker(&plugins, "notion").is_file(),
            "{line}: left in place"
        );
    }
    assert!(!plugins.join("notion").exists(), "and nothing moved");
}

/// The boot sweep takes an earlier process's staging copies only — never
/// this process's, never a marker, never a name that is not a staging copy.
#[test]
fn the_staging_sweep_takes_only_an_earlier_processs_copies() {
    let (_tmp, plugins, _sources) = roots();
    let staging = plugins.join(STAGING_DIR);
    let stamp = "20261017T120000.123456789Z";
    let theirs = staging.join(format!("notion.1.{stamp}"));
    let dotted = staging.join(format!("my.plugin.1.{stamp}"));
    let ours = staging.join(format!("notion.{}.{stamp}", std::process::id()));
    let stranger = staging.join("notes-by-hand");
    for dir in [&theirs, &dotted, &ours, &stranger] {
        std::fs::create_dir_all(dir.join("lib")).unwrap();
    }
    std::fs::write(swap_marker(&plugins, "notion"), ".trash/notion-x\n").unwrap();

    assert_eq!(sweep_stale_staging(&plugins), 2);

    assert!(
        !theirs.exists() && !dotted.exists(),
        "an earlier process's copies go"
    );
    assert!(ours.exists(), "a copy this process is making stays");
    assert!(stranger.exists(), "a name that is not a staging copy stays");
    assert!(swap_marker(&plugins, "notion").is_file(), "a marker stays");
    assert_eq!(staging_pid(&format!("notion.42.{stamp}")), Some(42));
    assert_eq!(staging_pid("notion.42"), None);
}
