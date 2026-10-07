//! Storage design §3 *Resolve* (Stage 1 S2): the walk from a client path to
//! its project — a registered root, else a folder holding `.git`, the nearest
//! winning — R13's home rule and R10's choice. Every test runs under its own
//! home store.

use super::*;
use crate::store::tests::HomeStoreGuard;
use std::fs;
use tempfile::{TempDir, tempdir};

/// One temp root: the home store at `<tmp>/home/.openalpaca` (so `<tmp>/home`
/// stands in for `$HOME`) and the folders a client names under `<tmp>/work`.
/// A walk from any of them stops at `<tmp>`, which contains the home store
/// (R13), so nothing above the temp root can answer.
struct Scene {
    _guard: HomeStoreGuard,
    tmp: TempDir,
}

impl Scene {
    fn new() -> Self {
        let tmp = tempdir().unwrap();
        let home_store = tmp
            .path()
            .canonicalize()
            .unwrap()
            .join("home")
            .join(".openalpaca");
        fs::create_dir_all(&home_store).unwrap();
        let guard = HomeStoreGuard::set(&home_store);
        Self { _guard: guard, tmp }
    }

    fn root(&self) -> PathBuf {
        self.tmp.path().canonicalize().unwrap()
    }

    fn home(&self) -> PathBuf {
        self.root().join("home")
    }

    fn home_store(&self) -> PathBuf {
        self.home().join(".openalpaca")
    }

    /// `<tmp>/work/<rel>`, created.
    fn dir(&self, rel: &str) -> PathBuf {
        let dir = self.root().join("work").join(rel);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A git repository at `<tmp>/work/<rel>`: a `.git` directory in it.
    fn repo(&self, rel: &str) -> PathBuf {
        let dir = self.dir(rel);
        fs::create_dir(dir.join(".git")).unwrap();
        dir
    }
}

#[test]
fn a_folder_inside_a_git_repository_resolves_to_the_repository_unregistered() {
    let s = Scene::new();
    let repo = s.repo("app");
    let deep = s.dir("app/src/lib");

    let found = resolve(&deep).unwrap().expect("inside a repository");
    assert_eq!(found.root, repo);
    assert!(!found.is_registered(), "nobody has used it yet (R12)");
    assert_eq!(found.root_string(), repo.to_string_lossy());
    assert_eq!(resolve(&repo).unwrap().map(|r| r.root), Some(repo));
}

#[test]
fn a_git_file_makes_a_worktree_its_own_project() {
    let s = Scene::new();
    s.repo("main");
    let worktree = s.dir("main/worktrees/feature");
    fs::write(
        worktree.join(".git"),
        "gitdir: ../../.git/worktrees/feature\n",
    )
    .unwrap();
    let inside = s.dir("main/worktrees/feature/src");

    let found = resolve(&inside).unwrap().expect("a worktree is a project");
    assert_eq!(
        found.root, worktree,
        "the nearest `.git`, a file here, wins"
    );
}

#[test]
fn a_folder_with_no_git_and_no_registration_is_no_project() {
    let s = Scene::new();
    assert_eq!(resolve(&s.dir("notes/2026")).unwrap(), None);
}

#[test]
fn an_openalpaca_folder_is_not_a_project_marker() {
    let s = Scene::new();
    let old = s.dir("old-project");
    fs::create_dir(old.join(".openalpaca")).unwrap();
    assert_eq!(
        resolve(&old).unwrap(),
        None,
        "ST-9: an in-project store is never read, and it marks nothing"
    );
}

#[test]
fn a_registered_folder_resolves_itself_and_every_folder_below_it() {
    let s = Scene::new();
    let notes = s.dir("notes");
    let registered = register(&notes).unwrap();
    let below = s.dir("notes/2026/march");

    for path in [&notes, &below] {
        let found = resolve(path).unwrap().expect("a registered project");
        assert_eq!(found.root, notes);
        assert_eq!(found.project.as_ref().map(|p| p.id), Some(registered.id));
    }
}

#[test]
fn the_nearest_registered_root_or_repository_wins() {
    let s = Scene::new();
    // A folder registered inside a repository answers for itself; the rest
    // of the repository is still the repository's.
    let mono = s.repo("mono");
    let web = s.dir("mono/packages/web");
    register(&web).unwrap();
    let found = resolve(&s.dir("mono/packages/web/src"))
        .unwrap()
        .expect("project");
    assert_eq!(found.root, web);
    assert!(found.is_registered());
    let found = resolve(&s.dir("mono/packages/api/src"))
        .unwrap()
        .expect("project");
    assert_eq!(found.root, mono);
    assert!(!found.is_registered());

    // And a repository inside a registered folder answers for itself.
    let notes = s.dir("notes");
    register(&notes).unwrap();
    let code = s.repo("notes/code");
    let found = resolve(&s.dir("notes/code/src")).unwrap().expect("project");
    assert_eq!(found.root, code);
    assert!(!found.is_registered());
}

#[test]
fn a_registered_repository_resolves_as_registered() {
    let s = Scene::new();
    let repo = s.repo("app");
    let registered = register(&repo).unwrap();

    let found = resolve(&s.dir("app/src")).unwrap().expect("project");
    assert_eq!(found.root, repo);
    assert_eq!(found.project, Some(registered));
}

#[test]
fn a_missing_registered_project_still_resolves_to_itself() {
    let s = Scene::new();
    let repo = s.repo("gone");
    let registered = register(&repo).unwrap();
    fs::remove_dir_all(&repo).unwrap();

    for path in [repo.clone(), repo.join("src").join("lib")] {
        let found = resolve(&path)
            .unwrap()
            .expect("ST-7: a missing project keeps resolving");
        assert_eq!(found.root, repo);
        assert_eq!(found.project.as_ref().map(|p| p.id), Some(registered.id));
        assert!(found.project.as_ref().is_some_and(|p| p.missing));
    }
}

#[test]
fn a_missing_path_is_matched_against_registered_roots_only() {
    let s = Scene::new();
    let repo = s.repo("app");
    // The folder a CLI was started in, deleted since: it cannot be
    // canonicalised, so only a registered root above it answers — the
    // repository's `.git` is not consulted (§3).
    let gone = repo.join("deleted").join("sub");
    assert_eq!(resolve(&gone).unwrap(), None);

    register(&repo).unwrap();
    assert_eq!(resolve(&gone).unwrap().map(|r| r.root), Some(repo));
}

#[test]
fn the_home_store_its_insides_and_the_folders_around_it_are_no_project() {
    let s = Scene::new();
    // `$HOME` stays no project even with a `~/.git`, and so does a repository
    // the home store sits inside (R13).
    fs::create_dir(s.home().join(".git")).unwrap();
    fs::create_dir(s.root().join(".git")).unwrap();
    let documents = s.home().join("Documents");
    fs::create_dir(&documents).unwrap();
    let inside = s.home_store().join("artifacts");
    fs::create_dir(&inside).unwrap();

    for path in [s.home(), documents, s.home_store(), inside, s.root()] {
        assert_eq!(resolve(&path).unwrap(), None, "{}", path.display());
    }
}

#[test]
fn a_relative_path_is_no_project_even_where_the_working_directory_has_one() {
    let _s = Scene::new();
    // Cargo runs a test from its package directory, inside this repository's
    // checkout: read against the working directory, `src` would resolve to
    // the checkout (R22).
    assert!(Path::new("src").is_dir());
    assert_eq!(resolve(Path::new("src")).unwrap(), None);
    assert_eq!(resolve(Path::new("")).unwrap(), None);
}

#[test]
fn resolving_creates_nothing_under_the_home_store() {
    let s = Scene::new();
    let repo = s.repo("app");
    resolve(&repo).unwrap();
    resolve(&repo.join("missing")).unwrap();
    resolve(&s.dir("loose")).unwrap();
    assert!(
        !projects_dir().unwrap().exists(),
        "a resolution creates nothing"
    );
}
