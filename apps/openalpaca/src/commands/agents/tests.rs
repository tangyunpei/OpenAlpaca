//! `openalpaca agents tidy` (subagent design §2.12 D-1, D-2): it lists the
//! unedited copies of shipped templates the daemon sets aside and, with
//! `--apply`, moves them into `agents/.archived/` — nothing else. Each test
//! holds the binary's `EnvSandbox` and works in its own scratch config
//! directory, never `~/.openalpaca` or the repository's `config/`.

use super::*;
use crate::test_util::{EnvSandbox, json_answer, serve};
use chrono::TimeZone;
use clap::Parser;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Parser)]
struct Harness {
    #[command(subcommand)]
    command: AgentsCommands,
}

/// `colored` decides by tty at first use; pin it off so output reads literally.
fn plain() {
    colored::control::set_override(false);
}

/// An earlier build's shipped file, as core's tests hold it
/// (`crates/openalpaca_core/src/bundled/fixtures/<commit>/<path>`).
fn fixture(commit: &str, path: &str) -> String {
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/openalpaca_core/src/bundled/fixtures")
        .join(commit)
        .join(path);
    std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()))
}

/// The instant every test tidies at, so the archive's name is pinned.
fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc.with_ymd_and_hms(2026, 11, 7, 12, 0, 0).unwrap()
}

const WARNING: &str = "An earlier build started after this finds agents/ without these files and \
     does not seed them again; to go back, move agents/ aside and let that build seed it \
     (Installation Manual, Keep the builds apart).";

/// A scratch config directory under the sandbox, its `agents/` holding:
/// `eb98b820`'s `system_agent.md` (an earlier build's unedited copy), its
/// `writing_agent.md` with a line added (an edited copy), a template of the
/// person's own, and a file switching `research_agent` off (C3a).
struct Store {
    // Declared first, so it drops first.
    _env: EnvSandbox,
    _dir: tempfile::TempDir,
    config: PathBuf,
    agents: PathBuf,
}

impl Store {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = EnvSandbox::enter(dir.path());
        let config = dir.path().join("config");
        let agents = config.join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        let store = Self {
            _env: env,
            _dir: dir,
            config,
            agents,
        };
        store.write(
            "system_agent.md",
            &fixture("eb98b820", "agents/system_agent.md"),
        );
        store.write(
            "writing_agent.md",
            &format!(
                "{}\nKeep it short.\n",
                fixture("eb98b820", "agents/writing_agent.md")
            ),
        );
        store.write(
            "reader.md",
            "---\nid: reader\nname: Reader\ndescription: reads\ncapabilities:\n  - \"file_read\"\n---\nRead.\n",
        );
        store.write(
            "research_agent.md",
            "---\nid: research_agent\ndisabled: true\n---\n",
        );
        store
    }

    fn write(&self, name: &str, contents: &str) {
        std::fs::write(self.agents.join(name), contents).unwrap();
    }

    /// Every file under `agents/`, by its path relative to it, with its bytes
    /// and modification time.
    fn snapshot(&self) -> Vec<(String, Vec<u8>, SystemTime)> {
        let mut files = Vec::new();
        walk(&self.agents, &self.agents, &mut files);
        files.sort();
        files
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>, SystemTime)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(root, &path, out);
        } else {
            let relative = path.strip_prefix(root).unwrap().display().to_string();
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            out.push((relative, std::fs::read(&path).unwrap(), modified));
        }
    }
}

#[test]
fn tidy_is_a_verb_and_apply_its_one_flag() {
    assert!(matches!(
        Harness::try_parse_from(["agents", "tidy"]).unwrap().command,
        AgentsCommands::Tidy { apply: false }
    ));
    assert!(matches!(
        Harness::try_parse_from(["agents", "tidy", "--apply"])
            .unwrap()
            .command,
        AgentsCommands::Tidy { apply: true }
    ));
}

/// Without `--apply` the verb names the one copy the daemon sets aside —
/// with its version and where it would go — and touches nothing: every
/// file's bytes and modification time are as they were, and no
/// `.archived/` is made.
#[tokio::test]
async fn agents_tidy_lists_the_set_aside_copies_and_moves_nothing() {
    plain();
    let store = Store::new();
    let before = store.snapshot();

    let text = tidy_lines(&store.config, None, false, now())
        .await
        .unwrap()
        .join("\n");

    assert!(
        text.starts_with(&format!(
            "Unedited copies of shipped templates in {}, not read — the built-in ones run in \
             their place:",
            store.agents.display()
        )),
        "{text}"
    );
    let copy = store.agents.join("system_agent.md");
    assert!(
        text.contains(&format!("{} (version: earlier)", copy.display())),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "would move to {}",
            store
                .agents
                .join(".archived/system_agent.20261107120000.md")
                .display()
        )),
        "{text}"
    );
    for kept in ["writing_agent.md", "reader.md", "research_agent.md"] {
        assert!(
            !text.contains(kept),
            "{kept} is not a set-aside copy: {text}"
        );
    }
    assert!(
        text.contains("Nothing was moved; `openalpaca agents tidy --apply` moves them."),
        "{text}"
    );
    assert!(!text.contains(WARNING), "{text}");
    assert_eq!(store.snapshot(), before);
    assert!(!store.agents.join(".archived").exists());
}

/// `--apply` moves the set-aside copy, its bytes whole, to
/// `.archived/<file stem>.<UTC timestamp>.md`, says so, and ends with the
/// warning for an earlier build; the edited copy, the person's own template
/// and the switching file stay where they were, byte for byte.
#[tokio::test]
async fn agents_tidy_apply_moves_only_set_aside_copies_to_archived() {
    plain();
    let store = Store::new();
    let before = store.snapshot();

    let lines = tidy_lines(&store.config, None, true, now()).await.unwrap();
    let text = lines.join("\n");

    let from = store.agents.join("system_agent.md");
    let to = store
        .agents
        .join(".archived/system_agent.20261107120000.md");
    assert!(!from.exists());
    assert_eq!(
        std::fs::read_to_string(&to).unwrap(),
        fixture("eb98b820", "agents/system_agent.md")
    );
    assert!(
        text.contains(&format!("Moved {} to {}", from.display(), to.display())),
        "{text}"
    );
    assert_eq!(lines.last().map(String::as_str), Some(WARNING), "{text}");

    let after = store.snapshot();
    let untouched = |files: &[(String, Vec<u8>, SystemTime)]| {
        files
            .iter()
            .filter(|(name, _, _)| name != "system_agent.md" && !name.starts_with(".archived"))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(untouched(&after), untouched(&before));
    assert_eq!(untouched(&after).len(), 3);
    let archived: Vec<_> = std::fs::read_dir(store.agents.join(".archived"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(archived, ["system_agent.20261107120000.md"]);
}

/// With nothing set aside the verb says so, with or without `--apply`, and
/// prints no warning.
#[tokio::test]
async fn agents_tidy_with_no_copy_says_there_is_nothing_to_tidy() {
    plain();
    let store = Store::new();
    std::fs::remove_file(store.agents.join("system_agent.md")).unwrap();
    for apply in [false, true] {
        let text = tidy_lines(&store.config, None, apply, now())
            .await
            .unwrap()
            .join("\n");
        assert_eq!(
            text,
            format!(
                "No unedited copies of shipped templates in {}: nothing to tidy.",
                store.agents.display()
            )
        );
    }
    assert!(!store.agents.join(".archived").exists());
}

/// A running daemon of an older build may read as the person's a copy this
/// build sets aside (its shipped-versions list lacks that version). So with
/// a daemon answering, `--apply` moves only the copies the daemon's own
/// listing names as unedited copies not used, and names the one it left.
/// The daemon's path is the canonical one; the CLI's is not.
#[tokio::test]
async fn tidy_apply_moves_nothing_the_running_daemon_reads() {
    plain();
    let store = Store::new();
    store.write(
        "writing_agent.md",
        &fixture("eb98b820", "agents/writing_agent.md"),
    );
    let canonical = store.agents.canonicalize().unwrap();
    let rows = serde_json::json!([
        {"id": "system_agent", "source": "bundled", "not_used": [{
            "source": "file",
            "path": canonical.join("system_agent.md").display().to_string(),
            "plugin": null, "reason": "unedited_copy", "version": "earlier", "edit": null
        }]},
        {"id": "writing_agent", "source": "file", "not_used": []}
    ])
    .to_string();
    let (addr, server) = serve(vec![json_answer("200 OK", "templates", &rows)]);
    let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");

    let text = tidy_lines(&store.config, Some(&client), true, now())
        .await
        .unwrap()
        .join("\n");

    let requests = server.join().expect("the server finishes");
    assert!(
        requests[0].starts_with("GET /v1/agent-templates HTTP/1.1"),
        "{}",
        requests[0]
    );
    let left = store.agents.join("writing_agent.md");
    assert_eq!(
        std::fs::read_to_string(&left).unwrap(),
        fixture("eb98b820", "agents/writing_agent.md")
    );
    assert!(
        text.contains(&format!(
            "Left {}: the running daemon does not list it as an unused copy",
            left.display()
        )),
        "{text}"
    );
    assert!(!store.agents.join("system_agent.md").exists());
    assert!(
        store
            .agents
            .join(".archived/system_agent.20261107120000.md")
            .is_file()
    );
    assert!(text.ends_with(WARNING), "{text}");
}

/// The listing previews `--apply` against a running daemon too: the copy
/// the daemon lists as unused would move, the one it does not would be
/// left, named — and nothing moves.
#[tokio::test]
async fn tidy_listing_with_a_running_daemon_says_which_copy_would_be_left() {
    plain();
    let store = Store::new();
    store.write(
        "writing_agent.md",
        &fixture("eb98b820", "agents/writing_agent.md"),
    );
    let before = store.snapshot();
    let canonical = store.agents.canonicalize().unwrap();
    let rows = serde_json::json!([
        {"id": "system_agent", "source": "bundled", "not_used": [{
            "source": "file",
            "path": canonical.join("system_agent.md").display().to_string(),
            "plugin": null, "reason": "unedited_copy", "version": "earlier", "edit": null
        }]},
        {"id": "writing_agent", "source": "file", "not_used": []}
    ])
    .to_string();
    let (addr, server) = serve(vec![json_answer("200 OK", "templates", &rows)]);
    let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");

    let text = tidy_lines(&store.config, Some(&client), false, now())
        .await
        .unwrap()
        .join("\n");

    let requests = server.join().expect("the server finishes");
    assert!(
        requests[0].starts_with("GET /v1/agent-templates HTTP/1.1"),
        "{}",
        requests[0]
    );
    assert!(
        text.contains(&format!(
            "  {} (version: earlier)\n    would be left: the running daemon does not list it \
             as an unused copy, so it may read it as yours",
            store.agents.join("writing_agent.md").display()
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "  {} (version: earlier)\n    would move to {}",
            store.agents.join("system_agent.md").display(),
            store
                .agents
                .join(".archived/system_agent.20261107120000.md")
                .display()
        )),
        "{text}"
    );
    assert!(
        text.ends_with("Nothing was moved; `openalpaca agents tidy --apply` moves them."),
        "{text}"
    );
    assert!(!text.contains(WARNING), "{text}");
    assert_eq!(store.snapshot(), before);
    assert!(!store.agents.join(".archived").exists());
}

// ── `--apply` inside a git work tree (the owner's item 22, 2026-11-12) ──
//
// Each fake work tree is a `.git` entry this test writes in its own
// scratch directory; no test runs git or writes under the repository.

/// What `--apply` answers inside a git work tree: `path` is the directory
/// it found inside one, as given, and `root` the work tree, as the file
/// system resolves it.
fn refusal(path: &Path, root: &Path) -> String {
    format!(
        "`openalpaca agents tidy --apply` moves nothing inside a git work tree, and {} is \
         inside {}: a copy there may be a file the repository holds, as in a development \
         checkout, whose config/ is the source of the built-in ones. Nothing was moved; \
         `openalpaca agents tidy` lists the copies, for you to move yourself.",
        path.display(),
        root.display()
    )
}

/// The listing's last line inside a git work tree.
fn listing_end(path: &Path, root: &Path) -> String {
    format!(
        "Nothing was moved; `openalpaca agents tidy --apply` moves nothing here: {} is inside \
         the git work tree {}.",
        path.display(),
        root.display()
    )
}

/// What `--apply` answers with `agents/` a git work tree itself, the config
/// directory outside one: `path` is `agents/`, as given (TIDY-LINKED).
fn own_refusal(path: &Path) -> String {
    format!(
        "`openalpaca agents tidy --apply` moves nothing inside a git work tree, and {} is \
         a git work tree itself: a copy there may be a file the repository holds, as in a \
         development checkout, whose config/ is the source of the built-in ones. Nothing was \
         moved; `openalpaca agents tidy` lists the copies, for you to move yourself.",
        path.display()
    )
}

/// The listing's last line with `agents/` a git work tree itself.
fn own_listing_end(path: &Path) -> String {
    format!(
        "Nothing was moved; `openalpaca agents tidy --apply` moves nothing here: {} is a git \
         work tree itself.",
        path.display()
    )
}

/// What a tidy says of an `agents/` holding no unedited copy (TIDY-1).
fn nothing_to_tidy(agents: &Path) -> String {
    format!(
        "No unedited copies of shipped templates in {}: nothing to tidy.",
        agents.display()
    )
}

/// What `--apply` answers with `agents/` a link into a git work tree:
/// `path` is the link, as given, and `root` the work tree it resolves into.
#[cfg(unix)]
fn linked_refusal(path: &Path, root: &Path) -> String {
    format!(
        "`openalpaca agents tidy --apply` moves nothing inside a git work tree, and {} is \
         a link into {}: a copy there may be a file the repository holds, as in a development \
         checkout, whose config/ is the source of the built-in ones. Nothing was moved; \
         `openalpaca agents tidy` lists the copies, for you to move yourself.",
        path.display(),
        root.display()
    )
}

/// The listing's last line with `agents/` a link into a git work tree.
#[cfg(unix)]
fn linked_listing_end(path: &Path, root: &Path) -> String {
    format!(
        "Nothing was moved; `openalpaca agents tidy --apply` moves nothing here: {} is a link \
         into the git work tree {}.",
        path.display(),
        root.display()
    )
}

/// A `.git` directory above the config directory — a development
/// checkout's `config/`, named by `OPENALPACA_CONFIG_DIR` — makes `--apply`
/// a refusal naming the work tree: nothing moves, no `.archived/` is made,
/// and the daemon is not asked (the client given answers nothing, so asking
/// it would be another error). Without `--apply` the copy is still listed.
#[tokio::test]
async fn agents_tidy_apply_in_a_git_work_tree_is_refused_naming_it() {
    plain();
    let store = Store::new();
    std::fs::create_dir(store._dir.path().join(".git")).unwrap();
    let root = store._dir.path().canonicalize().unwrap();
    let before = store.snapshot();
    let nobody = DaemonClient::for_tests("http://127.0.0.1:1", "token");

    let error = tidy_lines(&store.config, Some(&nobody), true, now())
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), refusal(&store.config, &root));
    assert_eq!(store.snapshot(), before);
    assert!(!store.agents.join(".archived").exists());

    let text = tidy_lines(&store.config, None, false, now())
        .await
        .unwrap()
        .join("\n");
    assert!(
        text.contains(&format!(
            "  {} (version: earlier)\n    would move to {}",
            store.agents.join("system_agent.md").display(),
            store
                .agents
                .join(".archived/system_agent.20261107120000.md")
                .display()
        )),
        "{text}"
    );
    assert!(text.ends_with(&listing_end(&store.config, &root)), "{text}");
    assert_eq!(store.snapshot(), before);
    assert!(!store.agents.join(".archived").exists());
}

/// With nothing to tidy, a tidy says so first (TIDY-1): inside a git work
/// tree an `agents/` holding no unedited copy — only the person's own files
/// — is no refusal under `--apply`, but the one "nothing to tidy" line, and
/// nothing is moved or made; with the copy back, `--apply` is refused.
#[tokio::test]
async fn agents_tidy_apply_in_a_git_work_tree_with_nothing_to_tidy_says_so() {
    plain();
    let store = Store::new();
    let copy = std::fs::read(store.agents.join("system_agent.md")).unwrap();
    std::fs::remove_file(store.agents.join("system_agent.md")).unwrap();
    std::fs::create_dir(store._dir.path().join(".git")).unwrap();
    let root = store._dir.path().canonicalize().unwrap();
    let before = store.snapshot();
    assert_eq!(before.len(), 3);

    let lines = tidy_lines(&store.config, None, true, now()).await.unwrap();

    assert_eq!(lines, vec![nothing_to_tidy(&store.agents)]);
    assert_eq!(store.snapshot(), before);
    assert!(!store.agents.join(".archived").exists());

    std::fs::write(store.agents.join("system_agent.md"), copy).unwrap();
    let error = tidy_lines(&store.config, None, true, now())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), refusal(&store.config, &root));
    assert!(!store.agents.join(".archived").exists());
}

// ── An `agents/` linked into a git work tree (the owner's item 6 of the
// second answers of 2026-11-14, TIDY-LINKED) ──
//
// The config directory is outside any work tree; `agents/` is a link to a
// folder inside one this test makes, `.git` and all, in its own scratch
// directory, beside the config directory, never above it.

/// Moves `store`'s `agents/` into `repo/config/agents` under the scratch
/// directory, makes `repo` a work tree with a `.git` directory, and puts a
/// link at `agents/` naming the moved folder. Returns the work tree, as the
/// file system resolves it.
#[cfg(unix)]
fn link_agents_into_a_work_tree(store: &Store) -> PathBuf {
    let repo = store._dir.path().join("repo");
    let target = repo.join("config/agents");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::rename(&store.agents, &target).unwrap();
    std::os::unix::fs::symlink(&target, &store.agents).unwrap();
    assert!(!store._dir.path().join(".git").exists());
    assert!(!store.config.join(".git").exists());
    repo.canonicalize().unwrap()
}

/// With `agents/` a link into a git work tree, the config directory outside
/// one, `--apply` is refused naming `agents/` (as given) and the work tree:
/// nothing moves, no `.archived/` is made through the link, and no daemon
/// is needed to say so. Without `--apply` the copy is still listed, and the
/// last line says `--apply` moves nothing there.
#[cfg(unix)]
#[tokio::test]
async fn agents_tidy_apply_with_agents_linked_into_a_git_work_tree_is_refused_naming_it() {
    plain();
    let store = Store::new();
    let root = link_agents_into_a_work_tree(&store);
    let before = store.snapshot();
    assert_eq!(before.len(), 4);

    let error = tidy_lines(&store.config, None, true, now())
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), linked_refusal(&store.agents, &root));
    assert_eq!(store.snapshot(), before);
    assert!(!store.agents.join(".archived").exists());

    let text = tidy_lines(&store.config, None, false, now())
        .await
        .unwrap()
        .join("\n");
    assert!(
        text.contains(&format!(
            "  {} (version: earlier)\n    would move to {}",
            store.agents.join("system_agent.md").display(),
            store
                .agents
                .join(".archived/system_agent.20261107120000.md")
                .display()
        )),
        "{text}"
    );
    assert!(
        text.ends_with(&linked_listing_end(&store.agents, &root)),
        "{text}"
    );
    assert_eq!(store.snapshot(), before);
    assert!(!store.agents.join(".archived").exists());
}

/// The linked `agents/` says "nothing to tidy" first, as the config
/// directory does (TIDY-1): with nothing in it, `--apply` answers that one
/// line, and nothing is made through the link.
#[cfg(unix)]
#[tokio::test]
async fn agents_tidy_apply_with_agents_linked_into_a_work_tree_and_nothing_to_tidy_says_so() {
    plain();
    let store = Store::new();
    for entry in std::fs::read_dir(&store.agents).unwrap() {
        std::fs::remove_file(entry.unwrap().path()).unwrap();
    }
    link_agents_into_a_work_tree(&store);

    let lines = tidy_lines(&store.config, None, true, now()).await.unwrap();

    assert_eq!(lines, vec![nothing_to_tidy(&store.agents)]);
    assert!(store.snapshot().is_empty());
    assert!(!store.agents.join(".archived").exists());
}

/// An `agents/` that is a git work tree itself — a folder of the config
/// directory's own holding a `.git` entry, the config directory outside any
/// work tree — is checked too (TIDY-LINKED, the advisor's "check it too"):
/// `--apply` is refused naming `agents/`, nothing moves, no `.archived/` is
/// made, and the listing's last line says so; with nothing to tidy in it,
/// `--apply` says that first (TIDY-1). The `.git` entry is this test's own,
/// in its scratch directory; no git runs.
#[tokio::test]
async fn agents_tidy_apply_in_an_agents_folder_that_is_its_own_work_tree_is_refused_naming_it() {
    plain();
    let store = Store::new();
    std::fs::create_dir(store.agents.join(".git")).unwrap();
    assert!(!store._dir.path().join(".git").exists());
    assert!(!store.config.join(".git").exists());
    let before = store.snapshot();
    assert_eq!(before.len(), 4);

    let error = tidy_lines(&store.config, None, true, now())
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), own_refusal(&store.agents));
    assert_eq!(store.snapshot(), before);
    assert!(!store.agents.join(".archived").exists());

    let text = tidy_lines(&store.config, None, false, now())
        .await
        .unwrap()
        .join("\n");
    assert!(
        text.contains(&format!(
            "  {} (version: earlier)\n    would move to {}",
            store.agents.join("system_agent.md").display(),
            store
                .agents
                .join(".archived/system_agent.20261107120000.md")
                .display()
        )),
        "{text}"
    );
    assert!(text.ends_with(&own_listing_end(&store.agents)), "{text}");
    assert_eq!(store.snapshot(), before);

    std::fs::remove_file(store.agents.join("system_agent.md")).unwrap();
    let lines = tidy_lines(&store.config, None, true, now()).await.unwrap();
    assert_eq!(lines, vec![nothing_to_tidy(&store.agents)]);
    assert!(!store.agents.join(".archived").exists());
}

/// AGENT-SETS: `agents set` on a run's own instance while it runs is
/// refused by the daemon, `409 AGENT_RUNNING`, its sentence naming the
/// template to set. The CLI prints that sentence and exits non-zero — it
/// is no version conflict, and "please retry" would never succeed.
#[tokio::test]
async fn agents_set_on_a_running_instance_fails_with_the_daemons_sentence() {
    plain();
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(dir.path());
    let config = serde_json::json!({
        "config": {"agent": {"id": "reader::1a2b3c4d"}, "llm": {"model": "model-r"}},
        "config_version": 0
    })
    .to_string();
    let refusal = serde_json::json!({"error": {
        "code": "AGENT_RUNNING",
        "message": "the agent 'reader::1a2b3c4d' is running, and a set never changes a \
                    running agent; set its template 'reader' instead, which its next runs load"
    }})
    .to_string();
    let (addr, server) = serve(vec![
        json_answer("200 OK", "config", &config),
        json_answer("409 Conflict", "refused", &refusal),
    ]);
    let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");

    let err = agent_set_with(&client, "reader::1a2b3c4d", "llm.model", "model-set")
        .await
        .unwrap_err()
        .to_string();

    let requests = server.join().expect("the server finishes");
    assert!(
        requests[1].starts_with("PUT /v1/agents/reader::1a2b3c4d/config HTTP/1.1"),
        "{}",
        requests[1]
    );
    assert!(err.starts_with("AGENT_RUNNING: "), "{err}");
    assert!(err.contains("set its template 'reader'"), "{err}");
}

/// A stale version is still the retry case: the CLI says so and the run
/// ends as before.
#[tokio::test]
async fn agents_set_on_a_stale_version_says_to_retry() {
    plain();
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvSandbox::enter(dir.path());
    let config = serde_json::json!({
        "config": {"agent": {"id": "reader"}, "llm": {"model": "model-r"}},
        "config_version": 3
    })
    .to_string();
    let refusal = serde_json::json!({"error": {
        "code": "CONFIG_CONFLICT",
        "message": "Config version mismatch — reload and retry"
    }})
    .to_string();
    let (addr, server) = serve(vec![
        json_answer("200 OK", "config", &config),
        json_answer("409 Conflict", "stale", &refusal),
    ]);
    let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");

    agent_set_with(&client, "reader", "llm.model", "model-set")
        .await
        .unwrap();

    server.join().expect("the server finishes");
}

/// *Rev 123*'s 123-6 and *Rev 124*'s 124-1, (b): `agent list` and `agent
/// status` read the row the daemon sends — `skills_json` (a list of
/// capabilities) and `llm_config_json` (an object with a `model`) — where
/// they read `model` and `skills`, keys no route sends, and always printed
/// `-`. The row is `cli/get-agents.json`'s keys with values in them.
fn agent_row(
    skills_json: serde_json::Value,
    llm_config_json: serde_json::Value,
) -> serde_json::Value {
    let sample: serde_json::Value = serde_json::from_slice(
        &std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../openalpaca-gui/src/lib/api/contract/cli/get-agents.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let mut row = sample[0].clone();
    row["id"] = "reviewer".into();
    row["name"] = "Reviewer".into();
    row["status"] = "idle".into();
    row["description"] = "Reviews code".into();
    row["skills_json"] = skills_json;
    row["llm_config_json"] = llm_config_json;
    row
}

fn stored_skills() -> serde_json::Value {
    serde_json::json!(
        r#"[{"name":"research","category":"assigned","proficiency":1.0},{"name":"code_review","category":"assigned","proficiency":1.0}]"#
    )
}

#[test]
fn agent_list_prints_the_rows_model_and_skills() {
    plain();
    let row: AgentItem = serde_json::from_value(agent_row(
        stored_skills(),
        serde_json::json!(r#"{"model":"model-a","fallback_models":[],"overrides":{}}"#),
    ))
    .unwrap();
    let line = row.table_row();
    assert!(line.contains("model-a"), "{line}");
    assert!(line.contains("research, code_"), "{line}");
    // Nothing stored, an empty list, or a text that is not JSON: `-`.
    for (skills, llm) in [
        (serde_json::Value::Null, serde_json::Value::Null),
        (serde_json::json!("[]"), serde_json::json!("{}")),
        (serde_json::json!("not json"), serde_json::json!("[1]")),
    ] {
        let row: AgentItem = serde_json::from_value(agent_row(skills, llm)).unwrap();
        let line = row.table_row();
        let columns: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(
            columns,
            ["reviewer", "Reviewer", "idle", "-", "-"],
            "{line}"
        );
    }
}

#[test]
fn agent_status_prints_the_rows_model_and_skills() {
    plain();
    let detail: AgentDetail = serde_json::from_value(serde_json::json!({
        "agent": agent_row(stored_skills(), serde_json::json!(r#"{"model":"model-a"}"#)),
        "metrics": null,
    }))
    .unwrap();
    let lines = status_lines(&detail);
    assert!(lines.contains(&"Model: model-a".to_string()), "{lines:?}");
    assert!(
        lines.contains(&"Skills: research, code_review".to_string()),
        "{lines:?}"
    );
    let bare: AgentDetail = serde_json::from_value(serde_json::json!({
        "agent": agent_row(serde_json::Value::Null, serde_json::Value::Null),
    }))
    .unwrap();
    let lines = status_lines(&bare);
    assert!(lines.contains(&"Model: -".to_string()), "{lines:?}");
    assert!(!lines.iter().any(|l| l.starts_with("Skills:")), "{lines:?}");
}

/// The capabilities prompt's examples are capabilities a shipped template
/// assigns, so a person who types them gets an agent whose tools the
/// sandbox admits (its answer is `capabilities.assigned`).
#[test]
fn the_capabilities_prompt_names_capabilities_a_shipped_template_assigns() {
    let assigned: std::collections::HashSet<String> = openalpaca_core::bundled::bundled_templates()
        .into_iter()
        .flat_map(|t| t.frontmatter.capabilities)
        .collect();
    assert!(
        CAPABILITIES_PROMPT.starts_with("Capabilities "),
        "{CAPABILITIES_PROMPT}"
    );
    let examples = CAPABILITIES_PROMPT
        .split_once("e.g., ")
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .expect("the prompt gives its examples as `(…, e.g., a,b)`");
    for name in examples.split(',') {
        assert!(
            assigned.contains(name),
            "{name:?} is not a capability any shipped template assigns: {assigned:?}"
        );
    }
}
