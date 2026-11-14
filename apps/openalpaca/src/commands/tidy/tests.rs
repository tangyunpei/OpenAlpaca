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
#[test]
fn no_daemon_runs_without_a_discovery_file_naming_a_live_one() {
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(dir.path());
    let config = dir.path().join("config");

    let (found, daemon) = target().unwrap();
    assert_eq!(found, config);
    assert!(daemon.is_none());

    let now = Utc::now();
    openalpaca_storage::discovery::write_discovery_atomic(
        &openalpaca_storage::discovery::Discovery {
            schema: 1,
            instance_id: "stale".to_string(),
            pid: std::process::id(),
            started_at: now,
            listen: openalpaca_storage::discovery::Listen {
                host: "127.0.0.1".to_string(),
                port: 1,
            },
            auth: openalpaca_storage::discovery::Auth {
                token: "token".to_string(),
                issued_at: now,
                expires_at: now,
            },
            build: openalpaca_storage::discovery::Build {
                version: "0.0.0".to_string(),
                protocol: 1,
            },
        },
    )
    .unwrap();
    let (found, daemon) = target().unwrap();
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
