//! `openalpaca config reset --factory`, driven without a daemon, a database
//! handle or a terminal: the environment is a `FakeEnv` whose `database()`
//! panics, and every store is a tempdir behind the one `EnvSandbox`.

use super::*;
use crate::commands::config::tests::{FakeEnv, assert_sandboxed};
use crate::commands::config::{ConfigAction, ConfigArgs, run_with};
use crate::test_util::{EnvSandbox, LockHolder};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use tempfile::tempdir;

const TRIO: [&str; 3] = [
    "state/openalpaca.db",
    "state/openalpaca.db-wal",
    "state/openalpaca.db-shm",
];

/// The home store `EnvSandbox` points `OPENALPACA_HOME_STORE` at.
fn home(tmp: &Path) -> PathBuf {
    tmp.join("home")
}

fn seed(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn seed_trio(root: &Path) {
    for rel in TRIO {
        seed(root, rel, b"not a real database");
    }
}

/// Through the real dispatcher, as `openalpaca config reset --factory` does.
fn factory_reset(env: &mut FakeEnv) -> Result<()> {
    run_with(
        ConfigArgs {
            action: Some(ConfigAction::Reset {
                key: None,
                factory: true,
            }),
        },
        env,
    )
}

#[test]
fn a_factory_reset_never_opens_the_store_database() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    assert_sandboxed(tmp.path());
    let root = home(tmp.path());
    seed_trio(&root);

    let mut env = FakeEnv {
        answer: true,
        ..FakeEnv::default()
    };
    // `FakeEnv::database` panics, so completing proves the open is not on
    // the dispatcher's path for `--factory`.
    factory_reset(&mut env).unwrap();

    for rel in TRIO {
        assert!(!root.join(rel).exists(), "{rel} must be deleted");
    }
    assert_eq!(env.confirm_calls.get(), 1);
    let warning = env.shown_warning.borrow().clone().unwrap();
    assert!(
        root.is_absolute() && warning.contains(&format!("{}/state/openalpaca.db", root.display())),
        "the confirmation must be shown the absolute store root:\n{warning}"
    );
}

/// A daemon — running, or still booting before it has written
/// `discovery.json` — holds the singleton lock. Here another process holds
/// it for real, which is the only way to hold an `fcntl` lock against this
/// one.
#[test]
fn a_held_daemon_lock_refuses_the_factory_reset() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    assert_sandboxed(tmp.path());
    let root = home(tmp.path());
    seed_trio(&root);
    let _daemon = LockHolder::spawn(&root);

    let mut env = FakeEnv {
        answer: true,
        daemon_running: true,
        ..FakeEnv::default()
    };
    let error = factory_reset(&mut env).expect_err("a held daemon lock must refuse the reset");

    let message = format!("{error:#}");
    assert!(
        message.contains("openalpaca daemon stop"),
        "the refusal must say how to stop the daemon: {message}"
    );
    assert_eq!(
        env.confirm_calls.get(),
        0,
        "the question must not be asked when the answer cannot be acted on"
    );
    for rel in TRIO {
        assert!(root.join(rel).exists(), "{rel} must survive a refusal");
    }
}

/// A held lock with no daemon to be found — one still booting, or a lock file
/// left unwritable by a daemon once run under `sudo` — still refuses, but must
/// not say a daemon is running: that sends the user to a `daemon stop` which
/// answers "No active daemon found".
#[test]
fn a_held_lock_with_no_visible_daemon_refuses_without_claiming_one() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    assert_sandboxed(tmp.path());
    let root = home(tmp.path());
    seed_trio(&root);
    let _holder = LockHolder::spawn(&root);

    let mut env = FakeEnv {
        answer: true,
        daemon_running: false,
        ..FakeEnv::default()
    };
    let error = factory_reset(&mut env).expect_err("a held lock must refuse the reset");

    let message = format!("{error:#}");
    assert!(
        !message.contains("A daemon is running"),
        "no daemon was found, so the refusal must not claim one: {message}"
    );
    assert!(
        message.contains("openalpacad.lock") && message.contains("Nothing was deleted"),
        "the refusal must name the lock file and say nothing was deleted: {message}"
    );
    assert_eq!(
        env.confirm_calls.get(),
        0,
        "no question for an answer we cannot act on"
    );
    for rel in TRIO {
        assert!(root.join(rel).exists(), "{rel} must survive a refusal");
    }
}

/// The race the lock closes: nothing was running when the question was
/// asked, and a daemon came up while the user was still reading it. The
/// store had no `state/` yet, so there was nothing to lock up front; the
/// reset must take the lock after the answer, fail, and delete nothing.
#[test]
fn a_daemon_that_starts_during_the_prompt_refuses_the_factory_reset() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    assert_sandboxed(tmp.path());
    let root = home(tmp.path());
    assert!(!root.join("state").exists());

    let daemon: Rc<RefCell<Option<LockHolder>>> = Rc::default();
    let started = Rc::clone(&daemon);
    let booted_root = root.clone();
    let mut env = FakeEnv {
        answer: true,
        during_prompt: RefCell::new(Some(Box::new(move || {
            // A daemon's boot: it takes the lock (creating `state/`), then
            // opens the database.
            *started.borrow_mut() = Some(LockHolder::spawn(&booted_root));
            seed_trio(&booted_root);
        }))),
        daemon_running: true,
        ..FakeEnv::default()
    };
    let error = factory_reset(&mut env)
        .expect_err("a daemon started during the prompt must refuse the reset");

    assert!(daemon.borrow().is_some(), "the daemon must have started");
    assert!(
        format!("{error:#}").contains("openalpaca daemon stop"),
        "{error:#}"
    );
    for rel in TRIO {
        assert!(
            root.join(rel).exists(),
            "{rel} belongs to a live daemon and must survive"
        );
    }
}

/// And the lock is held from before the question to after the delete: a
/// daemon started during the prompt of a store that *did* exist cannot take
/// it — which is what makes it exit instead of opening the database.
#[test]
fn the_reset_holds_the_daemon_lock_while_the_prompt_is_open() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    assert_sandboxed(tmp.path());
    let root = home(tmp.path());
    seed_trio(&root);

    let lock_free_during_prompt: Rc<Cell<Option<bool>>> = Rc::default();
    let seen = Rc::clone(&lock_free_during_prompt);
    let prompt_root = root.clone();
    let mut env = FakeEnv {
        answer: true,
        during_prompt: RefCell::new(Some(Box::new(move || {
            // A daemon starting now: can it take the lock?
            seen.set(Some(LockHolder::try_spawn(&prompt_root).is_some()));
        }))),
        ..FakeEnv::default()
    };
    factory_reset(&mut env).unwrap();

    assert_eq!(
        lock_free_during_prompt.get(),
        Some(false),
        "a daemon starting while the prompt is open must find the lock held"
    );
    for rel in TRIO {
        assert!(!root.join(rel).exists(), "{rel} must be deleted");
    }
    assert!(
        LockHolder::try_spawn(&root).is_some(),
        "the lock must be released once the reset is done"
    );
}

#[test]
fn the_embedding_cache_and_the_master_key_survive_a_factory_reset() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    assert_sandboxed(tmp.path());
    let root = home(tmp.path());
    seed_trio(&root);
    let survivors: BTreeMap<&str, &[u8]> = BTreeMap::from([
        ("state/cache/fastembed/model.onnx", b"weights".as_slice()),
        ("state/.master_key", b"key material".as_slice()),
        ("state/logs/daemon.log", b"a log line".as_slice()),
        ("state/backups/llm.toml.bak.1", b"[providers]".as_slice()),
        ("artifacts/report.md", b"# report".as_slice()),
        ("uploads/photo.png", b"\x89PNG".as_slice()),
        ("sessions/s1/log.jsonl", b"{}".as_slice()),
        ("plugins/demo/plugin.toml", b"name = \"demo\"".as_slice()),
    ]);
    for (rel, bytes) in &survivors {
        seed(&root, rel, bytes);
    }

    let mut env = FakeEnv {
        answer: true,
        ..FakeEnv::default()
    };
    factory_reset(&mut env).unwrap();

    for rel in TRIO {
        assert!(!root.join(rel).exists(), "{rel} must be deleted");
    }
    for (rel, bytes) in &survivors {
        assert_eq!(
            std::fs::read(root.join(rel)).unwrap_or_else(|e| panic!("{rel} is gone: {e}")),
            *bytes,
            "{rel} must survive byte for byte"
        );
    }
}

/// Pure text: no file here exists, so both configuration lines say so.
fn targets_under(root: &str) -> ResetTargets {
    ResetTargets {
        root: PathBuf::from(root),
        llm_config: PathBuf::from(format!("{root}/config/llm.toml")),
        daemon_config: PathBuf::from(format!("{root}/config/daemon.toml")),
    }
}

#[test]
fn the_confirmation_names_the_root_and_what_survives() {
    let warning = factory_reset_warning(&targets_under("/tmp/oa-test-root"));
    for needle in [
        "/tmp/oa-test-root",
        "state/openalpaca.db",
        "-wal",
        "-shm",
        "artifacts/",
        "uploads/",
        "sessions/",
        "state/cache/",
        ".master_key",
        "state/logs/",
        "No backup",
    ] {
        assert!(
            warning.contains(needle),
            "the warning must mention {needle:?}:\n{warning}"
        );
    }
    assert_eq!(CONFIRM_WORD, "factory-reset");
}

/// `llm.toml` and `daemon.toml` resolve through `OPENALPACA_CONFIG_DIR`, the
/// store's `config/`, and last `./config/` under the current directory — a
/// checkout's own files. So the warning names each by its absolute path,
/// never as a bare `config/…` a reader would place under the store root, and
/// the reset clears exactly the file it named.
#[test]
fn the_confirmation_names_the_absolute_config_files_it_clears() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    assert_sandboxed(tmp.path());
    // `EnvSandbox` points OPENALPACA_CONFIG_DIR here, outside the store root.
    let config = tmp.path().join("config");
    seed(&config, "llm.toml", b"");
    seed(&config, "daemon.toml", b"[execution]\nmax_rounds = 12\n");

    let mut env = FakeEnv {
        answer: true,
        ..FakeEnv::default()
    };
    factory_reset(&mut env).unwrap();

    let warning = env.shown_warning.borrow().clone().unwrap();
    for file in ["llm.toml", "daemon.toml"] {
        let absolute = config.join(file);
        assert!(absolute.is_absolute());
        assert!(
            warning.contains(&format!("  - {} — ", absolute.display())),
            "the warning must name {} absolutely:\n{warning}",
            absolute.display()
        );
    }
    assert!(
        !warning.contains("  - config/"),
        "no configuration file may be named relatively:\n{warning}"
    );
    assert_eq!(
        std::fs::read_to_string(config.join("daemon.toml")).unwrap(),
        "# Reset to defaults\n",
        "the daemon.toml the warning named is the one reset"
    );
}

#[test]
fn a_missing_config_file_is_named_as_none() {
    let warning = factory_reset_warning(&targets_under("/tmp/oa-no-such-root"));
    for (kind, path) in [
        ("llm.toml", "/tmp/oa-no-such-root/config/llm.toml"),
        ("daemon.toml", "/tmp/oa-no-such-root/config/daemon.toml"),
    ] {
        assert!(
            warning.contains(&format!("no {kind} to clear (there is none at {path})")),
            "{kind}:\n{warning}"
        );
    }
}

#[test]
fn a_declined_confirmation_deletes_nothing() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    assert_sandboxed(tmp.path());
    let root = home(tmp.path());
    seed_trio(&root);

    let mut env = FakeEnv {
        answer: false,
        ..FakeEnv::default()
    };
    // Declining is not a failure: it must not set a non-zero exit status.
    factory_reset(&mut env).unwrap();

    assert_eq!(env.confirm_calls.get(), 1);
    for rel in TRIO {
        assert!(
            root.join(rel).exists(),
            "{rel} must survive a declined reset"
        );
    }
}

#[test]
fn a_factory_reset_on_a_store_with_no_database_succeeds() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    assert_sandboxed(tmp.path());
    let root = home(tmp.path());

    let mut env = FakeEnv {
        answer: true,
        ..FakeEnv::default()
    };
    factory_reset(&mut env).unwrap();

    assert!(
        !root.join("state").exists(),
        "resolving the database path must not create the store's state directory"
    );
}

#[test]
fn the_confirmation_word_is_not_y() {
    for (typed, accepted) in [
        ("factory-reset", true),
        ("Factory-Reset", true),
        (" factory-reset\n", true),
        ("y", false),
        ("yes", false),
        ("", false),
        ("factory reset", false),
    ] {
        assert_eq!(prompt_accepts(typed), accepted, "typed {typed:?}");
    }
}
