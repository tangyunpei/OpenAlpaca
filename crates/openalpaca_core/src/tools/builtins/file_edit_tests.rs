//! `file_edit` (ST-22; storage design §13, R47–R49): the match, the body,
//! the pre-edit copy and the pointer row. Every test that touches a file does
//! it in a guarded temp project (`GuardedProject`, interfaces §1.7) — never in
//! the repository, never in the process's current directory.

use super::*;
use crate::session_log::{RecordType, SNAPSHOTS_DIR, SessionLogLimits, SessionLogService};
use crate::test_util::GuardedProject;
use sha2::{Digest, Sha256};
use std::fs;

const SESSION: &str = "sess-edit";
const OWNER: &str = "owner-test";

// ── R47's match, pure ───────────────────────────────────────────────────

#[test]
fn one_occurrence_is_replaced_and_every_other_byte_kept() {
    let text = "keep\tthis  \nold line\r\nlast line, no newline at the end";
    let edited = apply_edit("a.txt", text, "old line", "new line", false).unwrap();
    assert_eq!(
        edited,
        Edited {
            text: "keep\tthis  \nnew line\r\nlast line, no newline at the end".to_string(),
            replacements: 1,
            first_line: 2,
        }
    );
}

#[test]
fn two_occurrences_without_replace_all_are_refused_naming_the_count() {
    let refused = apply_edit(
        "src/a.rs",
        "x = 1;\ny = 2;\nx = 1;\n",
        "x = 1;",
        "x = 3;",
        false,
    )
    .unwrap_err();
    assert_eq!(
        refused,
        "old_string occurs 2 times in 'src/a.rs'. Give more of the surrounding text so it \
         occurs once, or set replace_all to true to change every one."
    );
}

#[test]
fn replace_all_changes_every_occurrence_and_names_the_first_line() {
    let edited = apply_edit("a.txt", "x\na\nb\na\n", "a", "c", true).unwrap();
    assert_eq!(
        edited,
        Edited {
            text: "x\nc\nb\nc\n".to_string(),
            replacements: 2,
            first_line: 2,
        }
    );
}

#[test]
fn an_absent_old_string_is_refused_naming_file_read() {
    let refused = apply_edit("a.txt", "alpha\n", "beta", "gamma", false).unwrap_err();
    assert_eq!(
        refused,
        "old_string was not found in 'a.txt'. Read the file with file_read and copy the exact \
         text, whitespace and line breaks included."
    );
}

#[test]
fn an_empty_old_string_is_refused_naming_file_write() {
    let refused = apply_edit("a.txt", "alpha\n", "", "beta", false).unwrap_err();
    assert_eq!(
        refused,
        "old_string is empty. Give the exact text to replace; to create a file, use file_write."
    );
}

#[test]
fn identical_strings_are_refused() {
    let refused = apply_edit("a.txt", "alpha\n", "alpha", "alpha", false).unwrap_err();
    assert_eq!(
        refused,
        "old_string and new_string are the same, so there is nothing to change."
    );
}

/// R47: a model writes LF; a CRLF file keeps its endings.
#[test]
fn an_lf_old_string_matches_a_crlf_file_and_the_file_keeps_its_endings() {
    let text = "fn a() {\r\n    1\r\n}\r\n";
    let edited = apply_edit("a.rs", text, "    1\n}", "    2\n    // two\n}", false).unwrap();
    assert_eq!(
        edited,
        Edited {
            text: "fn a() {\r\n    2\r\n    // two\r\n}\r\n".to_string(),
            replacements: 1,
            first_line: 2,
        }
    );
}

/// The CRLF retry may make the two strings one: that is still nothing to do.
#[test]
fn a_crlf_retry_that_makes_the_strings_equal_is_refused_as_identical() {
    let refused = apply_edit("a.txt", "a\r\nb\r\n", "a\nb", "a\r\nb", false).unwrap_err();
    assert_eq!(
        refused,
        "old_string and new_string are the same, so there is nothing to change."
    );
}

#[test]
fn lf_to_crlf_leaves_an_existing_crlf_alone() {
    assert_eq!(lf_to_crlf("a\nb\r\nc\n"), "a\r\nb\r\nc\r\n");
    assert_eq!(lf_to_crlf("no break"), "no break");
}

/// The lead's ruling R-T11 (R47: "line endings included"): a file whose
/// every line break is `\r\n` stays all-CRLF when a one-line `old_string` is
/// replaced by a two-line `new_string`; a file that already mixes the two
/// keeps what the model wrote.
#[test]
fn a_two_line_new_string_in_an_all_crlf_file_keeps_it_all_crlf() {
    let text = "[a]\r\nkey = 1\r\n";
    let edited = apply_edit("a.toml", text, "key = 1", "key = 2\nother = 3", false).unwrap();
    assert_eq!(
        edited,
        Edited {
            text: "[a]\r\nkey = 2\r\nother = 3\r\n".to_string(),
            replacements: 1,
            first_line: 2,
        }
    );

    let mixed = "[a]\nkey = 1\r\n";
    let edited = apply_edit("a.toml", mixed, "key = 1", "key = 2\nother = 3", false).unwrap();
    assert_eq!(edited.text, "[a]\nkey = 2\nother = 3\r\n");
}

/// R-T11's edge: in an all-CRLF file an `old_string` that starts with a line
/// break is found as given — its `\n` right after the file's `\r` — so a
/// `new_string` written `\r\n` would leave `\r\r\n` behind. Both strings are
/// written `\r\n` there, and the file stays all-CRLF.
#[test]
fn an_old_string_that_starts_with_a_line_break_keeps_an_all_crlf_file_all_crlf() {
    let text = "[a]\r\nkey = 1\r\n";
    let edited = apply_edit("a.toml", text, "\nkey = 1", "\nkey = 2", false).unwrap();
    assert_eq!(
        edited,
        Edited {
            text: "[a]\r\nkey = 2\r\n".to_string(),
            replacements: 1,
            first_line: 1,
        }
    );
}

// ── The tool, in a guarded project ──────────────────────────────────────

/// A guarded project, this conversation's log and a database for its
/// pointer rows. Declared so the log and the database drop before the
/// project's guard and folders.
struct Bench {
    service: Arc<SessionLogService>,
    db: Database,
    _db_dir: tempfile::TempDir,
    gp: GuardedProject,
}

impl Bench {
    fn new() -> Self {
        let gp = GuardedProject::new();
        fs::create_dir_all(gp.home_root().join("sessions")).unwrap();
        let service = SessionLogService::new(
            gp.home_root().join("sessions"),
            None,
            SessionLogLimits::default(),
            "test".to_string(),
        )
        .into_arc();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();
        Self {
            service,
            db,
            _db_dir: db_dir,
            gp,
        }
    }

    /// The tool with a database; `cap` sets `snapshot_max_bytes`.
    fn tool(&self, cap: Option<u64>) -> FileEditTool {
        let daemon_config = cap.map(|bytes| {
            let mut config = DaemonConfig::default();
            config.orchestrator.sessions.snapshot_max_bytes = bytes;
            Arc::new(ArcSwap::from_pointee(config))
        });
        FileEditTool {
            db: Some(self.db.clone()),
            daemon_config,
        }
    }

    /// A project turn whose loop writes this conversation's log — what the
    /// main loop and a subagent hand the tool.
    fn ctx(&self) -> ToolContext {
        ToolContext {
            session_id: Some(SESSION.to_string()),
            session_log: Some(self.service.handle_for(SESSION)),
            ..self.gp.ctx()
        }
    }

    fn put(&self, rel: &str, bytes: &[u8]) {
        let file = self.gp.root.join(rel);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, bytes).unwrap();
    }

    fn bytes(&self, rel: &str) -> Vec<u8> {
        fs::read(self.gp.root.join(rel)).unwrap()
    }

    /// Every copy this conversation kept, by name.
    fn images(&self) -> Vec<std::path::PathBuf> {
        let mut out: Vec<std::path::PathBuf> =
            fs::read_dir(self.service.session_dir(SESSION).join(SNAPSHOTS_DIR))
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_file())
                .collect();
        out.sort();
        out
    }

    /// The owner's pointer rows (interfaces §5.2).
    fn rows(&self) -> Vec<openalpaca_storage::ArtifactRecord> {
        let mut query = openalpaca_storage::ArtifactQuery::new(OWNER);
        query.in_project = Some(true);
        openalpaca_storage::ArtifactStore::new(&self.db)
            .list(&query)
            .unwrap()
            .0
    }
}

fn args(path: &str, old: &str, new: &str) -> serde_json::Value {
    serde_json::json!({"path": path, "old_string": old, "new_string": new})
}

const LIB: &str = "pub fn retries() -> u32 {\n    3\n}\n";

#[tokio::test]
async fn an_edit_changes_the_file_in_the_project_and_says_where() {
    let b = Bench::new();
    b.put("src/lib.rs", LIB.as_bytes());

    let out = b
        .tool(None)
        .execute_with_context(&args("src/lib.rs", "    3", "    5"), &b.ctx())
        .await
        .unwrap();

    let after = "pub fn retries() -> u32 {\n    5\n}\n";
    assert_eq!(b.bytes("src/lib.rs"), after.as_bytes());
    assert_eq!(
        out,
        format!(
            "Edited src/lib.rs in the project: 1 replacement, the first at line 2; {} bytes at {}. \
             Its previous content is kept in this conversation's log.",
            after.len(),
            b.gp.root.join("src/lib.rs").display()
        )
    );
}

/// R47 (§13's first test): refused, and nothing is touched — no copy, no row.
#[tokio::test]
async fn old_string_found_twice_without_replace_all_is_refused_and_the_file_unchanged() {
    let b = Bench::new();
    let before = "x = 1;\ny = 2;\nx = 1;\n";
    b.put("src/a.rs", before.as_bytes());
    let ctx = b.ctx();

    let refused = b
        .tool(None)
        .execute_with_context(&args("src/a.rs", "x = 1;", "x = 3;"), &ctx)
        .await
        .unwrap_err();

    assert!(
        refused.starts_with("old_string occurs 2 times in 'src/a.rs'."),
        "{refused}"
    );
    assert_eq!(b.bytes("src/a.rs"), before.as_bytes());
    assert!(ctx.session_log.as_ref().unwrap().flush().await);
    assert!(b.images().is_empty(), "a refusal keeps no copy");
    assert!(b.rows().is_empty(), "a refusal records no row");
}

/// R47 (§13's second test): a missing file's refusal names file_write, and
/// nothing is created.
#[tokio::test]
async fn a_missing_file_is_refused_naming_file_write() {
    let b = Bench::new();

    let refused = b
        .tool(None)
        .execute_with_context(&args("docs/new.md", "a", "b"), &b.ctx())
        .await
        .unwrap_err();

    assert_eq!(
        refused,
        "'docs/new.md' does not exist in the project. file_edit changes an existing file; \
         use file_write to create it."
    );
    assert!(!b.gp.root.join("docs").exists(), "nothing was created");
}

/// ST-17 (§13's third test): the bytes the edit destroys are in this
/// conversation's `snapshots/`, committed by a `file_snapshot` record.
#[tokio::test]
async fn the_pre_edit_copy_is_kept_before_the_change() {
    let b = Bench::new();
    b.put("src/lib.rs", LIB.as_bytes());
    let ctx = b.ctx();

    b.tool(None)
        .execute_with_context(&args("src/lib.rs", "    3", "    5"), &ctx)
        .await
        .unwrap();
    assert!(ctx.session_log.as_ref().unwrap().flush().await);

    let images = b.images();
    assert_eq!(images.len(), 1, "{images:?}");
    assert_eq!(
        fs::read(&images[0]).unwrap(),
        LIB.as_bytes(),
        "the copy holds the old bytes"
    );
    let records = crate::session_log::read_records(&b.service.session_dir(SESSION)).unwrap();
    let snapshot = records
        .iter()
        .find(|r| r.kind == RecordType::FileSnapshot.as_str())
        .expect("the copy is committed by a record");
    assert_eq!(snapshot.data["path"], "src/lib.rs");
}

/// ST-17's second refusal, and the proof of the order: a copy that cannot be
/// taken stops the edit before the file is touched.
#[tokio::test]
async fn a_copy_that_fails_refuses_the_edit_and_leaves_the_file() {
    let b = Bench::new();
    b.put("src/lib.rs", LIB.as_bytes());
    // `snapshots/` cannot be created: a file stands at its name.
    fs::create_dir_all(b.service.session_dir(SESSION)).unwrap();
    fs::write(
        b.service.session_dir(SESSION).join(SNAPSHOTS_DIR),
        "in the way",
    )
    .unwrap();

    let refused = b
        .tool(None)
        .execute_with_context(&args("src/lib.rs", "    3", "    5"), &b.ctx())
        .await
        .unwrap_err();

    assert!(
        refused.starts_with(
            "Refusing to edit 'src/lib.rs': its pre-edit snapshot could not be taken ("
        ),
        "{refused}"
    );
    assert!(refused.ends_with("). The file is unchanged."), "{refused}");
    assert_eq!(b.bytes("src/lib.rs"), LIB.as_bytes());
    assert!(b.rows().is_empty());
}

/// R35 (§13's fourth test): one row per project file, updated in place.
#[tokio::test]
async fn a_second_edit_updates_the_same_pointer_row() {
    let b = Bench::new();
    b.put("src/lib.rs", LIB.as_bytes());
    let tool = b.tool(None);

    tool.execute_with_context(&args("src/lib.rs", "    3", "    5"), &b.ctx())
        .await
        .unwrap();
    let first = b.rows();
    assert_eq!(first.len(), 1, "{first:?}");
    tool.execute_with_context(&args("src/lib.rs", "    5", "    8"), &b.ctx())
        .await
        .unwrap();
    let second = b.rows();

    assert_eq!(second.len(), 1, "{second:?}");
    assert_eq!(second[0].id, first[0].id, "the same row");
    let after = b.bytes("src/lib.rs");
    assert_eq!(after, "pub fn retries() -> u32 {\n    8\n}\n".as_bytes());
    assert!(second[0].in_project);
    assert_eq!(second[0].rel_path.as_deref(), Some("src/lib.rs"));
    assert_eq!(second[0].size_bytes, after.len() as i64);
    assert_eq!(second[0].sha256, format!("{:x}", Sha256::digest(&after)));
    assert_ne!(
        second[0].sha256, first[0].sha256,
        "the row follows the file"
    );
}

/// R47 (§13's last test), through the tool.
#[tokio::test]
async fn a_crlf_file_keeps_its_line_endings() {
    let b = Bench::new();
    b.put("notes.txt", b"one\r\ntwo\r\nthree\r\n");

    b.tool(None)
        .execute_with_context(&args("notes.txt", "two\nthree", "2\n3"), &b.ctx())
        .await
        .unwrap();

    assert_eq!(b.bytes("notes.txt"), b"one\r\n2\r\n3\r\n");
}

#[cfg(unix)]
#[tokio::test]
async fn an_edit_keeps_the_files_mode() {
    use std::os::unix::fs::PermissionsExt;

    let b = Bench::new();
    b.put("run.sh", b"#!/bin/sh\necho 1\n");
    fs::set_permissions(b.gp.root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();

    b.tool(None)
        .execute_with_context(&args("run.sh", "echo 1", "echo 2"), &b.ctx())
        .await
        .unwrap();

    let mode = fs::metadata(b.gp.root.join("run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
}

/// R46: a loop with no session log keeps no copy, so an existing file —
/// every file `file_edit` changes — is not changed.
#[tokio::test]
async fn without_a_session_log_an_existing_file_is_not_edited() {
    let b = Bench::new();
    b.put("notes.md", b"draft\n");

    let refused = b
        .tool(None)
        .execute_with_context(&args("notes.md", "draft", "final"), &b.gp.ctx())
        .await
        .unwrap_err();

    assert_eq!(
        refused,
        "Refusing to edit 'notes.md': it already exists, and no copy of its current content can \
         be kept here — a skill or a plugin agent keeps no conversation log. The file is \
         unchanged. Write the result to a new file, or ask for the change in the conversation \
         itself."
    );
    assert_eq!(b.bytes("notes.md"), b"draft\n");
}

/// ST-17's cap, asked before the file is read.
#[tokio::test]
async fn a_file_above_the_snapshot_cap_is_refused() {
    let b = Bench::new();
    b.put("big.txt", b"far more than eight bytes");

    let refused = b
        .tool(Some(8))
        .execute_with_context(&args("big.txt", "far", "near"), &b.ctx())
        .await
        .unwrap_err();

    assert_eq!(
        refused,
        "Refusing to edit 'big.txt': it is 25 bytes, above the 8 byte [orchestrator.sessions] \
         snapshot_max_bytes limit, so no pre-edit snapshot can be kept for it. The file is \
         unchanged. Raise snapshot_max_bytes, or copy the file aside yourself first."
    );
    assert_eq!(b.bytes("big.txt"), b"far more than eight bytes");
}

/// R48: in a project only, and the refusal names file_write.
#[tokio::test]
async fn with_no_project_file_edit_is_refused_naming_file_write() {
    let b = Bench::new();
    let ctx = ToolContext {
        session_id: Some(SESSION.to_string()),
        session_log: Some(b.service.handle_for(SESSION)),
        ..b.gp.scratch_ctx()
    };

    let refused = b
        .tool(None)
        .execute_with_context(&args("notes.md", "a", "b"), &ctx)
        .await
        .unwrap_err();

    assert_eq!(
        refused,
        "file_edit changes a file in a project, and this conversation has no project. Use \
         file_write to save a file instead."
    );
}

/// R32: `.git` is git's.
#[tokio::test]
async fn a_path_inside_dot_git_is_refused() {
    let b = Bench::new();
    b.put(".git/config", b"[core]\n\tbare = false\n");

    let refused = b
        .tool(None)
        .execute_with_context(&args(".git/config", "false", "true"), &b.ctx())
        .await
        .unwrap_err();

    assert_eq!(
        refused,
        "Writing '.git/config' is refused: it is inside the project's .git folder, which git \
         keeps. Run git through shell_execute instead."
    );
    assert_eq!(b.bytes(".git/config"), b"[core]\n\tbare = false\n");
}

#[tokio::test]
async fn a_file_that_is_not_utf8_is_refused_and_left_alone() {
    let b = Bench::new();
    b.put("logo.bin", &[0xff, 0xfe, 0x00, 0x41]);

    let refused = b
        .tool(None)
        .execute_with_context(&args("logo.bin", "A", "B"), &b.ctx())
        .await
        .unwrap_err();

    assert_eq!(
        refused,
        "'logo.bin' is not UTF-8 text, so file_edit cannot change it."
    );
    assert_eq!(b.bytes("logo.bin"), [0xff, 0xfe, 0x00, 0x41]);
}

#[tokio::test]
async fn a_folder_is_refused() {
    let b = Bench::new();
    fs::create_dir_all(b.gp.root.join("src")).unwrap();

    let refused = b
        .tool(None)
        .execute_with_context(&args("src", "a", "b"), &b.ctx())
        .await
        .unwrap_err();

    assert_eq!(refused, "'src' is a folder, not a file.");
}

/// R36: the file is the result; the row is a view of it.
#[tokio::test]
async fn without_a_database_the_edit_lands_and_says_the_app_could_not_list_it() {
    let b = Bench::new();
    b.put("src/lib.rs", LIB.as_bytes());
    let tool = FileEditTool {
        db: None,
        daemon_config: None,
    };

    let out = tool
        .execute_with_context(&args("src/lib.rs", "    3", "    5"), &b.ctx())
        .await
        .unwrap();

    assert!(
        out.ends_with(
            "Its previous content is kept in this conversation's log. The file is written, but \
             the app could not list it (no database is wired to this tool)."
        ),
        "{out}"
    );
    assert_eq!(
        b.bytes("src/lib.rs"),
        "pub fn retries() -> u32 {\n    5\n}\n".as_bytes()
    );
}

/// R26: a call with no context names no folder.
#[tokio::test]
async fn execute_without_a_context_is_refused() {
    let tool = FileEditTool {
        db: None,
        daemon_config: None,
    };
    let refused = tool.execute(&args("a.txt", "a", "b")).await.unwrap_err();
    assert_eq!(
        refused,
        crate::tools::working_folder::WorkingFolderRefusal::NoContext.to_string()
    );
}

// ── R49: the card ───────────────────────────────────────────────────────

/// §13's fifth test: a U+202E in either string is shown escaped on the card,
/// which names the folder, the file and the line.
#[tokio::test]
async fn the_card_shows_both_strings_escaped_and_names_the_folder_the_file_and_the_line() {
    let b = Bench::new();
    b.put("src/a.rs", "fn main() {}\nlet a = 1;\u{202E}\n".as_bytes());

    let card = b
        .tool(None)
        .confirmation_preview(
            &args("src/a.rs", "let a = 1;\u{202E}", "let a = 2;\u{202E}"),
            &b.ctx(),
        )
        .await
        .unwrap()
        .expect("file_edit's card carries the daemon's preview");

    assert!(!card.contains('\u{202E}'), "{card}");
    let lines: Vec<&str> = card.lines().collect();
    assert_eq!(
        lines[0],
        format!(
            "Edits src/a.rs in the project folder {}: 1 replacement, the first at line 2. Its \
             current content is kept in this conversation's log.",
            b.gp.root.display()
        )
    );
    assert_eq!(
        lines[1..].to_vec(),
        vec![
            "Replace:",
            "let a = 1;<U+202E>",
            "With:",
            "let a = 2;<U+202E>",
            "Note: this request contains 2 hidden characters, shown as <U+…>.",
        ]
    );
}

/// S4a's review (a folder at `file_write`'s target raised a card, then failed
/// EISDIR): `file_edit`'s card is built from the plan that refuses a folder,
/// so no card is raised for one — the preview answers R47's sentence.
#[tokio::test]
async fn a_folder_at_the_target_raises_no_card() {
    let b = Bench::new();
    fs::create_dir_all(b.gp.root.join("src")).unwrap();

    let refused = b
        .tool(None)
        .confirmation_preview(&args("src", "a", "b"), &b.ctx())
        .await
        .unwrap_err();

    assert_eq!(refused, "'src' is a folder, not a file.");
}

/// Interfaces §4.3: a call the body would refuse raises no card — the preview
/// refuses with the body's own sentence — and a card's plan writes nothing:
/// no copy, no row, the file as it was.
#[tokio::test]
async fn a_refused_edit_raises_no_card_and_a_card_writes_nothing() {
    let b = Bench::new();
    let before = "x = 1;\ny = 2;\nx = 1;\n";
    b.put("src/a.rs", before.as_bytes());
    let ctx = b.ctx();
    let tool = b.tool(None);

    let refused = tool
        .confirmation_preview(&args("src/a.rs", "x = 1;", "x = 3;"), &ctx)
        .await
        .unwrap_err();
    assert!(
        refused.starts_with("old_string occurs 2 times in 'src/a.rs'."),
        "{refused}"
    );

    let card = tool
        .confirmation_preview(&args("src/a.rs", "y = 2;", "y = 4;"), &ctx)
        .await
        .unwrap();
    assert!(card.is_some());
    assert_eq!(b.bytes("src/a.rs"), before.as_bytes());
    assert!(ctx.session_log.as_ref().unwrap().flush().await);
    assert!(b.images().is_empty(), "a card keeps no copy");
    assert!(b.rows().is_empty(), "a card records no row");
}

/// The card through the sandbox, as the main loop raises it: `file_edit` is
/// destructive, so a card goes out carrying the escaped strings, and a Deny
/// leaves the file as it was.
#[tokio::test]
async fn the_sandbox_raises_file_edits_card_and_a_deny_leaves_the_file() {
    use crate::agent::subagent::AgentConstraints;
    use crate::bus::EventBus;
    use crate::daemon_config::CircuitBreakerConfig;
    use crate::security::confirmation::{ConfirmationBroker, ConfirmationResponse};
    use crate::security::sandbox::{SandboxManager, SandboxPolicy};
    use crate::tools::ToolRegistry;

    let b = Bench::new();
    let before = "let a = 1;\u{202E}\n";
    b.put("src/a.rs", before.as_bytes());
    let registry = ToolRegistry::default();
    registry
        .register(file_edit_tool(Some(b.db.clone()), None))
        .unwrap();
    let mut sandbox = SandboxManager::new(
        Arc::new(registry),
        EventBus::new(16),
        &CircuitBreakerConfig::default(),
    );
    let broker = Arc::new(ConfirmationBroker::new());
    sandbox.set_confirmation_broker(broker.clone());
    let mut policy = SandboxPolicy::from_constraints(
        "orchestrator",
        &AgentConstraints {
            allowed_capabilities: vec!["file_edit".to_string()],
            ..Default::default()
        },
    );
    // Fail fast rather than wait out the 300 s default should no card come.
    policy.confirmation_timeout_secs = Some(5);
    let answering = {
        let broker = broker.clone();
        tokio::spawn(async move {
            for _ in 0..300 {
                if let Some(card) = broker.pending_requests().into_iter().next() {
                    broker
                        .respond(
                            &card.request_id,
                            ConfirmationResponse {
                                approved: false,
                                approval_scope: None,
                            },
                        )
                        .unwrap();
                    return Some(card);
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            None
        })
    };
    let call = openalpaca_llm::ToolCall {
        id: "tc-1".to_string(),
        name: "file_edit".to_string(),
        arguments: args("src/a.rs", "let a = 1;\u{202E}", "let a = 2;\u{202E}"),
    };
    let ctx = ToolContext {
        agent_id: Some("orchestrator".to_string()),
        ..b.ctx()
    };

    let refused = sandbox
        .execute_tool(&call, &policy, &ctx)
        .await
        .unwrap_err();

    let card = answering.await.unwrap().expect("a card was raised");
    let preview = card.preview.expect("the card carries file_edit's preview");
    assert!(preview.contains("let a = 1;<U+202E>"), "{preview}");
    assert!(preview.contains("let a = 2;<U+202E>"), "{preview}");
    assert!(!preview.contains('\u{202E}'), "{preview}");
    assert!(refused.contains("denied by user"), "{refused}");
    assert_eq!(b.bytes("src/a.rs"), before.as_bytes());
}
