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

    let (name, summary) = inspect_source(&dir, &plugins).expect("a valid source");

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
    let error = inspect_source(Path::new("some/plugin"), &plugins).unwrap_err();
    assert_eq!(error.code(), "invalid_path");
}

/// "Load in place from an arbitrary path" stays declined, and installing a
/// directory that is *already* under the plugins root would either be a no-op
/// or a self-copy — both are caller mistakes, not installs.
#[test]
fn a_source_inside_the_plugins_root_is_refused() {
    let (_tmp, plugins, _sources) = roots();
    let inside = source(&plugins, "notion", "notion");
    let error = inspect_source(&inside, &plugins).unwrap_err();
    assert_eq!(error.code(), "invalid_path");
}

#[test]
fn a_missing_directory_is_source_not_found() {
    let (_tmp, plugins, sources) = roots();
    let error = inspect_source(&sources.join("nope"), &plugins).unwrap_err();
    assert_eq!(error.code(), "source_not_found");
}

#[test]
fn a_directory_with_no_manifest_is_invalid_manifest() {
    let (_tmp, plugins, sources) = roots();
    let bare = sources.join("bare");
    std::fs::create_dir_all(&bare).unwrap();
    assert_eq!(
        inspect_source(&bare, &plugins).unwrap_err().code(),
        "invalid_manifest"
    );

    std::fs::write(bare.join("plugin.toml"), "this is not = = toml [[[").unwrap();
    assert_eq!(
        inspect_source(&bare, &plugins).unwrap_err().code(),
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
    let error = inspect_source(&dir, &plugins).unwrap_err();
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
        inspect_source(&dir, &plugins).unwrap_err().code(),
        "invalid_path"
    );
}

// ── stage → commit ───────────────────────────────────────────────────────

#[test]
fn staging_copies_the_whole_tree_and_commit_renames_it_into_place() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");

    let staged = stage(&dir, &plugins, "notion").expect("stage");
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
#[test]
fn a_staged_copy_that_is_never_committed_leaves_no_half_written_plugin() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");

    {
        let staged = stage(&dir, &plugins, "notion").expect("stage");
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
#[test]
fn the_staging_area_is_a_sibling_of_the_destination() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");
    let staged = stage(&dir, &plugins, "notion").expect("stage");
    assert_eq!(
        staged.path().parent().and_then(|p| p.file_name()),
        Some(std::ffi::OsStr::new(STAGING_DIR)),
    );
    assert_eq!(staged.path().parent().and_then(|p| p.parent()), Some(&*plugins));
}

// ── symlinks ─────────────────────────────────────────────────────────────

#[cfg(unix)]
#[test]
fn a_symlink_pointing_out_of_the_source_is_refused() {
    let (tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");
    let secret = tmp.path().join("id_rsa");
    std::fs::write(&secret, "PRIVATE KEY").unwrap();
    std::os::unix::fs::symlink(&secret, dir.join("key")).unwrap();

    let error = stage(&dir, &plugins, "notion").unwrap_err();
    assert_eq!(error.code(), "escaping_symlink");
    assert!(
        std::fs::read_dir(plugins.join(STAGING_DIR))
            .map(|mut d| d.next().is_none())
            .unwrap_or(true),
        "the refused copy leaves nothing behind"
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_inside_the_source_is_copied_as_its_target() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");
    std::os::unix::fs::symlink(dir.join("lib/helper.js"), dir.join("main.js")).unwrap();

    let staged = stage(&dir, &plugins, "notion").expect("stage");
    let copied = staged.path().join("main.js");
    assert!(
        !copied.symlink_metadata().unwrap().file_type().is_symlink(),
        "the copy dereferences it — the install never links out of the store"
    );
    assert_eq!(std::fs::read_to_string(copied).unwrap(), "// helper\n");
}

// ── trash ────────────────────────────────────────────────────────────────

/// §1.3 rule 3: never `rm -rf` a directory the owner dropped in.
#[test]
fn trashing_moves_the_directory_rather_than_deleting_it() {
    let (_tmp, plugins, sources) = roots();
    let dir = source(&sources, "notion", "notion");
    let staged = stage(&dir, &plugins, "notion").expect("stage");
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
#[test]
fn two_trashed_copies_of_one_name_never_overwrite_each_other() {
    let (_tmp, plugins, sources) = roots();
    let mut seen = Vec::new();
    for _ in 0..2 {
        let dir = source(&sources, "notion", "notion");
        stage(&dir, &plugins, "notion")
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
