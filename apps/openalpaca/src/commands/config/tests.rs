//! The `config` dispatcher: which arms open the database, and which must not.

use super::*;
use crate::test_util::EnvSandbox;
use clap::Parser;
use std::cell::{Cell, RefCell};
use std::path::Path;
use tempfile::tempdir;

/// A `ConfigEnv` whose database cannot be opened: `database()` panics. Any
/// test that completes with it has proved the arm it drove opened nothing.
#[derive(Default)]
pub(in crate::commands) struct FakeEnv {
    /// What `confirm_factory_reset` answers.
    pub answer: bool,
    pub confirm_calls: Cell<usize>,
    /// The warning the prompt was shown, verbatim.
    pub shown_warning: RefCell<Option<String>>,
    /// Runs while the prompt is "open" — what the world does while a user
    /// is still reading the warning.
    pub during_prompt: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl ConfigEnv for FakeEnv {
    fn database(&mut self) -> Result<Database> {
        panic!("this form of `openalpaca config` must not open the store database");
    }

    fn confirm_factory_reset(&self, warning: &str) -> Result<bool> {
        self.confirm_calls.set(self.confirm_calls.get() + 1);
        *self.shown_warning.borrow_mut() = Some(warning.to_owned());
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

/// The editor's own words around a database that will not open, rendered
/// the way `main` prints an error (`{:?}`: the context, then "Caused by").
fn bare_config_error() -> String {
    let error = run_with(ConfigArgs { action: None }, &mut RealConfigEnv::default())
        .expect_err("the editor must not open this database");
    format!("{error:?}")
}

/// Ruling C6: no message names another message's remedy. An older install's
/// stranded data is not fixed by `config reset --factory` — that deletes this
/// install's database, which does not exist — so the editor must not put the
/// verb in front of the stranded explanation.
#[test]
fn the_editor_does_not_offer_the_factory_reset_for_stranded_data() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    assert_sandboxed(tmp.path());
    let legacy = store::legacy_root::legacy_app_dir().unwrap();
    assert!(legacy.starts_with(tmp.path()));
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("openalpaca.db"), b"an older install's data").unwrap();

    let rendered = bare_config_error();

    assert!(
        rendered.contains("an older OpenAlpaca install's data is still on this machine"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("reset --factory"),
        "the stranded refusal must not be offered the factory reset:\n{rendered}"
    );
    assert!(
        !store::database_path().unwrap().exists(),
        "refusing must not create the database it refused over"
    );
}

/// The legacy-schema refusal names the factory reset itself — once. The
/// editor adds nothing to it.
#[test]
fn the_editor_leaves_the_legacy_schema_remedy_to_the_refusal() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    assert_sandboxed(tmp.path());
    let db = store::open_home_database().unwrap();
    db.with_connection(|conn| {
        Ok(conn.execute_batch(
            "DELETE FROM schema_version; INSERT INTO schema_version (version) VALUES (5);",
        )?)
    })
    .unwrap();
    drop(db);

    let rendered = bare_config_error();

    assert!(
        rendered.contains("Unsupported legacy schema version 5"),
        "{rendered}"
    );
    assert_eq!(
        rendered.matches("config reset --factory").count(),
        1,
        "the refusal names its own remedy, and nothing repeats it:\n{rendered}"
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
