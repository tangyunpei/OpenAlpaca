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

/// The refusal comes before a copy is read, so inside a git work tree an
/// `agents/` with nothing to tidy is refused the same way, and nothing is
/// made in it.
#[tokio::test]
async fn agents_tidy_apply_in_a_git_work_tree_is_refused_with_nothing_to_tidy() {
    plain();
    let store = Store::new();
    for entry in std::fs::read_dir(&store.agents).unwrap() {
        std::fs::remove_file(entry.unwrap().path()).unwrap();
    }
    std::fs::create_dir(store._dir.path().join(".git")).unwrap();
    let root = store._dir.path().canonicalize().unwrap();

    let error = tidy_lines(&store.config, None, true, now())
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), refusal(&store.config, &root));
    assert!(store.snapshot().is_empty());
    assert!(!store.agents.join(".archived").exists());
}
