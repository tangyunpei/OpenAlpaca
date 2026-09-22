use super::*;
use tempfile::tempdir;

fn touch(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

#[test]
fn a_project_store_moves_by_one_rename() {
    let tmp = tempdir().unwrap();
    let old = tmp.path().join("old-project");
    let new = tmp.path().join("new-project");
    touch(&old.join(".openalpaca").join("artifacts").join("a.md"), "a");

    assert_eq!(
        plan_project_store_move(&old, &new).unwrap(),
        StoreMove::Rename
    );
    assert!(move_project_store(&old, &new).unwrap());

    assert!(!old.join(".openalpaca").exists());
    assert_eq!(
        fs::read_to_string(new.join(".openalpaca").join("artifacts").join("a.md")).unwrap(),
        "a"
    );
    // The rest of the old project is the user's; the mover touches nothing else.
    assert!(old.exists());
}

#[test]
fn a_store_already_at_the_new_root_is_nothing_to_move() {
    let tmp = tempdir().unwrap();
    let old = tmp.path().join("old-project");
    let new = tmp.path().join("new-project");
    // The ordinary way a project moves: the whole directory went with it, so
    // only the rows are left to re-base.
    touch(&new.join(".openalpaca").join(".layout"), "1\n");

    assert_eq!(
        plan_project_store_move(&old, &new).unwrap(),
        StoreMove::NothingToMove
    );
    assert!(!move_project_store(&old, &new).unwrap());
    assert!(new.join(".openalpaca").join(".layout").exists());
}

#[test]
fn two_stores_are_refused_rather_than_merged() {
    let tmp = tempdir().unwrap();
    let old = tmp.path().join("old-project");
    let new = tmp.path().join("new-project");
    touch(&old.join(".openalpaca").join("keep.md"), "old");
    touch(&new.join(".openalpaca").join("keep.md"), "new");

    assert_eq!(
        plan_project_store_move(&old, &new).unwrap(),
        StoreMove::Ambiguous
    );
    let err = move_project_store(&old, &new).unwrap_err();
    assert!(
        err.to_string().contains("two stores"),
        "expected the refusal, got: {err}"
    );
    // Neither side lost anything.
    assert_eq!(
        fs::read_to_string(old.join(".openalpaca").join("keep.md")).unwrap(),
        "old"
    );
    assert_eq!(
        fs::read_to_string(new.join(".openalpaca").join("keep.md")).unwrap(),
        "new"
    );
}

#[test]
fn a_project_with_no_store_is_nothing_to_move() {
    let tmp = tempdir().unwrap();
    assert_eq!(
        plan_project_store_move(&tmp.path().join("a"), &tmp.path().join("b")).unwrap(),
        StoreMove::NothingToMove
    );
}
