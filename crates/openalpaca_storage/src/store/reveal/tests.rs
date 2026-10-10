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

/// The refusal's sentence for the resolved `path`, as the toast shows it.
fn outside_sentence(path: &Path) -> String {
    format!(
        "{} is not in an OpenAlpaca store or one of your projects, so it is not shown in its folder.",
        path.display()
    )
}

#[test]
fn a_file_in_a_projects_folder_is_revealed_by_its_canonical_path() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("isolated-store");
    let file = file_at(
        &home,
        "projects/-Users-me-dev-app/artifacts/reports/01-pasta-night.md",
    );

    let revealed = revealable_path_among(&file, Some(&home), &[]).unwrap();

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

    let refusal = revealable_path_among(&file, Some(&home), &[]).unwrap_err();

    let canonical = file.canonicalize().unwrap();
    assert_eq!(refusal, RevealRefusal::OutsideStore(canonical.clone()));
    assert_eq!(refusal.to_string(), outside_sentence(&canonical));
}

#[test]
fn a_directory_in_a_store_is_refused() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("isolated-store");
    let dir = home.join("projects/-Users-me-dev-app/artifacts/reports");
    fs::create_dir_all(&dir).unwrap();

    let refusal = revealable_path_among(&dir, Some(&home), &[]).unwrap_err();

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

    let refusal = revealable_path_among(&gone, Some(&home), &[]).unwrap_err();

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
            revealable_path_among(&file, Some(&home), &[]),
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
        revealable_path_among(&link, Some(&home), &[]),
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
        revealable_path_among(&home.join("artifacts/01-notes.md"), Some(&home), &[]),
        Ok(file.canonicalize().unwrap())
    );
}

/// ST-9: an old in-project `.openalpaca/` is no store any more — a file in
/// one of a folder no project records is refused, however its folder is
/// named.
#[test]
fn a_file_in_an_old_in_project_store_is_refused() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("isolated-store");
    fs::create_dir_all(&home).unwrap();
    let file = file_at(tmp.path(), "dev/app/.openalpaca/artifacts/01-notes.md");

    assert_eq!(
        revealable_path_among(&file, Some(&home), &[]),
        Err(RevealRefusal::OutsideStore(file.canonicalize().unwrap()))
    );
}

/// With no home root and no project to compare with, nothing is in a store.
#[test]
fn with_no_home_root_nothing_is_revealed() {
    let tmp = tempdir().unwrap();
    let file = file_at(tmp.path(), ".openalpaca/artifacts/01-notes.md");

    assert_eq!(
        revealable_path_among(&file, None, &[]),
        Err(RevealRefusal::OutsideStore(file.canonicalize().unwrap()))
    );
}

/// ST-12, ST-18: a file an agent wrote into a project lives under the
/// project's own root, and the root `projects/` records admits it.
#[test]
fn a_file_under_a_registered_projects_root_is_revealed() {
    let tmp = tempdir().unwrap();
    let project = tmp.path().join("dev/app");
    let file = file_at(&project, "src/main.rs");
    let _env = HomeStoreGuard::set(&tmp.path().join("isolated-store"));
    crate::store::projects::register(&project).unwrap();

    assert_eq!(revealable_path(&file), Ok(file.canonicalize().unwrap()));
}

/// A folder `projects/` does not record is no project: its files are refused,
/// with the sentence the toast shows.
#[test]
fn a_file_in_a_folder_no_project_records_is_refused() {
    let tmp = tempdir().unwrap();
    let file = file_at(tmp.path(), "dev/app/src/main.rs");
    let _env = HomeStoreGuard::set(&tmp.path().join("isolated-store"));

    let refusal = revealable_path(&file).unwrap_err();

    let canonical = file.canonicalize().unwrap();
    assert_eq!(refusal, RevealRefusal::OutsideStore(canonical.clone()));
    assert_eq!(refusal.to_string(), outside_sentence(&canonical));
}

#[test]
fn a_project_root_admits_its_files_and_nothing_beside_it() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("isolated-store");
    fs::create_dir_all(&home).unwrap();
    let root = tmp.path().join("dev/app");
    let inside = file_at(&root, "docs/notes.md");
    // A sibling whose name only starts like the root is outside it.
    let sibling = file_at(tmp.path(), "dev/app-evil/notes.md");

    assert_eq!(
        revealable_path_among(&inside, Some(&home), std::slice::from_ref(&root)),
        Ok(inside.canonicalize().unwrap())
    );
    assert_eq!(
        revealable_path_among(&sibling, Some(&home), &[root]),
        Err(RevealRefusal::OutsideStore(sibling.canonicalize().unwrap()))
    );
}

/// ST-18: a result file whose kind the store does not keep is offered Reveal
/// alone, so Reveal must answer for it — whatever its name, and with no home
/// root to compare with.
#[test]
fn a_file_no_artifact_kind_keeps_is_revealed_in_its_project() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("dev/app");
    for rel in ["Makefile", "scripts/release.sh", ".env", "bin/tool.exe"] {
        let file = file_at(&root, rel);
        assert_eq!(
            revealable_path_among(&file, None, std::slice::from_ref(&root)),
            Ok(file.canonicalize().unwrap()),
            "{rel}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_in_a_project_that_leads_outside_every_root_is_refused() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("isolated-store");
    fs::create_dir_all(&home).unwrap();
    let root = tmp.path().join("dev/app");
    let outside = file_at(tmp.path(), "Documents/secret.md");
    let link = root.join("notes/link.md");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &link).unwrap();

    assert_eq!(
        revealable_path_among(&link, Some(&home), &[root]),
        Err(RevealRefusal::OutsideStore(outside.canonicalize().unwrap()))
    );
}

/// The lead's ruling R-T18 (ST-23): a project whose folder moved with a link
/// left at its old root is missing, and its root admits nothing — a file
/// asked for through the link resolves to the moved folder, which no project
/// records, and is refused.
#[cfg(unix)]
#[test]
fn a_project_moved_behind_a_link_admits_no_file_through_its_old_root() {
    let tmp = tempdir().unwrap();
    let project = tmp.path().join("dev/app");
    let file = file_at(&project, "src/main.rs");
    let _env = HomeStoreGuard::set(&tmp.path().join("isolated-store"));
    crate::store::projects::register(&project).unwrap();
    let moved = tmp.path().join("dev/app-moved");
    fs::rename(&project, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &project).unwrap();

    let canonical = file.canonicalize().unwrap();
    assert_eq!(canonical, moved.join("src/main.rs").canonicalize().unwrap());
    assert_eq!(
        revealable_path(&file),
        Err(RevealRefusal::OutsideStore(canonical))
    );
}
