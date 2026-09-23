//! `openalpaca config reset --factory`, driven without a daemon, a database
//! handle or a terminal: the environment is a `FakeEnv` whose `database()`
//! panics, and every store is a tempdir behind the one `EnvSandbox`.

use super::*;
use crate::commands::config::tests::{FakeEnv, assert_sandboxed};
use crate::commands::config::{ConfigAction, ConfigArgs, run_with};
use crate::test_util::EnvSandbox;
use std::collections::BTreeMap;
use std::path::PathBuf;
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
    assert_eq!(
        env.confirmed_root.borrow().as_deref(),
        Some(root.as_path()),
        "the confirmation must be shown the absolute store root"
    );
}

#[test]
fn a_running_daemon_refuses_the_factory_reset() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    assert_sandboxed(tmp.path());
    let root = home(tmp.path());
    seed_trio(&root);

    let mut env = FakeEnv {
        daemon_running: true,
        answer: true,
        ..FakeEnv::default()
    };
    let error = factory_reset(&mut env).expect_err("a live daemon must refuse the reset");

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

#[test]
fn the_confirmation_names_the_root_and_what_survives() {
    let root = Path::new("/tmp/oa-test-root");
    let warning = factory_reset_warning(root);
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
