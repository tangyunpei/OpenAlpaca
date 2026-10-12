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

/// R13 with a home store reached through a link (Stage 1 review R4):
/// `~/.openalpaca` leads to another volume and `$HOME` holds a dotfiles
/// `.git`. `$HOME`, and a folder under it with no repository of its own, stay
/// no project, and `$HOME` cannot be chosen; choosing the folder makes it a
/// project of its own. The store is judged as it is named — its parent
/// resolved, its last component not followed — as well as where it leads.
#[cfg(unix)]
#[test]
fn a_home_store_reached_through_a_link_keeps_the_folder_around_its_name_no_project() {
    let tmp = tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("home");
    let elsewhere = base.join("volume").join("oa");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();
    let link = home.join(".openalpaca");
    std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
    let _guard = HomeStoreGuard::set(&link);
    fs::create_dir(home.join(".git")).unwrap();
    let notes = home.join("notes");
    fs::create_dir(&notes).unwrap();

    for path in [&home, &notes] {
        assert_eq!(resolve(path).unwrap(), None, "{}", path.display());
    }
    let err = choose(&home).unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<ProjectError>(),
            Some(ProjectError::IsHome { .. })
        ),
        "{err:#}"
    );
    assert_eq!(
        choose(&notes).unwrap().root,
        notes,
        "R10: the folder itself"
    );
    assert_eq!(list().unwrap().len(), 1, "$HOME was never recorded");
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

#[test]
fn resolve_and_register_records_an_unused_repository_once() {
    let s = Scene::new();
    let repo = s.repo("app");

    let first = resolve_and_register(&s.dir("app/src"))
        .unwrap()
        .expect("a project");
    assert_eq!(first.root, repo);
    assert_eq!(lookup(&repo).unwrap().map(|p| p.id), Some(first.id));

    let again = resolve_and_register(&repo).unwrap().expect("a project");
    assert_eq!(
        again.id, first.id,
        "recorded once; the second call finds it"
    );
    assert_eq!(list().unwrap().len(), 1);
}

#[test]
fn resolve_and_register_records_nothing_for_a_path_with_no_project() {
    let s = Scene::new();
    assert_eq!(resolve_and_register(&s.dir("loose")).unwrap(), None);
    assert_eq!(resolve_and_register(&s.home()).unwrap(), None);
    assert_eq!(resolve_and_register(Path::new("relative")).unwrap(), None);
    assert!(!projects_dir().unwrap().exists());
}

#[test]
fn resolve_and_register_returns_a_missing_registered_project_as_it_is() {
    let s = Scene::new();
    let repo = s.repo("gone");
    let registered = register(&repo).unwrap();
    fs::remove_dir_all(&repo).unwrap();

    let found = resolve_and_register(&repo).unwrap().expect("ST-7");
    assert_eq!(found.id, registered.id);
    assert!(found.missing);
    assert_eq!(list().unwrap().len(), 1, "nothing new recorded");
}

#[test]
fn choosing_a_folder_inside_a_repository_chooses_the_repository() {
    let s = Scene::new();
    let repo = s.repo("app");
    let chosen = choose(&s.dir("app/src")).unwrap();
    assert_eq!(chosen.root, repo, "R10: inside git, that is the repository");
    assert_eq!(list().unwrap().len(), 1);
}

#[test]
fn choosing_a_folder_with_no_git_makes_that_folder_the_project() {
    let s = Scene::new();
    let notes = s.dir("notes");
    let chosen = choose(&notes).unwrap();
    assert_eq!(chosen.root, notes);
    // §3's table: after `Use`, a folder below it resolves to it.
    assert_eq!(
        resolve(&s.dir("notes/2026")).unwrap().map(|r| r.root),
        Some(notes)
    );
}

#[test]
fn choosing_inside_a_chosen_folder_chooses_that_folder() {
    let s = Scene::new();
    let first = choose(&s.dir("notes")).unwrap();
    let again = choose(&s.dir("notes/2026")).unwrap();
    assert_eq!(again.id, first.id);
    assert_eq!(list().unwrap().len(), 1);
}

#[test]
fn choosing_the_home_store_or_a_folder_around_or_inside_it_is_refused() {
    let s = Scene::new();
    let inside = s.home_store().join("artifacts");
    fs::create_dir(&inside).unwrap();

    for path in [s.home(), s.home_store(), inside, s.root()] {
        let err = choose(&path).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<ProjectError>(),
                Some(ProjectError::IsHome { .. })
            ),
            "{}: {err:#}",
            path.display()
        );
    }
    assert!(list().unwrap().is_empty());
}

#[test]
fn choosing_a_missing_path_or_a_file_is_refused_as_not_a_directory() {
    let s = Scene::new();
    let missing = s.root().join("work").join("not-yet");
    let file = s.dir("files").join("notes.txt");
    fs::write(&file, "text\n").unwrap();

    for path in [missing, file] {
        let err = choose(&path).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<ProjectError>(),
                Some(ProjectError::NotADirectory { .. })
            ),
            "{}: {err:#}",
            path.display()
        );
    }
    assert!(list().unwrap().is_empty());
}

#[test]
fn choosing_a_relative_path_is_refused_and_records_nothing() {
    let _s = Scene::new();
    // `resolve` answers no project for it, and `register`'s `canonical_root`
    // (S1.3) refuses it before anything is read (R22).
    let err = choose(Path::new("src")).unwrap_err();
    assert!(
        err.to_string().contains("must be an absolute path"),
        "{err:#}"
    );
    assert!(list().unwrap().is_empty());
}

// ── ST-23: the path as written first (Stage 2 S0) ───────────────────────────

/// A recorded root whose folder moved with a link left at the old path: the
/// old path and a path below it name that project, missing; once a turn in
/// the moved folder registers it, the new path names the new project and
/// the old one still names the old, and a turn at the old path records
/// nothing new.
#[cfg(unix)]
#[test]
fn a_recorded_root_that_now_leads_elsewhere_resolves_to_its_own_project() {
    let s = Scene::new();
    let repo = s.repo("app");
    s.dir("app/src");
    let own = register(&repo).unwrap();
    let moved = s.root().join("work").join("app-moved");
    fs::rename(&repo, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &repo).unwrap();

    for path in [repo.clone(), repo.join("src")] {
        let found = resolve(&path).unwrap().expect("its own project");
        assert_eq!(found.root, repo, "{}", path.display());
        assert_eq!(
            found.project.map(|p| (p.id, p.missing)),
            Some((own.id, true)),
            "{}",
            path.display()
        );
    }
    let other = register(&moved).unwrap();
    let id_at = |path: &Path| resolve(path).unwrap().and_then(|r| r.project).map(|p| p.id);
    assert_eq!(id_at(&repo.join("src")), Some(own.id));
    assert_eq!(id_at(&moved.join("src")), Some(other.id));
    assert_eq!(
        resolve_and_register(&repo).unwrap().map(|p| p.id),
        Some(own.id)
    );
    assert_eq!(list().unwrap().len(), 2, "nothing new was recorded");
}

/// The same for a folder renamed by case only, on a case-insensitive volume.
#[cfg(unix)]
#[test]
fn a_recorded_root_renamed_by_case_only_resolves_to_its_own_project() {
    let s = Scene::new();
    let work = s.dir("cases");
    if !crate::store::tests::volume_ignores_case(&work) {
        eprintln!(
            "skipped: {} is case-sensitive, so no folder can be renamed by case only",
            work.display()
        );
        return;
    }
    let repo = s.repo("cases/App");
    let own = register(&repo).unwrap();
    let renamed = work.join("app");
    fs::rename(&repo, &renamed).unwrap();

    let found = resolve(&repo).unwrap().expect("its own project");
    assert_eq!(found.root, repo);
    assert_eq!(
        found.project.map(|p| (p.id, p.missing)),
        Some((own.id, true))
    );
    let other = register(&renamed).unwrap();
    let id_at = |path: &Path| resolve(path).unwrap().and_then(|r| r.project).map(|p| p.id);
    assert_eq!(id_at(&repo), Some(own.id));
    assert_eq!(id_at(&renamed), Some(other.id));
}

/// The lead's ruling R-T29 (d0-st23-2): a recorded root is also matched as
/// named — its parent resolved, its last component not followed — so a
/// missing project's root spelled through a link in an ancestor (macOS's
/// `/var/…` for a root recorded under `/private/var/…`; Linux's `/home` →
/// `/var/home`) names that project, missing, as the recorded spelling does.
/// Before it, `lookup` found nothing as written and the canonical form — the
/// folder the link left at the root reaches — was registered as a new
/// project, where every turn of a `chat --project` then ran.
#[cfg(unix)]
#[test]
fn a_missing_root_spelled_through_a_link_in_an_ancestor_names_its_own_project() {
    let s = Scene::new();
    let repo = s.repo("app");
    s.dir("app/src");
    let aliased = s.tmp.path().join("work").join("app");
    if aliased == repo {
        eprintln!(
            "skipped: {} is its own canonical form, so no ancestor link spells it otherwise",
            s.tmp.path().display()
        );
        return;
    }
    let own = register(&repo).unwrap();
    let moved = s.root().join("work").join("app-moved");
    fs::rename(&repo, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &repo).unwrap();

    let found = lookup(&aliased).unwrap().expect("its own project");
    assert_eq!((found.id, found.missing), (own.id, true));
    for path in [aliased.clone(), aliased.join("src")] {
        let found = resolve(&path).unwrap().expect("its own project");
        assert_eq!(found.root, repo, "{}", path.display());
        assert_eq!(
            found.project.map(|p| (p.id, p.missing)),
            Some((own.id, true)),
            "{}",
            path.display()
        );
    }
    assert_eq!(choose(&aliased).unwrap().id, own.id);
    assert_eq!(register(&aliased).unwrap().id, own.id);
    assert_eq!(
        list().unwrap().len(),
        1,
        "nothing registered at the folder the link reaches"
    );
}

/// The written walk's precedence: a folder nearer on the written path that
/// holds `.git`, or that is a registered root by its canonical form (one a
/// link reaches), outranks the recorded root above it, and the canonical
/// walk answers it as today; anywhere else under the old path the recorded
/// root answers.
#[cfg(unix)]
#[test]
fn a_repository_or_a_project_nearer_on_the_written_path_outranks_the_recorded_root() {
    let s = Scene::new();
    let repo = s.repo("app");
    s.repo("app/vendor/lib");
    s.dir("app/docs");
    let own = register(&repo).unwrap();
    let moved = s.root().join("work").join("app-moved");
    fs::rename(&repo, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &repo).unwrap();
    let docs = register(&moved.join("docs")).unwrap();

    let nested = resolve(&repo.join("vendor").join("lib"))
        .unwrap()
        .expect("the nested repository");
    assert_eq!(nested.root, moved.join("vendor").join("lib"));
    assert!(!nested.is_registered());
    let chosen = resolve(&repo.join("docs"))
        .unwrap()
        .expect("the folder registered at the new path");
    assert_eq!(chosen.project.map(|p| p.id), Some(docs.id));
    let rest = resolve(&repo.join("vendor"))
        .unwrap()
        .expect("the recorded root");
    assert_eq!(rest.root, repo);
    assert_eq!(
        rest.project.map(|p| (p.id, p.missing)),
        Some((own.id, true))
    );
}

/// The written walk answers only a missing project (the lead's ruling
/// R-T17): for a project that still leads to itself, a path through a link
/// inside it, leading out of it to a folder that is no project, resolves by
/// its canonical form, as before Stage 2 — here to nothing, as the folder
/// itself does.
#[cfg(unix)]
#[test]
fn a_path_through_a_link_inside_a_present_project_resolves_by_its_canonical_form() {
    let s = Scene::new();
    let app = s.repo("app");
    register(&app).unwrap();
    let outside = s.dir("outside");
    std::os::unix::fs::symlink(&outside, app.join("vendor")).unwrap();

    assert_eq!(
        resolve(&app.join("vendor")).unwrap(),
        None,
        "what the link reaches"
    );
    assert_eq!(
        resolve(&outside).unwrap(),
        None,
        "the folder itself is no project"
    );
}

/// R-T17 (the S0 check's O2): a path through a link inside a project that
/// still leads to itself, into a folder below another registered project's
/// root, names that other project — the canonical walk's answer, as before
/// Stage 2 — never the project the link sits in.
#[cfg(unix)]
#[test]
fn a_link_inside_a_present_project_into_another_projects_folder_names_that_project() {
    let s = Scene::new();
    let p = s.repo("p");
    let q = s.repo("q");
    s.dir("q/sub");
    register(&p).unwrap();
    let other = register(&q).unwrap();
    std::os::unix::fs::symlink(q.join("sub"), p.join("link")).unwrap();

    let found = resolve(&p.join("link"))
        .unwrap()
        .expect("the project the link leads into");
    assert_eq!(found.root, q);
    assert_eq!(
        found.project.map(|project| (project.id, project.missing)),
        Some((other.id, false))
    );
}
