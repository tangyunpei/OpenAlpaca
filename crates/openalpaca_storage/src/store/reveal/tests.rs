use super::*;
use crate::store::tests::HomeStoreGuard;
use std::fs;
use tempfile::tempdir;

/// `<dir>/<rel>` with `contents`, its parents created.
fn file_at(dir: &Path, rel: &str) -> PathBuf {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "contents").unwrap();
    path
}

#[test]
fn a_file_in_a_projects_folder_is_revealed_by_its_canonical_path() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("isolated-store");
    let file = file_at(
        &home,
        "projects/-Users-me-dev-app/artifacts/reports/01-pasta-night.md",
    );

    let revealed = revealable_path_with_home(&file, Some(&home)).unwrap();

    assert_eq!(revealed, file.canonicalize().unwrap());
}

#[test]
fn a_file_under_an_overridden_home_root_is_revealed() {
    let tmp = tempdir().unwrap();
    // The override names no `.openalpaca` directory, and is given as written
    // (on macOS `/var/…`, which canonicalises to `/private/var/…`).
    let home = tmp.path().join("isolated-store");
    let file = file_at(&home, "artifacts/01-notes.md");
    let _env = HomeStoreGuard::set(&home);

    let revealed = revealable_path(&file).unwrap();

    assert_eq!(revealed, file.canonicalize().unwrap());
}

#[test]
fn a_file_outside_every_store_is_refused() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("isolated-store");
    fs::create_dir_all(&home).unwrap();
    let file = file_at(tmp.path(), "Documents/notes.md");

    let refusal = revealable_path_with_home(&file, Some(&home)).unwrap_err();

    assert_eq!(
        refusal,
        RevealRefusal::OutsideStore(file.canonicalize().unwrap())
    );
    assert!(
        refusal
            .to_string()
            .ends_with("is not in an OpenAlpaca store, so it is not shown in its folder."),
        "{refusal}"
    );
}

#[test]
fn a_directory_in_a_store_is_refused() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("isolated-store");
    let dir = home.join("projects/-Users-me-dev-app/artifacts/reports");
    fs::create_dir_all(&dir).unwrap();

    let refusal = revealable_path_with_home(&dir, Some(&home)).unwrap_err();

    assert_eq!(
        refusal,
        RevealRefusal::NotAFile(dir.canonicalize().unwrap())
    );
}

#[test]
fn a_missing_file_is_refused_with_the_path_it_was_asked_for() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("isolated-store");
    fs::create_dir_all(&home).unwrap();
    let gone = home.join("artifacts/01-gone.md");

    let refusal = revealable_path_with_home(&gone, Some(&home)).unwrap_err();

    assert_eq!(refusal, RevealRefusal::Missing(gone.clone()));
    assert_eq!(
        refusal.to_string(),
        format!(
            "{} is not there any more, so it cannot be shown in its folder.",
            gone.display()
        )
    );
}

// The shell always has a home root (`home_root()` is `Some` on any machine with
// a home directory), so the refusals are pinned with one supplied too: the
// home clause must admit what is under the home root and nothing beside it.

#[test]
fn a_file_outside_the_home_root_is_refused_when_a_home_root_is_known() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("isolated-store");
    fs::create_dir_all(&home).unwrap();
    let outside = file_at(tmp.path(), "Documents/notes.md");
    // A sibling whose name only starts like the home root is outside it too.
    let sibling = file_at(tmp.path(), "isolated-store-evil/notes.md");

    for file in [outside, sibling] {
        assert_eq!(
            revealable_path_with_home(&file, Some(&home)),
            Err(RevealRefusal::OutsideStore(file.canonicalize().unwrap()))
        );
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_under_the_home_root_that_leads_outside_it_is_refused() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("isolated-store");
    let outside = file_at(tmp.path(), "Documents/secret.md");
    let link = home.join("artifacts/01-link.md");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &link).unwrap();

    assert_eq!(
        revealable_path_with_home(&link, Some(&home)),
        Err(RevealRefusal::OutsideStore(outside.canonicalize().unwrap()))
    );
}

#[cfg(unix)]
#[test]
fn a_home_root_that_is_a_symlink_admits_its_files() {
    // Pins the home root's canonicalisation on every platform, not only where
    // the temp directory happens to be a symlink.
    let tmp = tempdir().unwrap();
    let file = file_at(&tmp.path().join("volume/store"), "artifacts/01-notes.md");
    let home = tmp.path().join("home-link");
    std::os::unix::fs::symlink(tmp.path().join("volume/store"), &home).unwrap();

    assert_eq!(
        revealable_path_with_home(&home.join("artifacts/01-notes.md"), Some(&home)),
        Ok(file.canonicalize().unwrap())
    );
}

/// ST-9: an old in-project `.openalpaca/` is no store any more — a file in
/// one is outside the home root and refused, however its folder is named.
#[test]
fn a_file_in_an_old_in_project_store_is_refused() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("isolated-store");
    fs::create_dir_all(&home).unwrap();
    let file = file_at(tmp.path(), "dev/app/.openalpaca/artifacts/01-notes.md");

    assert_eq!(
        revealable_path_with_home(&file, Some(&home)),
        Err(RevealRefusal::OutsideStore(file.canonicalize().unwrap()))
    );
}

/// With no home root to compare with, nothing is in a store.
#[test]
fn with_no_home_root_nothing_is_revealed() {
    let tmp = tempdir().unwrap();
    let file = file_at(tmp.path(), ".openalpaca/artifacts/01-notes.md");

    assert_eq!(
        revealable_path_with_home(&file, None),
        Err(RevealRefusal::OutsideStore(file.canonicalize().unwrap()))
    );
}
