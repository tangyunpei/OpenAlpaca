//! What a tidy acts on (subagent design §2.12 D-2, as the commit reads it):
//! the config directory the daemon is handed — `OPENALPACA_CONFIG_DIR` when
//! it names a directory, else the store's `config/`, never one found by
//! walking up from the working directory — and the running daemon, if one
//! runs. Each test holds the binary's `EnvSandbox`; the working directory
//! stays the crate's, inside the repository, whose `config/agents/` a walk
//! up would find.

use super::*;
use crate::test_util::EnvSandbox;

/// The repository's own `config/`, the one a walk up from the crate finds.
fn repository_config() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config")
        .canonicalize()
        .unwrap()
}

/// Sets or removes `OPENALPACA_CONFIG_DIR`; the caller holds the
/// `EnvSandbox`, which restores it on drop.
fn set_config_dir(value: Option<&Path>) {
    // SAFETY: serialized by the EnvSandbox the caller holds (its ENV_LOCK).
    unsafe {
        match value {
            Some(dir) => std::env::set_var("OPENALPACA_CONFIG_DIR", dir),
            None => std::env::remove_var("OPENALPACA_CONFIG_DIR"),
        }
    }
}

#[test]
fn the_config_directory_is_the_variable_s_else_the_store_s_never_a_walk_up() {
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(dir.path());
    let store_config = dir.path().join("home/config");
    assert!(repository_config().join("agents").is_dir());

    // The variable naming a directory: that directory.
    let named = dir.path().join("elsewhere");
    std::fs::create_dir_all(&named).unwrap();
    set_config_dir(Some(&named));
    assert_eq!(config_dir().unwrap(), named);

    // Naming nothing, or unset: the store's config/, not the repository's.
    set_config_dir(Some(&dir.path().join("missing")));
    assert_eq!(config_dir().unwrap(), store_config);
    set_config_dir(None);
    let found = config_dir().unwrap();
    assert_eq!(found, store_config);
    assert!(!found.starts_with(repository_config().parent().unwrap()));
    assert!(!store_config.exists(), "finding it creates nothing");
}

/// No `discovery.json`, or one naming a process that is not a daemon (this
/// test's own): no daemon runs, so a tidy asks none and needs none.
#[tokio::test]
async fn no_daemon_runs_without_a_discovery_file_naming_a_live_one() {
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(dir.path());
    let config = dir.path().join("config");

    let (found, daemon) = target().await.unwrap();
    assert_eq!(found, config);
    assert!(daemon.is_none());

    let now = Utc::now();
    openalpaca_app_support::discovery::write_discovery_atomic(
        &openalpaca_app_support::discovery::Discovery {
            schema: 1,
            instance_id: "stale".to_string(),
            pid: std::process::id(),
            started_at: now,
            listen: openalpaca_app_support::discovery::Listen {
                host: "127.0.0.1".to_string(),
                port: 1,
            },
            auth: openalpaca_app_support::discovery::Auth {
                token: "token".to_string(),
                issued_at: now,
                expires_at: now,
            },
            build: openalpaca_app_support::discovery::Build {
                version: "0.0.0".to_string(),
                protocol: 1,
            },
        },
    )
    .unwrap();
    let (found, daemon) = target().await.unwrap();
    assert_eq!(found, config);
    assert!(daemon.is_none(), "a stale discovery file names no daemon");
}

/// A work tree is the nearest of a path and its ancestors, as resolved,
/// holding a `.git` entry of either kind; a path that does not exist yet
/// is in its nearest existing ancestor's. Every `.git` here is this test's
/// own, in its scratch directory; no git runs.
#[test]
fn a_git_work_tree_is_the_nearest_ancestor_holding_a_git_entry() {
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(dir.path());
    let scratch = dir.path().canonicalize().unwrap();
    let config = scratch.join("outer/inner/config");
    std::fs::create_dir_all(&config).unwrap();
    assert_eq!(
        git_work_tree(&config),
        None,
        "the scratch dir is in no work tree"
    );

    // A `.git` directory above: that directory, by its resolved path.
    std::fs::create_dir(scratch.join("outer/.git")).unwrap();
    assert_eq!(git_work_tree(&config), Some(scratch.join("outer")));
    assert_eq!(
        git_work_tree(&dir.path().join("outer/inner/config")),
        Some(scratch.join("outer"))
    );
    // A path below it that does not exist yet: the same work tree.
    assert_eq!(
        git_work_tree(&config.join("agents/missing")),
        Some(scratch.join("outer"))
    );
    // A nearer `.git` file (a linked worktree's, a submodule's) wins.
    std::fs::write(
        scratch.join("outer/inner/.git"),
        "gitdir: ../.git/modules/inner\n",
    )
    .unwrap();
    assert_eq!(git_work_tree(&config), Some(scratch.join("outer/inner")));
    // The directory itself holding one is in its own work tree.
    std::fs::create_dir(config.join(".git")).unwrap();
    assert_eq!(git_work_tree(&config), Some(config.clone()));
}

/// Which work tree a tidy of a config directory's `agents/` would move
/// inside, and the path it names (TIDY-LINKED): the config directory's,
/// naming it, first; else, for an `agents/` that is a link resolving into
/// one, that one, naming `agents/` as given; else, for a folder of its own
/// holding a `.git` entry of either kind, that folder, a work tree itself;
/// nothing for a missing `agents/`, a plain folder of its own (a `.git`
/// only in a folder below it, as in a skill's own folder), a link to a
/// folder in no work tree, or a link naming nothing. Every `.git` and link here is this test's own,
/// in its scratch directory; no git runs.
#[cfg(unix)]
#[test]
fn a_linked_folder_or_one_that_is_its_own_work_tree_is_checked_after_the_config_directory() {
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(dir.path());
    let scratch = dir.path().canonicalize().unwrap();
    let config = dir.path().join("store/config");
    let agents = config.join("agents");
    std::fs::create_dir_all(&config).unwrap();
    let repo = scratch.join("repo");
    let in_repo = repo.join("config/agents");
    std::fs::create_dir_all(&in_repo).unwrap();
    std::fs::create_dir(repo.join(".git")).unwrap();
    let outside = scratch.join("plain/agents");
    std::fs::create_dir_all(&outside).unwrap();

    // Missing, then a plain folder of its own, one with a `.git` only in a
    // template's folder below it, then one holding a `.git` directory, then
    // a `.git` file: a work tree itself, named as given.
    assert_eq!(work_tree_of(&config, &agents), None);
    std::fs::create_dir(&agents).unwrap();
    assert_eq!(work_tree_of(&config, &agents), None);
    std::fs::create_dir_all(agents.join("deeper/.git")).unwrap();
    assert_eq!(work_tree_of(&config, &agents), None);
    std::fs::create_dir(agents.join(".git")).unwrap();
    let own = Some(WorkTree {
        named: agents.clone(),
        root: agents.canonicalize().unwrap(),
        place: Place::Itself,
    });
    assert_eq!(work_tree_of(&config, &agents), own);
    std::fs::remove_dir(agents.join(".git")).unwrap();
    std::fs::write(agents.join(".git"), "gitdir: ../elsewhere/.git\n").unwrap();
    assert_eq!(work_tree_of(&config, &agents), own);
    std::fs::remove_dir_all(&agents).unwrap();

    // A link to a folder in no work tree, then one naming nothing.
    std::os::unix::fs::symlink(&outside, &agents).unwrap();
    assert_eq!(work_tree_of(&config, &agents), None);
    std::fs::remove_file(&agents).unwrap();
    std::os::unix::fs::symlink(scratch.join("missing"), &agents).unwrap();
    assert_eq!(work_tree_of(&config, &agents), None);
    std::fs::remove_file(&agents).unwrap();

    // A link into a work tree: that one, naming `agents/` as given.
    std::os::unix::fs::symlink(&in_repo, &agents).unwrap();
    assert_eq!(
        work_tree_of(&config, &agents),
        Some(WorkTree {
            named: agents.clone(),
            root: repo.clone(),
            place: Place::LinkInto,
        })
    );

    // The config directory inside a work tree of its own: that one, naming
    // the config directory, whatever its `agents/` links to.
    std::fs::create_dir(scratch.join("store/.git")).unwrap();
    assert_eq!(
        work_tree_of(&config, &agents),
        Some(WorkTree {
            named: config.clone(),
            root: scratch.join("store"),
            place: Place::Inside,
        })
    );
}

// ── a copy's own `.git` (TIDY-COPY-GIT) ──────────────────────────────

/// The instant these tidies run at, so the archive's name is pinned.
fn tidy_now() -> DateTime<Utc> {
    use chrono::TimeZone;
    Utc.with_ymd_and_hms(2026, 11, 7, 12, 0, 0).unwrap()
}

/// A scratch config directory under the sandbox whose `skills/` holds two
/// unedited copies: `code-review/`, this build's version, and
/// `create-skill/`, what `82022776`'s seeding wrote (core's fixture).
fn two_skill_copies(root: &Path) -> (PathBuf, PathBuf) {
    let config = root.join("config");
    let skills = config.join("skills");
    for file in openalpaca_core::bundled::BUNDLED_SKILLS
        .iter()
        .filter(|file| file.path.starts_with("skills/code-review/"))
    {
        let target = config.join(file.path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, file.contents).unwrap();
    }
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/openalpaca_core/src/bundled/fixtures/82022776/skills/create-skill");
    for name in [
        "SKILL.md",
        "scripts/check_exists.sh",
        "scripts/list_skills.sh",
        "scripts/scaffold.sh",
        "scripts/write_skill_md.sh",
    ] {
        let target = skills.join("create-skill").join(name);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(fixture.join(name), &target).unwrap();
    }
    (config, skills)
}

const OWN_GIT: &str = "its folder holds a .git entry; move it yourself if you mean to";

/// An unedited copy whose own folder holds a `.git` entry the classifier
/// still reads as a copy — a `.git` directory with no file in it — is left
/// where it is, with one line naming it and saying why, while its sibling,
/// a plain unedited copy, is listed and moved as before; the run is not
/// refused (the advisor's pick (b) on Rev 137 item 2: "leave that one copy
/// with a line"). Every `.git` here is this test's own, in its scratch
/// directory; no git runs.
#[tokio::test]
async fn a_skill_copy_holding_its_own_git_entry_is_left_and_named_while_a_sibling_moves() {
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(dir.path());
    let (config, skills) = two_skill_copies(dir.path());
    let held = skills.join("create-skill");
    let sibling = skills.join("code-review");
    std::fs::create_dir(held.join(".git")).unwrap();
    assert_eq!(work_tree_of(&config, &skills), None);

    let listed = tidy_lines(Kind::Skills, &config, None, false, tidy_now())
        .await
        .unwrap();
    let sibling_to = skills.join(".archived/code-review.20261107120000");
    assert_eq!(
        listed,
        vec![
            format!(
                "Unedited copies of shipped skills in {}, not read — the built-in ones, or a \
                 plugin's skill of the same name, run in their place:",
                skills.display()
            ),
            format!("  {} (version: current)", sibling.display()),
            format!("    would move to {}", sibling_to.display()),
            format!("  {} (version: earlier)", held.display()),
            format!("    would be left: {OWN_GIT}"),
            "Nothing was moved; `openalpaca skills tidy --apply` moves them.".to_string(),
        ]
    );

    let lines = tidy_lines(Kind::Skills, &config, None, true, tidy_now())
        .await
        .unwrap();
    assert_eq!(
        lines,
        vec![
            format!("Moved {} to {}", sibling.display(), sibling_to.display()),
            format!("Left {}: {OWN_GIT}", held.display()),
            Kind::Skills.warning().to_string(),
        ]
    );
    assert!(sibling_to.join("SKILL.md").is_file());
    assert!(!sibling.exists());
    assert!(held.join("SKILL.md").is_file());
    assert!(held.join(".git").is_dir());
    assert!(
        !skills
            .join(".archived/create-skill.20261107120000")
            .exists()
    );

    // Alone, it is still left with its line, and nothing moves; the
    // listing's last line promises no move.
    let listed = tidy_lines(Kind::Skills, &config, None, false, tidy_now())
        .await
        .unwrap();
    assert_eq!(
        &listed[1..],
        [
            format!("  {} (version: earlier)", held.display()),
            format!("    would be left: {OWN_GIT}"),
            "Nothing was moved, and `openalpaca skills tidy --apply` would move none of them."
                .to_string(),
        ]
    );
    let lines = tidy_lines(Kind::Skills, &config, None, true, tidy_now())
        .await
        .unwrap();
    assert_eq!(lines, vec![format!("Left {}: {OWN_GIT}", held.display())]);
    assert!(held.join(".git").is_dir());
}

/// A `.git` entry with a file in it — the file a linked worktree or a
/// submodule has, or a `.git` directory holding git's own files — makes
/// the folder the person's, not an unedited copy (core's
/// `classify_skill_dir` counts every regular file under it), so a tidy
/// neither lists it nor moves it, and its sibling is tidied as before.
/// Templates are top-level `.md` files only, never folders, so a template
/// has no folder of its own to hold a `.git`. This test passes on the
/// parent of TIDY-COPY-GIT by design: it pins what core already did.
#[tokio::test]
async fn a_skill_folder_whose_own_git_holds_a_file_is_not_a_copy_and_is_not_listed() {
    for form in ["file", "directory"] {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvSandbox::enter(dir.path());
        let (config, skills) = two_skill_copies(dir.path());
        let held = skills.join("create-skill");
        let sibling = skills.join("code-review");
        match form {
            "file" => std::fs::write(held.join(".git"), "gitdir: ../elsewhere/.git\n").unwrap(),
            _ => {
                std::fs::create_dir(held.join(".git")).unwrap();
                std::fs::write(held.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
            }
        }

        let listed = tidy_lines(Kind::Skills, &config, None, false, tidy_now())
            .await
            .unwrap()
            .join("\n");
        assert!(
            !listed.contains(&held.display().to_string()),
            "{form}: {listed}"
        );
        assert!(
            listed.contains(&sibling.display().to_string()),
            "{form}: {listed}"
        );

        let lines = tidy_lines(Kind::Skills, &config, None, true, tidy_now())
            .await
            .unwrap();
        let sibling_to = skills.join(".archived/code-review.20261107120000");
        assert_eq!(
            lines,
            vec![
                format!("Moved {} to {}", sibling.display(), sibling_to.display()),
                Kind::Skills.warning().to_string(),
            ],
            "{form}"
        );
        assert!(held.join("SKILL.md").is_file(), "{form}");
        assert!(
            std::fs::symlink_metadata(held.join(".git")).is_ok(),
            "{form}"
        );
    }
}

/// With a daemon answering that lists neither copy as unused, the copy
/// holding its own `.git` is left for that reason, named first, and its
/// sibling for the daemon's; neither would move, so the listing's last
/// line promises no move.
#[tokio::test]
async fn a_copy_s_own_git_is_its_reason_before_the_running_daemon_s() {
    use crate::test_util::{json_answer, serve};
    colored::control::set_override(false);
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(dir.path());
    let (config, skills) = two_skill_copies(dir.path());
    let held = skills.join("create-skill");
    let sibling = skills.join("code-review");
    std::fs::create_dir(held.join(".git")).unwrap();
    let (addr, server) = serve(vec![json_answer("200 OK", "skills", "[]")]);
    let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");

    let listed = tidy_lines(Kind::Skills, &config, Some(&client), false, tidy_now())
        .await
        .unwrap();

    let requests = server.join().expect("the server finishes");
    assert!(
        requests[0].starts_with("GET /v1/skills HTTP/1.1"),
        "{}",
        requests[0]
    );
    assert_eq!(
        &listed[1..],
        [
            format!("  {} (version: current)", sibling.display()),
            "    would be left: the running daemon does not list it as an unused copy, so it \
             may read it as yours"
                .to_string(),
            format!("  {} (version: earlier)", held.display()),
            format!("    would be left: {OWN_GIT}"),
            "Nothing was moved, and `openalpaca skills tidy --apply` would move none of them."
                .to_string(),
        ]
    );
    assert!(held.join(".git").is_dir());
    assert!(sibling.join("SKILL.md").is_file());
    assert!(!skills.join(".archived").exists());
}
