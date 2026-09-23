//! The `config` dispatcher: which arms open the database, and which must not.

use super::*;
use crate::test_util::EnvSandbox;
use clap::Parser;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use tempfile::tempdir;

/// A `ConfigEnv` whose database cannot be opened: `database()` panics. Any
/// test that completes with it has proved the arm it drove opened nothing.
#[derive(Default)]
pub(in crate::commands) struct FakeEnv {
    /// What `confirm_factory_reset` answers.
    pub answer: bool,
    pub confirm_calls: Cell<usize>,
    pub confirmed_root: RefCell<Option<PathBuf>>,
    /// Runs while the prompt is "open" — what the world does while a user
    /// is still reading the warning.
    pub during_prompt: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl ConfigEnv for FakeEnv {
    fn database(&mut self) -> Result<Database> {
        panic!("this form of `openalpaca config` must not open the store database");
    }

    fn confirm_factory_reset(&self, root: &Path) -> Result<bool> {
        self.confirm_calls.set(self.confirm_calls.get() + 1);
        *self.confirmed_root.borrow_mut() = Some(root.to_path_buf());
        if let Some(meanwhile) = self.during_prompt.borrow_mut().take() {
            meanwhile();
        }
        Ok(self.answer)
    }
}

/// Refuses to go on unless every path a `config` arm can write resolves inside
/// `root` — the store, `llm.toml` and `daemon.toml`. The repo checkout has a
/// real `config/daemon.toml`, and the owner's `~/.openalpaca` is a real store.
pub(in crate::commands) fn assert_sandboxed(root: &Path) {
    let paths = [
        store::home_root().unwrap(),
        store::database_path().unwrap(),
        crate::commands::ai_config_helpers::llm_config_path().unwrap(),
        crate::commands::daemon_config_cli::daemon_config_path().unwrap(),
    ];
    for path in paths {
        assert!(
            path.starts_with(root),
            "{} escaped the sandbox; refusing to run",
            path.display()
        );
    }
}

fn run(action: ConfigAction, env: &mut FakeEnv) -> Result<()> {
    run_with(
        ConfigArgs {
            action: Some(action),
        },
        env,
    )
}

#[test]
fn a_file_backed_key_never_opens_the_database() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    assert_sandboxed(tmp.path());

    for (key, value) in [
        ("ai.default_model", "qwen3:8b"),
        ("daemon.execution.max_rounds", "12"),
    ] {
        let mut env = FakeEnv::default();
        run(
            ConfigAction::Set {
                key: key.into(),
                value: value.into(),
            },
            &mut env,
        )
        .unwrap_or_else(|e| panic!("set {key}: {e:#}"));
        run(ConfigAction::Get { key: key.into() }, &mut env)
            .unwrap_or_else(|e| panic!("get {key}: {e:#}"));
        run(
            ConfigAction::Reset {
                key: Some(key.into()),
                factory: false,
            },
            &mut env,
        )
        .unwrap_or_else(|e| panic!("reset {key}: {e:#}"));
    }

    // `llm.toml` handling may create `state/.master_key` — that is the
    // encryptor, not the database. The database file itself must not appear.
    assert!(
        !store::database_path().unwrap().exists(),
        "a file-backed key must not create the store database"
    );
}

#[test]
fn reset_rejects_a_key_together_with_factory() {
    let parsed = crate::Cli::try_parse_from([
        "openalpaca",
        "config",
        "reset",
        "ai.default_model",
        "--factory",
    ]);
    let error = parsed
        .err()
        .expect("`reset <key> --factory` must be a usage error, not a factory reset");
    assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);

    // Each on its own still parses.
    assert!(crate::Cli::try_parse_from(["openalpaca", "config", "reset", "--factory"]).is_ok());
    assert!(
        crate::Cli::try_parse_from(["openalpaca", "config", "reset", "ai.default_model"]).is_ok()
    );
}
