//! `openalpaca memory`: the syntax (design §7a), `edit`'s round trip and
//! its `409` (the text kept in a named temp file), the patch an edit sends,
//! and the two local verbs under the store lock — driven with no daemon, a
//! canned loopback server for the wire, and every store a tempdir behind the
//! one `EnvSandbox`.

use super::*;
use crate::test_util::{EnvSandbox, LockHolder, json_answer, serve};
use clap::Parser;
use openalpaca_storage::Database;
use openalpaca_storage::memory_files::{Actor, NewNote, PartitionHealth, owner_key};
use tempfile::tempdir;

#[derive(Parser)]
struct Harness {
    #[command(subcommand)]
    command: MemoryCommands,
}

fn parse(args: &[&str]) -> Result<MemoryCommands, clap::Error> {
    Harness::try_parse_from(std::iter::once("openalpaca").chain(args.iter().copied()))
        .map(|h| h.command)
}

// ── Syntax ───────────────────────────────────────────────────────

/// The §7a syntax parses, `--global` and `--project` exclude each other,
/// and the two local verbs take what the design says they take.
#[test]
fn the_design_syntax_parses() {
    assert!(matches!(
        parse(&[
            "list",
            "--kind",
            "fact",
            "--history",
            "--candidates",
            "--limit",
            "5"
        ]),
        Ok(MemoryCommands::List {
            history: true,
            candidates: true,
            limit: 5,
            ..
        })
    ));
    assert!(matches!(
        parse(&[
            "search",
            "vim",
            "--historical",
            "--as-of",
            "2026-09-01T00:00:00Z",
            "--global"
        ]),
        Ok(MemoryCommands::Search {
            historical: true,
            at: ScopeArgs { global: true, .. },
            ..
        })
    ));
    assert!(matches!(
        parse(&["show", "55e811b6", "--raw", "--revision", "abc"]),
        Ok(MemoryCommands::Show { raw: true, .. })
    ));
    match parse(&[
        "add",
        "--kind",
        "preference",
        "--title",
        "Editor",
        "--alias",
        "vim",
        "--alias",
        "编辑器",
        "--tag",
        "tools",
        "--fact-key",
        "editor",
        "--supersedes",
        "55e811b6",
        "-",
    ]) {
        Ok(MemoryCommands::Add {
            aliases,
            tags,
            fact_key,
            supersedes,
            body,
            ..
        }) => {
            assert_eq!(aliases, ["vim", "编辑器"]);
            assert_eq!(tags, ["tools"]);
            assert_eq!(fact_key.as_deref(), Some("editor"));
            assert_eq!(supersedes.as_deref(), Some("55e811b6"));
            assert_eq!(body.as_deref(), Some("-"));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        parse(&["edit", "55e811b6"]),
        Ok(MemoryCommands::Edit { .. })
    ));
    assert!(matches!(
        parse(&["confirm", "55e811b6"]),
        Ok(MemoryCommands::Confirm { .. })
    ));
    assert!(matches!(
        parse(&["undo", "55e811b6"]),
        Ok(MemoryCommands::Undo { .. })
    ));
    match parse(&["forget", "55e811b6", "0c7b2e4a", "--yes"]) {
        Ok(MemoryCommands::Forget { ids, yes: true, .. }) => assert_eq!(ids.len(), 2),
        other => panic!("{other:?}"),
    }
    assert!(parse(&["forget"]).is_err(), "forget needs at least one id");
    assert!(matches!(parse(&["sync"]), Ok(MemoryCommands::Sync { .. })));
    assert!(matches!(
        parse(&["doctor", "--repair", "--project", "/tmp/p"]),
        Ok(MemoryCommands::Doctor { repair: true, .. })
    ));
    assert!(matches!(
        parse(&["adopt", "32acea7eb739b08afd959db91e46d450"]),
        Ok(MemoryCommands::Adopt { .. })
    ));
    assert!(
        parse(&["list", "--global", "--project", "/tmp/p"]).is_err(),
        "--global and --project exclude each other"
    );
}

// ── The note behind a read, and the patch an edit sends ────────────────

const ID: &str = "55e811b6-3220-4758-a47b-034bd1023f38";
const OLD_REV: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const NEW_REV: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const OTHER_REV: &str = "3333333333333333333333333333333333333333333333333333333333333333";

fn read_answer() -> serde_json::Value {
    serde_json::json!({
        "id": ID,
        "revision": OLD_REV,
        "title": "Build and test commands for this workspace",
        "kind": "procedure",
        "scope": "project",
        "status": "active",
        "effective_status": "active",
        "origin": "user",
        "frontmatter": {
            "schema": 1,
            "id": ID,
            "title": "Build and test commands for this workspace",
            "kind": "procedure",
            "status": "active",
            "fact_key": "build.validation",
            "aliases": ["checks", "validation", "测试", "编译"],
            "tags": ["development"],
            "created_at": "2026-09-23T10:00:00Z",
            "updated_at": "2026-09-23T10:00:00Z",
            "observed_at": "2026-09-23T09:55:00Z",
            "valid_from": null,
            "valid_to": null,
            "review_after": null,
            "pinned": false,
            "sources": [{ "type": "task", "id": "7c1e0b2a-4d61-4f0e-9a3c-2b7f1d5e8c90", "observed_at": "2026-09-23T09:55:00Z" }],
            "supersedes": [],
        },
        "body": "# Build and test commands for this workspace\n\n## Knowledge\nRun the documented focused checks for the component being changed.\n",
        "truncated": false,
        "path": "notes/55e811b6-3220-4758-a47b-034bd1023f38--build-and-test-commands-for-this-workspace.md",
        "sources": [{ "type": "task", "id": "7c1e0b2a-4d61-4f0e-9a3c-2b7f1d5e8c90", "observed_at": "2026-09-23T09:55:00Z" }],
        "sources_status": "intact",
        "supersedes": [],
        "superseded_by": [],
        "freshness": "verified",
        "conflict": false,
    })
}

/// The read result rebuilds into the canon of design §4 — the file the
/// daemon holds, byte for byte the way the service writes it — and parses
/// back to the same note.
#[test]
fn a_read_result_rebuilds_the_note_file_in_the_canon() {
    let note = note_from_read(&read_answer()).unwrap();
    let text = note.serialize();
    assert!(text.starts_with("---\nschema: 1\nid: 55e811b6-3220-4758-a47b-034bd1023f38\ntitle: Build and test commands for this workspace\nkind: procedure\nstatus: active\nfact_key: build.validation\naliases:\n  - checks\n  - validation\n  - 测试\n  - 编译\ntags:\n  - development\n"), "{text}");
    assert!(text.contains("sources:\n  - type: task\n    id: 7c1e0b2a-4d61-4f0e-9a3c-2b7f1d5e8c90\n    observed_at: 2026-09-23T09:55:00Z\nsupersedes: []\n---\n# Build and test commands"), "{text}");
    assert_eq!(Note::parse(text.as_bytes()).unwrap(), note);
}

/// The patch is exactly the fields that changed, a cleared `fact_key` is an
/// explicit `null`, an unchanged note is an empty patch, and a change to
/// what an edit cannot change is refused by name.
#[test]
fn the_patch_is_the_changed_patchable_fields_and_nothing_else() {
    let before = note_from_read(&read_answer()).unwrap();
    assert_eq!(
        patch_between(&before, &before).unwrap(),
        serde_json::json!({})
    );

    let mut after = before.clone();
    after.title = "Build checks".into();
    after.body = "# Build checks\n\nRun `cargo test`.\n".into();
    after.aliases = vec!["checks".into()];
    after.fact_key = None;
    after.valid_to = Some("2026-12-31T00:00:00Z".parse().unwrap());
    let patch = patch_between(&before, &after).unwrap();
    assert_eq!(
        patch,
        serde_json::json!({
            "title": "Build checks",
            "body": "# Build checks\n\nRun `cargo test`.\n",
            "aliases": ["checks"],
            "fact_key": null,
            "valid_to": "2026-12-31T00:00:00Z",
        })
    );

    let mut kind = before.clone();
    kind.kind = NoteKind::Fact;
    let err = patch_between(&before, &kind).unwrap_err();
    assert!(
        err.contains("`kind` cannot be changed") && err.contains("--supersedes"),
        "{err}"
    );
    let mut status = before.clone();
    status.status = NoteStatus::Archived;
    assert!(
        patch_between(&before, &status)
            .unwrap_err()
            .contains("`status`")
    );
    let mut pinned = before.clone();
    pinned.pinned = true;
    assert!(
        patch_between(&before, &pinned)
            .unwrap_err()
            .contains("`pinned`")
    );
    let mut sources = before.clone();
    sources.sources.clear();
    assert!(
        patch_between(&before, &sources)
            .unwrap_err()
            .contains("`sources`")
    );
    let mut stamped = before.clone();
    stamped.updated_at = "2027-01-01T00:00:00Z".parse().unwrap();
    assert!(
        patch_between(&before, &stamped)
            .unwrap_err()
            .contains("updated_at")
    );
}

// ── edit's round trip over the wire ──────────────────────────────

fn read_wire() -> String {
    json_answer("200 OK", OLD_REV, &read_answer().to_string())
}

fn change_body(path: &Path) -> Result<()> {
    let text = std::fs::read_to_string(path)?;
    let (front, _body) = text.rsplit_once("---\n").unwrap();
    std::fs::write(
        path,
        format!("{front}---\n# Build checks\n\nRun `cargo test` first.\n"),
    )?;
    Ok(())
}

/// The round trip: GET, the editor's change, a POST carrying `If-Match`
/// with the fetched revision and a patch of only the body, and the new
/// revision back; the temp file is gone afterwards.
#[tokio::test]
async fn edit_posts_the_patch_with_the_fetched_revision_as_if_match() {
    let (addr, server) = serve(vec![
        read_wire(),
        json_answer(
            "200 OK",
            NEW_REV,
            &serde_json::json!({ "id": ID, "revision": NEW_REV, "status": "active" }).to_string(),
        ),
    ]);
    let api = Api::for_tests(DaemonClient::for_tests(&format!("http://{addr}"), "token"));
    let touched = std::sync::Mutex::new(None);
    let editor = |p: &Path| {
        *touched.lock().unwrap() = Some(p.to_path_buf());
        change_body(p)
    };
    let outcome = edit_note(&api, NoteId::parse(ID).unwrap(), &editor)
        .await
        .expect("the edit runs");
    assert_eq!(
        outcome,
        EditOutcome::Saved {
            revision: NEW_REV.to_string()
        }
    );
    let temp = touched
        .lock()
        .unwrap()
        .clone()
        .expect("the editor was opened");
    assert!(!temp.exists(), "a saved edit leaves no temp file behind");
    assert!(
        temp.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("openalpaca-memory-55e811b6-"),
        "{}",
        temp.display()
    );

    let requests = server.join().expect("the server finishes");
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0].starts_with(&format!("GET /v1/memories/{ID} ")),
        "{}",
        requests[0]
    );
    let post = &requests[1];
    assert!(
        post.starts_with(&format!("POST /v1/memories/{ID} ")),
        "{post}"
    );
    assert!(
        post.to_ascii_lowercase()
            .contains(&format!("if-match: \"{OLD_REV}\"")),
        "the POST carries the fetched revision as If-Match:\n{post}"
    );
    let body_start = post.find("\r\n\r\n").unwrap() + 4;
    let body: serde_json::Value = serde_json::from_str(&post[body_start..]).unwrap();
    assert_eq!(body["action"], "edit");
    assert_eq!(
        body["patch"],
        serde_json::json!({ "body": "# Build checks\n\nRun `cargo test` first.\n" })
    );
    assert!(
        body.get("expected_revision").is_none(),
        "the header, never a body field"
    );
}

/// **Plan 3.8's `edit` rule.** On the daemon's 409 the edited text is kept
/// in a named temp file, and both revisions are reported.
#[tokio::test]
async fn a_409_keeps_the_edited_text_in_a_named_temp_file() {
    let conflict = serde_json::json!({
        "error": {
            "code": "MEMORY_REVISION_CONFLICT",
            "message": format!("note {ID} changed since it was read: current revision is {OTHER_REV}"),
        }
    })
    .to_string();
    let (addr, server) = serve(vec![
        read_wire(),
        json_answer("409 Conflict", OTHER_REV, &conflict),
    ]);
    let api = Api::for_tests(DaemonClient::for_tests(&format!("http://{addr}"), "token"));
    let outcome = edit_note(&api, NoteId::parse(ID).unwrap(), &change_body)
        .await
        .expect("a 409 is an outcome, not an error");
    let EditOutcome::Conflict {
        ours,
        theirs,
        kept_at,
    } = outcome
    else {
        panic!("expected a conflict, got {outcome:?}");
    };
    assert_eq!(ours, OLD_REV);
    assert_eq!(theirs, OTHER_REV);
    assert!(
        kept_at.exists(),
        "the edited text is kept at {}",
        kept_at.display()
    );
    let kept = std::fs::read_to_string(&kept_at).unwrap();
    assert!(kept.ends_with("Run `cargo test` first.\n"), "{kept}");
    assert!(
        kept_at
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("openalpaca-memory-55e811b6-"),
        "{}",
        kept_at.display()
    );
    std::fs::remove_file(&kept_at).ok();
    server.join().expect("the server finishes");
}

/// A change to a field an edit cannot change sends nothing, and keeps the
/// text; an editor that changes nothing sends nothing.
#[tokio::test]
async fn a_refused_edit_and_an_unchanged_one_send_nothing() {
    let (addr, server) = serve(vec![read_wire()]);
    let api = Api::for_tests(DaemonClient::for_tests(&format!("http://{addr}"), "token"));
    let editor = |p: &Path| {
        let text = std::fs::read_to_string(p)?;
        std::fs::write(p, text.replace("kind: procedure", "kind: fact"))?;
        Ok(())
    };
    let outcome = edit_note(&api, NoteId::parse(ID).unwrap(), &editor)
        .await
        .unwrap();
    let EditOutcome::Refused { reason, kept_at } = outcome else {
        panic!("expected a refusal, got {outcome:?}");
    };
    assert!(reason.contains("`kind`"), "{reason}");
    assert!(kept_at.exists());
    std::fs::remove_file(&kept_at).ok();
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 1, "only the GET");

    let (addr, server) = serve(vec![read_wire()]);
    let api = Api::for_tests(DaemonClient::for_tests(&format!("http://{addr}"), "token"));
    let outcome = edit_note(&api, NoteId::parse(ID).unwrap(), &|_: &Path| Ok(()))
        .await
        .unwrap();
    assert_eq!(outcome, EditOutcome::Unchanged);
    assert_eq!(server.join().unwrap().len(), 1);
}

/// A read marked `truncated` — a note longer than the 32 KiB a read serves —
/// is refused before the editor opens: the editor never runs, no temp file
/// is made, nothing but the GET reaches the daemon, and the refusal names
/// the note's file from the read's `path` (its id when there is none).
/// Saving the cut body would have replaced the whole file with its first
/// 32 KiB.
#[tokio::test]
async fn a_note_cut_at_32_kib_is_refused_before_the_editor_opens() {
    let path = read_answer()["path"].as_str().unwrap().to_string();
    for (with_path, expected) in [(true, path.as_str()), (false, ID)] {
        let mut answer = read_answer();
        answer["truncated"] = serde_json::json!(true);
        if !with_path {
            answer.as_object_mut().unwrap().remove("path");
        }
        // One answer only: a second request would find the port closed.
        let (addr, server) = serve(vec![json_answer("200 OK", OLD_REV, &answer.to_string())]);
        let api = Api::for_tests(DaemonClient::for_tests(&format!("http://{addr}"), "token"));
        let opened = std::sync::atomic::AtomicBool::new(false);
        // What a user trimming the cut body would do: the data-loss save.
        let editor = |p: &Path| {
            opened.store(true, std::sync::atomic::Ordering::SeqCst);
            change_body(p)
        };
        let result = edit_note(&api, NoteId::parse(ID).unwrap(), &editor).await;
        assert!(
            !opened.load(std::sync::atomic::Ordering::SeqCst),
            "the editor must not open on a body cut at 32 KiB (with_path: {with_path})"
        );
        let outcome = result.expect("a refusal is an outcome, not an error");
        assert_eq!(
            outcome,
            EditOutcome::TooLong {
                path: expected.to_string()
            }
        );
        let requests = server.join().expect("the server finishes");
        assert_eq!(requests.len(), 1, "only the GET: {requests:?}");
        assert!(
            requests[0].starts_with(&format!("GET /v1/memories/{ID} ")),
            "{}",
            requests[0]
        );
    }
    assert_eq!(
        too_long_to_edit(&path),
        "This note is longer than 32 KiB, so `openalpaca memory edit` would save only its first 32 KiB. Edit the file itself: notes/55e811b6-3220-4758-a47b-034bd1023f38--build-and-test-commands-for-this-workspace.md"
    );
}

// ── The local verbs ──────────────────────────────────────────────

/// The sandboxed store's database with the owner the daemon would have
/// minted, so the local verbs have an owner id to read under the lock.
fn seed_owner(owner_id: &str) {
    let path = store::database_path().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = Database::open(&path).unwrap();
    ConfigRepository::new(&db)
        .set("identity.local_user_id", owner_id, "string")
        .unwrap();
}

/// Both local verbs refuse while another process holds the daemon's lock
/// — a running, or still booting, daemon — and change nothing.
#[test]
fn repair_and_adopt_refuse_while_the_store_lock_is_held() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    seed_owner("reborn");
    let root = store::home_root().unwrap();
    std::fs::create_dir_all(root.join("state")).unwrap();
    let _holder = LockHolder::spawn(&root);

    let err = repair_locally(&ScopeArgs {
        global: true,
        ..Default::default()
    })
    .unwrap_err();
    assert!(format!("{err:#}").contains("lock"), "{err:#}");
    let err = adopt_locally("32acea7eb739b08afd959db91e46d450").unwrap_err();
    assert!(format!("{err:#}").contains("lock"), "{err:#}");
}

/// `doctor --repair` rebuilds a lost snapshot and lifts the quarantine;
/// `adopt` re-binds an orphaned owner key's partition to the current owner.
#[test]
fn repair_lifts_a_quarantine_and_adopt_rebinds_an_orphan() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    let previous = "u_example_alice";
    let current = "u_example_alice_reborn";
    seed_owner(current);

    // The current owner's home partition, its snapshot then deleted.
    let mine = Partition::open(&StoreScope::Home, current).unwrap();
    mine.create(
        NewNote {
            title: "Editor".into(),
            body: "# Editor\n\nvim.\n".into(),
            kind: Some(NoteKind::Preference),
            ..Default::default()
        },
        Actor::UserSurface,
    )
    .unwrap();
    std::fs::remove_file(mine.snapshot_path()).unwrap();
    drop(mine);
    assert!(matches!(
        Partition::open(&StoreScope::Home, current)
            .unwrap()
            .health(),
        PartitionHealth::Quarantined(_)
    ));
    repair_locally(&ScopeArgs {
        global: true,
        ..Default::default()
    })
    .unwrap();
    let repaired = Partition::open(&StoreScope::Home, current).unwrap();
    assert_eq!(repaired.health(), PartitionHealth::Ready);
    assert_eq!(repaired.snapshot().unwrap().notes.len(), 1);
    drop(repaired);

    // A previous owner's partition — orphaned by a factory reset.
    let theirs = Partition::open(&StoreScope::Home, previous).unwrap();
    theirs
        .create(
            NewNote {
                title: "Shell".into(),
                body: "# Shell\n\nzsh.\n".into(),
                kind: Some(NoteKind::Preference),
                ..Default::default()
            },
            Actor::UserSurface,
        )
        .unwrap();
    drop(theirs);
    let old_key = owner_key(previous);
    let new_key = owner_key(current);
    // A collision: the current owner already has a home partition.
    let err = adopt_locally(&old_key).unwrap_err();
    assert!(format!("{err:#}").contains("already exists"), "{err:#}");

    // With the current owner's partition gone, the adoption goes through.
    let owners = store::home_root().unwrap().join("memory").join("owners");
    std::fs::remove_dir_all(owners.join(&new_key)).unwrap();
    std::fs::remove_file(files::snapshot_path_for(&format!("home-{new_key}")).unwrap()).unwrap();
    adopt_locally(&old_key).unwrap();
    assert!(!owners.join(&old_key).exists());
    let adopted = Partition::open(&StoreScope::Home, current).unwrap();
    assert_eq!(adopted.health(), PartitionHealth::Ready);
    let loaded = adopted.load().unwrap();
    assert_eq!(loaded.views.len(), 1);
    assert_eq!(loaded.views[0].note.title, "Shell");
    assert!(loaded.views[0].derived.recorded);

    assert!(
        adopt_locally("not-a-key")
            .unwrap_err()
            .to_string()
            .contains("not an owner key")
    );
}

/// R22: `doctor` prints one line per project snapshot that carries no
/// project id — its key and the directory it was bound to — and says that
/// nothing opens, adopts or deletes it.
#[test]
fn the_doctor_names_an_idless_project_snapshot_and_that_nothing_deletes_it() {
    let line = idless_snapshot_line(&json!({
        "partition_key": "proj-32acea7eb739b08afd959db91e46d450-0123456789abcdef",
        "root": "/repo/.openalpaca/memory/owners/32acea7eb739b08afd959db91e46d450",
    }));
    assert_eq!(
        line,
        "project snapshot proj-32acea7eb739b08afd959db91e46d450-0123456789abcdef carries no \
         project id: an earlier build wrote it for \
         /repo/.openalpaca/memory/owners/32acea7eb739b08afd959db91e46d450. It is never opened \
         or adopted, and nothing deletes it."
    );
}

/// R12: `doctor --repair` in a git repository no turn has registered has no
/// project partition to repair. It answers from the home notes and creates
/// nothing — no folder under `projects/`, nothing in the repository.
#[test]
fn repair_answers_from_the_home_notes_for_an_unregistered_project() {
    let tmp = tempdir().unwrap();
    let _env = EnvSandbox::enter(tmp.path());
    seed_owner("u_example_alice");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();

    repair_locally(&ScopeArgs {
        project: Some(repo.clone()),
        ..Default::default()
    })
    .unwrap();

    assert!(!store::home_root().unwrap().join("projects").exists());
    assert!(!repo.join(".openalpaca").exists());
}

// ── The list page's "more" hint ──────────────────────────────────

/// A page with a `next_cursor` gets one hint, and the hint names only a
/// flag `list` takes: `--limit` and its ceiling. `list` has no `--cursor`
/// (§7a), so the hint never tells the user to continue from one — a
/// continuation the verb would refuse.
#[test]
fn the_list_hint_names_only_what_list_can_take() {
    assert_eq!(list_more_hint(&json!({ "items": [] })), None);
    let hint = list_more_hint(&json!({ "items": [], "next_cursor": "eyJvIjoi" }))
        .expect("a page with more gets a hint");
    assert!(hint.contains("--limit"), "{hint}");
    assert!(hint.contains("100"), "{hint}");
    assert!(
        !hint.to_lowercase().contains("cursor"),
        "list takes no cursor: {hint}"
    );
    assert!(
        parse(&["list", "--cursor", "eyJvIjoi"]).is_err(),
        "and the flag really does not exist"
    );
}

// ── Suggestions, confirm and undo (Learning v2, design 09 §3.5) ─────────

const SUGGESTION: &str = "b1c2d3e4-0000-4000-8000-000000000001";
const TARGET: &str = "a1b2c3d4-0000-4000-8000-000000000002";
const TARGET_REV: &str = "4444444444444444444444444444444444444444444444444444444444444444";

fn suggestion_read(supersedes: &[&str]) -> serde_json::Value {
    let mut read = read_answer();
    read["id"] = serde_json::json!(SUGGESTION);
    read["title"] = serde_json::json!("Commute");
    read["status"] = serde_json::json!("candidate");
    read["origin"] = serde_json::json!("extracted_user");
    read["supersedes"] = serde_json::json!(supersedes);
    read
}

fn target_read() -> serde_json::Value {
    let mut read = read_answer();
    read["id"] = serde_json::json!(TARGET);
    read["revision"] = serde_json::json!(TARGET_REV);
    read["title"] = serde_json::json!("My commute");
    read
}

fn post_body(request: &str) -> serde_json::Value {
    let start = request.find("\r\n\r\n").unwrap() + 4;
    serde_json::from_str(&request[start..]).unwrap()
}

#[test]
fn the_suggestions_verb_parses() {
    assert!(matches!(
        parse(&["suggestions", "--limit", "5", "--global"]),
        Ok(MemoryCommands::Suggestions {
            limit: 5,
            at: ScopeArgs { global: true, .. },
            ..
        })
    ));
    assert!(matches!(
        parse(&["suggestions"]),
        Ok(MemoryCommands::Suggestions { limit: 20, .. })
    ));
}

/// Design 09 §3.5's line: `<id8>  replace “X” (saved by you) → “B”  from
/// your words, <date> — <reason>`, the older ones counted; an inferred one
/// says so.
#[test]
fn a_suggestion_line_names_both_notes_whose_words_and_why() {
    let item = serde_json::json!({
        "id": SUGGESTION,
        "title": "Commute",
        "reason": "The user said they cycle now.",
        "origin": "extracted_user",
        "created_at": "2026-09-25T10:00:00Z",
        "observed_at": "2026-09-24T18:02:11Z",
        "older_count": 2,
        "target": { "id": TARGET, "title": "My commute", "origin": "user" },
    });
    assert_eq!(
        suggestion_line(&item),
        "b1c2d3e4  replace “My commute” (saved by you) → “Commute”  from your words, 2026-09-24 — The user said they cycle now.  (+2 older)"
    );
    let mut inferred = item.clone();
    inferred["origin"] = serde_json::json!("extracted_other");
    inferred["observed_at"] = serde_json::Value::Null;
    inferred["reason"] = serde_json::Value::Null;
    inferred["older_count"] = serde_json::json!(0);
    inferred["target"]["origin"] = serde_json::json!("external");
    assert_eq!(
        suggestion_line(&inferred),
        "b1c2d3e4  replace “My commute” (edited by hand) → “Commute”  inferred from the conversation, 2026-09-25"
    );
    // Owner decision LQ11: one the assistant's memory tools saved in chat —
    // no `observed_at`, which learning stamps on every note it writes.
    let mut in_chat = item.clone();
    in_chat["observed_at"] = serde_json::Value::Null;
    in_chat["older_count"] = serde_json::json!(0);
    assert_eq!(
        suggestion_line(&in_chat),
        "b1c2d3e4  replace “My commute” (saved by you) → “Commute”  suggested by the assistant, 2026-09-25 — The user said they cycle now."
    );
}

/// A suggestion's own text can change after learning wrote it — the owner
/// edited it (origin `user`), or its file was edited outside OpenAlpaca
/// (`external`) or imported — and the line then says who wrote the words
/// and when it was suggested, never the daemon's raw origin word.
#[test]
fn a_suggestion_line_says_who_wrote_an_edited_suggestion() {
    let item = serde_json::json!({
        "id": SUGGESTION,
        "title": "Commute",
        "reason": null,
        "origin": "user",
        "created_at": "2026-09-25T10:00:00Z",
        "observed_at": "2026-09-24T18:02:11Z",
        "older_count": 0,
        "target": { "id": TARGET, "title": "My commute", "origin": "user" },
    });
    let line = |origin: &str| {
        let mut item = item.clone();
        item["origin"] = serde_json::json!(origin);
        suggestion_line(&item)
    };
    assert_eq!(
        line("user"),
        "b1c2d3e4  replace “My commute” (saved by you) → “Commute”  edited by you, suggested 2026-09-24"
    );
    assert_eq!(
        line("external"),
        "b1c2d3e4  replace “My commute” (saved by you) → “Commute”  edited outside OpenAlpaca, suggested 2026-09-24"
    );
    assert_eq!(
        line("imported"),
        "b1c2d3e4  replace “My commute” (saved by you) → “Commute”  imported, suggested 2026-09-24"
    );
}

/// S5 from the CLI: `confirm` of a suggestion fetches the note it replaces
/// and sends that note's current revision as `target_revision`, beside the
/// suggestion's own as `If-Match`, and reports what it replaced and
/// dismissed.
#[tokio::test]
async fn confirm_of_a_suggestion_sends_the_targets_revision() {
    let confirmed = serde_json::json!({
        "id": SUGGESTION,
        "revision": NEW_REV,
        "status": "active",
        "superseded": [TARGET],
        "dismissed": ["c0ffee00-0000-4000-8000-000000000003"],
    });
    let (addr, server) = serve(vec![
        json_answer("200 OK", OLD_REV, &suggestion_read(&[TARGET]).to_string()),
        json_answer("200 OK", TARGET_REV, &target_read().to_string()),
        json_answer("200 OK", NEW_REV, &confirmed.to_string()),
    ]);
    let api = Api::for_tests(DaemonClient::for_tests(&format!("http://{addr}"), "token"));
    let id = NoteId::parse(SUGGESTION).unwrap();
    let out = confirm_note(&api, id).await.expect("the confirm runs");
    assert_eq!(out.replaced_title.as_deref(), Some("My commute"));
    let lines = out.lines(id);
    assert!(lines[0].contains("now replaces “My commute”"), "{lines:?}");
    assert!(
        lines[1].contains("Dismissed 1 other suggestion") && lines[1].contains("c0ffee00"),
        "{lines:?}"
    );

    let requests = server.join().expect("the server finishes");
    assert!(requests[0].starts_with(&format!("GET /v1/memories/{SUGGESTION} ")));
    assert!(requests[1].starts_with(&format!("GET /v1/memories/{TARGET} ")));
    let post = &requests[2];
    assert!(
        post.starts_with(&format!("POST /v1/memories/{SUGGESTION} ")),
        "{post}"
    );
    assert!(
        post.to_ascii_lowercase()
            .contains(&format!("if-match: \"{OLD_REV}\"")),
        "{post}"
    );
    let body = post_body(post);
    assert_eq!(body["action"], "confirm");
    assert_eq!(body["target_revision"], TARGET_REV);
}

/// A candidate that names no note confirms as before: no second read, no
/// `target_revision`.
#[tokio::test]
async fn confirm_of_a_plain_candidate_sends_no_target_revision() {
    let confirmed = serde_json::json!({
        "id": SUGGESTION,
        "revision": NEW_REV,
        "status": "active",
        "superseded": [],
        "dismissed": [],
    });
    let (addr, server) = serve(vec![
        json_answer("200 OK", OLD_REV, &suggestion_read(&[]).to_string()),
        json_answer("200 OK", NEW_REV, &confirmed.to_string()),
    ]);
    let api = Api::for_tests(DaemonClient::for_tests(&format!("http://{addr}"), "token"));
    let id = NoteId::parse(SUGGESTION).unwrap();
    let out = confirm_note(&api, id).await.expect("the confirm runs");
    assert!(out.replaced_title.is_none());
    assert_eq!(out.lines(id).len(), 1);
    let requests = server.join().expect("the server finishes");
    assert_eq!(requests.len(), 2);
    let body = post_body(&requests[1]);
    assert!(body.get("target_revision").is_none(), "{body}");
}

/// Design 09 §3.3: `undo` of a suggestion says the owner's note is kept;
/// of a learned replacement, that the note it replaced is back.
#[test]
fn undo_says_what_a_withdrawal_did() {
    let id = NoteId::parse(SUGGESTION).unwrap();
    let withdrawn = serde_json::json!({ "id": SUGGESTION, "status": "withdrawn", "restored": [] });
    assert_eq!(
        undo_message(id, &suggestion_read(&[TARGET]), &withdrawn),
        format!("Suggestion {SUGGESTION} (Commute) dismissed; your note is kept as it is.")
    );
    let mut learned = suggestion_read(&[TARGET]);
    learned["status"] = serde_json::json!("active");
    let back = serde_json::json!({ "id": SUGGESTION, "status": "withdrawn", "restored": [TARGET] });
    assert_eq!(
        undo_message(id, &learned, &back),
        format!("Note {SUGGESTION} (Commute) undone; the note it replaced is back.")
    );
    assert_eq!(
        undo_message(id, &suggestion_read(&[]), &withdrawn),
        format!("Candidate {SUGGESTION} (Commute) dismissed.")
    );
}

// ── search filters and timeline (design 09 §13.1, slice L10a) ─────────

/// **L10a evidence: the CLI mirrors the tool.** `search` takes `--since`,
/// `--until` and `--order` beside the old flags, its limit default is the
/// tool's 10; `timeline` takes exactly one of an id, `--query` and `--date`.
#[test]
fn the_search_filters_and_timeline_parse() {
    match parse(&[
        "search",
        "office",
        "--since",
        "2026-06-01",
        "--until",
        "2026-06-30T12:00:00Z",
        "--order",
        "newest",
    ]) {
        Ok(MemoryCommands::Search {
            since,
            until,
            order,
            limit,
            ..
        }) => {
            assert_eq!(since.as_deref(), Some("2026-06-01"));
            assert_eq!(until.as_deref(), Some("2026-06-30T12:00:00Z"));
            assert_eq!(order.as_deref(), Some("newest"));
            assert_eq!(limit, 10);
        }
        other => panic!("{other:?}"),
    }
    match parse(&["timeline", "55e811b6"]) {
        Ok(MemoryCommands::Timeline {
            id,
            query,
            date,
            before,
            after,
            ..
        }) => {
            assert_eq!(id.as_deref(), Some("55e811b6"));
            assert!(query.is_none() && date.is_none());
            assert_eq!((before, after), (5, 5));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        parse(&["timeline", "--query", "office move", "--global"]),
        Ok(MemoryCommands::Timeline {
            query: Some(_),
            at: ScopeArgs { global: true, .. },
            ..
        })
    ));
    assert!(matches!(
        parse(&[
            "timeline",
            "--date",
            "2026-06-10",
            "--before",
            "2",
            "--after",
            "3"
        ]),
        Ok(MemoryCommands::Timeline {
            date: Some(_),
            before: 2,
            after: 3,
            ..
        })
    ));
    assert!(parse(&["timeline"]).is_err(), "an anchor is required");
    assert!(
        parse(&["timeline", "55e811b6", "--date", "2026-06-10"]).is_err(),
        "one anchor, not two"
    );
}

/// The route's names for the tool's filters, each sent only when given.
#[test]
fn search_sends_the_tools_filters_by_their_route_names() {
    let plain = SearchFilters {
        query: "office move",
        kind: None,
        scope: None,
        limit: 10,
        historical: false,
        as_of: None,
        since: None,
        until: None,
        order: None,
    };
    assert_eq!(
        search_path(&plain),
        "/v1/memories/search?q=office%20move&limit=10"
    );
    let all = SearchFilters {
        kind: Some("decision"),
        historical: true,
        since: Some("2026-06-01"),
        until: Some("2026-06-30T12:00:00+02:00"),
        order: Some("oldest"),
        ..plain
    };
    assert_eq!(
        search_path(&all),
        "/v1/memories/search?q=office%20move&limit=10&kind=decision\
         &date_from=2026-06-01&date_to=2026-06-30T12%3A00%3A00%2B02%3A00&order_by=oldest\
         &historical=true"
    );
}

/// `timeline` asks `GET /v1/memories/timeline` for the rows around its
/// anchor — a full id as `anchor`, a search as `q`, a date as `date` — with
/// `before` and `after`.
#[tokio::test]
async fn timeline_asks_the_daemon_for_the_rows_around_its_anchor() {
    let answer = serde_json::json!({
        "anchor": {"id": ID}, "results": [], "count": 0, "index_state": "ready"
    })
    .to_string();
    let (addr, server) = serve(vec![
        json_answer("200 OK", OLD_REV, &answer),
        json_answer("200 OK", OLD_REV, &answer),
        json_answer("200 OK", OLD_REV, &answer),
    ]);
    let api = Api::for_tests(DaemonClient::for_tests(&format!("http://{addr}"), "token"));
    timeline(&api, Some(ID), None, None, 5, 5).await.unwrap();
    timeline(&api, None, Some("office move"), None, 2, 0)
        .await
        .unwrap();
    timeline(&api, None, None, Some("2026-06-10"), 5, 20)
        .await
        .unwrap();
    let requests = server.join().expect("the server finishes");
    let lines: Vec<&str> = requests.iter().map(|r| r.lines().next().unwrap()).collect();
    assert_eq!(
        lines,
        vec![
            format!("GET /v1/memories/timeline?anchor={ID}&before=5&after=5 HTTP/1.1"),
            "GET /v1/memories/timeline?q=office%20move&before=2&after=0 HTTP/1.1".to_string(),
            "GET /v1/memories/timeline?date=2026-06-10&before=5&after=20 HTTP/1.1".to_string(),
        ]
    );
}

// ── What reaches the terminal (residue review finding 3) ─────────────────

/// A title with a screen-clearing escape sequence, a bidi override and a
/// zero-width space — what a learned or hand-edited note can hold.
const EVIL: &str = "Evil\u{1b}[2J \u{202e}eltit\u{202c} ti\u{200b}tle";
/// The same title as it may reach a terminal: the escape and format
/// characters dropped, the visible text kept in its stored order.
const SHOWN: &str = "Evil[2J eltit title";

/// No escape sequence from the note and no format character reaches the
/// text. (The colours `colored` adds are its own escapes, so the check is
/// for the note's sequence, not for every ESC.)
fn assert_clean(text: &str) {
    assert!(!text.contains("\u{1b}[2J"), "an escape sequence: {text:?}");
    assert!(!text.contains('\u{202e}'), "a bidi override: {text:?}");
    assert!(!text.contains('\u{202c}'), "a bidi pop: {text:?}");
    assert!(!text.contains('\u{200b}'), "a zero-width space: {text:?}");
}

/// The same title as a suggestion's line shows it: every hidden character
/// as its code point, nothing dropped.
const ESCAPED: &str = "Evil<U+001B>[2J <U+202E>eltit<U+202C> ti<U+200B>tle";

/// The lines a person decides from — a suggestion to confirm or keep, a
/// note to forget or undo, what a confirm replaced — carry a note's title
/// and the suggestion's reason with no escape or format character: the
/// suggestion's line shows each as `<U+XXXX>`, the others drop them.
#[test]
fn the_decision_lines_carry_no_escape_or_format_character() {
    let item = serde_json::json!({
        "id": SUGGESTION,
        "title": EVIL,
        "reason": format!("Because \u{1b}]0;owned\u{7}{EVIL}"),
        "origin": "extracted_user",
        "created_at": "2026-09-25T10:00:00Z",
        "target": { "id": TARGET, "title": EVIL, "origin": "user" },
    });
    let line = suggestion_line(&item);
    assert_clean(&line);
    assert!(!line.contains('\u{7}'), "{line:?}");
    assert!(
        line.contains(&format!("replace “{ESCAPED}” (saved by you) → “{ESCAPED}”")),
        "{line}"
    );
    assert!(
        line.contains(&format!("— Because <U+001B>]0;owned<U+0007>{ESCAPED}")),
        "{line}"
    );

    let id = NoteId::parse(SUGGESTION).unwrap();
    let mut before = suggestion_read(&[TARGET]);
    before["title"] = serde_json::json!(EVIL);
    let withdrawn = serde_json::json!({ "id": SUGGESTION, "status": "withdrawn", "restored": [] });
    let undone = undo_message(id, &before, &withdrawn);
    assert_clean(&undone);
    assert!(undone.contains(&format!("({SHOWN})")), "{undone}");

    let forget = forget_line(id, &before, OLD_REV);
    assert_clean(&forget);
    assert!(forget.contains(SHOWN), "{forget}");

    let confirmed = Confirmed {
        out: serde_json::json!({ "id": SUGGESTION, "revision": NEW_REV, "status": "active" }),
        replaced_title: Some(EVIL.to_string()),
    };
    for line in confirmed.lines(id) {
        assert_clean(&line);
    }
    assert!(confirmed.lines(id)[0].contains(&format!("“{SHOWN}”")));
}

/// `memory list`, `memory search` and `memory timeline` print a title per
/// line, and search an excerpt: none of it carries an escape or format
/// character. An excerpt keeps its own lines and indentation.
#[test]
fn list_search_and_timeline_lines_carry_no_escape_or_format_character() {
    let row = serde_json::json!({
        "id": ID, "kind": "fact", "scope": "global", "status": "active",
        "effective_status": "active", "origin": "user", "updated_at": "2026-09-25T10:00:00Z",
        "date": "2026-09-25T10:00:00Z", "title": EVIL,
        "heading": format!("Heading {EVIL}"),
        "excerpt": format!("first {EVIL}\n\tsecond\u{1b}[31m line\r\n"),
        "line_range": [3, 4], "match": ["term"], "freshness": "verified",
    });
    let listed = list_lines(&serde_json::json!({ "items": [row.clone()] }));
    for line in &listed {
        assert_clean(line);
    }
    assert!(listed[1].ends_with(SHOWN), "{listed:?}");

    let found = search_lines(&serde_json::json!({ "results": [row.clone()] }));
    for line in &found {
        assert_clean(line);
        assert!(!line.contains("\u{1b}[31m"), "{line:?}");
        assert!(!line.contains('\r'), "{line:?}");
    }
    assert!(found[0].contains(SHOWN), "{found:?}");
    assert!(found[1].contains(&format!("Heading {SHOWN}")), "{found:?}");
    assert_eq!(found[2], format!("    first {SHOWN}"), "{found:?}");
    assert_eq!(found[3], "    \tsecond[31m line", "{found:?}");

    let timeline = timeline_lines(&serde_json::json!({ "results": [row] }));
    for line in &timeline {
        assert_clean(line);
    }
    assert!(timeline[0].contains(SHOWN), "{timeline:?}");
}

/// `memory show` prints the title, the facts line and the body. The title
/// and facts carry no escape or format character; the body — the note's
/// whole text, which the person asked to read — keeps its format
/// characters (a joiner or a right-to-left mark can be content) and loses
/// only its control characters, so no escape sequence reaches the terminal.
#[test]
fn show_prints_no_escape_sequence() {
    let mut note = read_answer();
    note["title"] = serde_json::json!(EVIL);
    note["frontmatter"]["aliases"] = serde_json::json!([EVIL]);
    note["frontmatter"]["tags"] = serde_json::json!([EVIL]);
    note["body"] = serde_json::json!(format!(
        "# Build\n\nRun \u{1b}[2Jthe checks\u{9b}31m.\n\u{202e}right-to-left\u{202c}\n"
    ));
    let text = note_text(&note, false);
    assert!(!text.contains("\u{1b}[2J"), "{text:?}");
    assert!(!text.contains('\u{9b}'), "a C1 control: {text:?}");
    let (head, body) = text.split_once("\n\n").unwrap();
    assert_clean(head);
    assert!(head.contains(SHOWN), "{head}");
    assert!(
        body.ends_with("# Build\n\nRun [2Jthe checks31m.\n\u{202e}right-to-left\u{202c}\n"),
        "{body:?}"
    );
}

// ── Hidden characters where the owner decides ──────────────────────────

/// A suggestion is what `memory confirm <id>` makes the owner's, so its
/// line shows every hidden character in the two titles and the reason as
/// `<U+XXXX>` — dropped, the line would name a title the note does not have
/// — each on one line, so a line break is shown too.
#[test]
fn a_suggestion_line_shows_hidden_characters_as_code_points() {
    let item = serde_json::json!({
        "id": SUGGESTION,
        "title": "Com\u{200b}mute\u{fe0f}",
        "reason": "Said\u{fffc} so\n.",
        "origin": "extracted_user",
        "created_at": "2026-09-25T10:00:00Z",
        "observed_at": "2026-09-24T18:02:11Z",
        "older_count": 0,
        "target": { "id": TARGET, "title": "My\u{202e} commute", "origin": "user" },
    });
    assert_eq!(
        suggestion_line(&item),
        "b1c2d3e4  replace “My<U+202E> commute” (saved by you) → “Com<U+200B>mute<U+FE0F>”  from your words, 2026-09-24 — Said<U+FFFC> so<U+000A>."
    );
}

/// Under each suggestion that holds any, one line counts its hidden
/// characters — the titles and the reason the line shows, and both notes'
/// texts it does not — and one line under the list says where those are
/// shown. A clean suggestion gets no line. On a terminal the list starts
/// with the reset every approval prompt starts with.
#[test]
fn the_suggestions_listing_counts_each_suggestions_hidden_characters() {
    colored::control::set_override(false);
    let odd = serde_json::json!({
        "id": SUGGESTION,
        "title": "Com\u{200b}mute",
        "knowledge": "Cycles\u{2800} now.",
        "reason": "Said so.",
        "origin": "extracted_user",
        "created_at": "2026-09-25T10:00:00Z",
        "observed_at": "2026-09-25T09:00:00Z",
        "older_count": 0,
        "target": {
            "id": TARGET, "title": "My\u{202e} commute", "origin": "user",
            "excerpt": "The\u{a0}bus.",
        },
    });
    let clean = serde_json::json!({
        "id": "c1c2d3e4-0000-4000-8000-000000000003",
        "title": "Rice",
        "knowledge": "Rice.",
        "reason": null,
        "origin": "extracted_user",
        "created_at": "2026-09-25T10:00:00Z",
        "observed_at": "2026-09-25T09:00:00Z",
        "older_count": 0,
        "target": { "id": TARGET, "title": "Lunch", "origin": "user", "excerpt": "Noodles." },
    });
    let page = serde_json::json!({ "items": [odd, clean] });
    let text = suggestions_text(&page, OutputFormat::Table, false);
    assert_eq!(
        text,
        "b1c2d3e4  replace “My<U+202E> commute” (saved by you) → “Com<U+200B>mute”  from your words, 2026-09-25 — Said so.\n\
         \x20 Note: this request contains 4 hidden characters, shown as <U+…>.\n\
         c1c2d3e4  replace “Lunch” (saved by you) → “Rice”  from your words, 2026-09-25\n\
         Each count includes both notes' texts, which this list does not print: `openalpaca memory show <id>` shows a suggestion's own with every hidden character as <U+XXXX>, and --format json prints both, each as a JSON \\u escape.\n\
         Replace: openalpaca memory confirm <id>   Keep yours: openalpaca memory undo <id>\n"
    );
    assert_eq!(
        suggestions_text(&page, OutputFormat::Table, true),
        format!("{}{text}", crate::output::TERMINAL_RESET)
    );
    let quiet = serde_json::json!({ "items": [page["items"][1].clone()] });
    assert!(
        !suggestions_text(&quiet, OutputFormat::Table, false).contains("hidden"),
        "a clean list says nothing about hidden characters"
    );
}

/// `--format json` is a machine format: every hidden character is written
/// as JSON's own `\u` escape, the document reads back as exactly the
/// daemon's values, and there is neither a notice nor a reset.
#[test]
fn the_suggestions_json_writes_hidden_characters_as_json_escapes() {
    let page = serde_json::json!({ "items": [{
        "id": SUGGESTION,
        "title": "Com\u{200b}mute",
        "knowledge": "x\u{202e}y",
        "target": { "title": "a\u{9b}b", "excerpt": "c\u{e0041}" },
    }] });
    let text = suggestions_text(&page, OutputFormat::Json, true);
    assert!(
        !text.contains(['\u{200b}', '\u{202e}', '\u{9b}', '\u{e0041}', '\u{1b}']),
        "{text:?}"
    );
    assert!(text.contains("Com\\u200bmute"), "{text}");
    assert!(!text.contains("<U+") && !text.contains("Note:"), "{text}");
    let back: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(back, page);
}

/// `memory show` of a candidate is what a person reads before `memory
/// confirm <id>`: its title, aliases, tags, fact key, the notes it would
/// replace and its text — the model's — show every hidden character as
/// `<U+XXXX>` (a control character too, so nothing drives the terminal),
/// and one line directly under the text counts them all. On a terminal it
/// starts with the reset.
#[test]
fn show_escapes_a_candidates_hidden_characters_and_counts_them() {
    colored::control::set_override(false);
    let mut note = suggestion_read(&[TARGET]);
    note["effective_status"] = serde_json::json!("candidate");
    note["title"] = serde_json::json!("Com\u{202e}mute");
    note["frontmatter"]["aliases"] = serde_json::json!(["bi\u{200b}ke"]);
    note["frontmatter"]["tags"] = serde_json::json!(["da\u{202e}ily", "tra\u{200b}vel"]);
    note["frontmatter"]["fact_key"] = serde_json::json!("commute.\u{202e}mode");
    note["frontmatter"]["supersedes"] = serde_json::json!([format!("{TARGET}\u{200b}")]);
    note["body"] = serde_json::json!(
        "# Commute\n\n## Knowledge\nThe user\u{2800} cycles \u{1b}[2J\tdaily.\r\n"
    );
    let text = note_text(&note, false);
    for raw in ['\u{202e}', '\u{200b}', '\u{2800}', '\u{1b}', '\r'] {
        assert!(!text.contains(raw), "{raw:?} in {text:?}");
    }
    let (head, body) = text.split_once("\n\n").unwrap();
    assert!(head.starts_with("Com<U+202E>mute\n"), "{head}");
    assert!(head.contains("aliases bi<U+200B>ke"), "{head}");
    assert!(
        head.contains("tags da<U+202E>ily, tra<U+200B>vel"),
        "{head}"
    );
    assert!(head.contains("fact_key commute.<U+202E>mode"), "{head}");
    assert!(
        head.contains(&format!("supersedes {TARGET}<U+200B>")),
        "{head}"
    );
    assert_eq!(
        body,
        "# Commute\n\n## Knowledge\nThe user<U+2800> cycles <U+001B>[2J\tdaily.<U+000D>\n\
         Note: this request contains 9 hidden characters, shown as <U+…>.\n"
    );
    assert_eq!(
        note_text(&note, true),
        format!("{}{text}", crate::output::TERMINAL_RESET)
    );
}

/// An ambiguous id prefix lists the notes it matches, one title per line:
/// with no escape or format character in them.
#[tokio::test]
async fn an_ambiguous_prefix_lists_titles_with_no_escape_or_format_character() {
    let page = serde_json::json!({
        "items": [
            { "id": "abcd0001-0000-4000-8000-000000000001", "title": EVIL },
            { "id": "abcd0002-0000-4000-8000-000000000002", "title": EVIL },
        ],
    })
    .to_string();
    let empty = serde_json::json!({ "items": [] }).to_string();
    let (addr, server) = serve(vec![
        json_answer("200 OK", OLD_REV, &page),
        json_answer("200 OK", OLD_REV, &empty),
        json_answer("200 OK", OLD_REV, &empty),
    ]);
    let api = Api::for_tests(DaemonClient::for_tests(&format!("http://{addr}"), "token"));
    let err = api.resolve_id("abcd").await.expect_err("two notes match");
    let message = format!("{err:#}");
    assert_clean(&message);
    assert!(message.contains(&format!("abcd0001-0000-4000-8000-000000000001  {SHOWN}")), "{message}");
    server.join().expect("the server finishes");
}

/// The L8 close-out's A3 from the CLI: `memory forget` sends every id in one
/// `POST /v1/memories/forget`, in the order given, so a suggestion named
/// with the note it would replace is forgotten as asked — one request per
/// id would forget the note first, whose forget withdraws the suggestion,
/// and the second request would answer it `not found`. Each target's
/// outcome is printed, and what a forget keeps once.
#[tokio::test]
async fn forget_sends_every_id_in_one_request_and_prints_each_outcome() {
    let answer = serde_json::json!({
        "results": [
            { "id": TARGET, "status": "forgotten" },
            { "id": SUGGESTION, "status": "revision_conflict", "current_revision": NEW_REV },
        ],
        "retained": { "transcript": true, "session_logs": true, "note": "Session logs are separate." },
    });
    let (addr, server) = serve(vec![json_answer("200 OK", OLD_REV, &answer.to_string())]);
    let api = Api::for_tests(DaemonClient::for_tests(&format!("http://{addr}"), "token"));
    let targets = vec![
        (NoteId::parse(TARGET).unwrap(), TARGET_REV.to_string()),
        (NoteId::parse(SUGGESTION).unwrap(), OLD_REV.to_string()),
    ];
    let mut lines = Vec::new();
    forget_notes(&api, &targets, &mut |l| lines.push(l))
        .await
        .expect("the forget runs");
    let requests = server.join().expect("the server finishes");
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(
        requests[0].starts_with("POST /v1/memories/forget "),
        "{}",
        requests[0]
    );
    let body = post_body(&requests[0]);
    assert_eq!(
        body["targets"],
        serde_json::json!([
            { "id": TARGET, "expected_revision": TARGET_REV },
            { "id": SUGGESTION, "expected_revision": OLD_REV },
        ])
    );
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(
        lines[0].contains(TARGET) && lines[0].contains("forgotten"),
        "{lines:?}"
    );
    assert!(
        lines[1].contains(SUGGESTION) && lines[1].contains("changed since"),
        "{lines:?}"
    );
    assert!(lines[2].contains("Session logs are separate."), "{lines:?}");
}

/// The `n`th of the ids the batch tests forget, and a `forget` answer
/// saying each id of `ids` was forgotten.
fn nth_id(n: usize) -> String {
    format!("c0ffee{n:02}-0000-4000-8000-0000000000{n:02}")
}

fn forgotten_answer(ids: &[String]) -> String {
    serde_json::json!({
        "results": ids
            .iter()
            .map(|id| serde_json::json!({ "id": id, "status": "forgotten" }))
            .collect::<Vec<_>>(),
        "retained": { "transcript": true, "session_logs": true, "note": "Session logs are separate." },
    })
    .to_string()
}

fn refused_answer() -> String {
    json_answer(
        "503 Service Unavailable",
        OLD_REV,
        &serde_json::json!({
            "error": { "code": "MEMORY_QUARANTINED", "message": "the project's partition is quarantined" }
        })
        .to_string(),
    )
}

/// Close-out repair round 1 (RT1 2, RG1 5): past 20 ids `memory forget`
/// sends more than one request, and a request's withdrawals run before the
/// next is sent — so a note in the first and its pending suggestion in the
/// second would be the order-dependent case A3 removed (the suggestion
/// withdrawn, then answered `not found`, its row never suppressed). The
/// pending suggestions go first; everything else keeps the order given.
#[test]
fn forget_sends_pending_suggestions_first_so_no_request_withdraws_a_later_one() {
    assert!(is_pending_suggestion(&suggestion_read(&[TARGET])));
    assert!(!is_pending_suggestion(&suggestion_read(&[])), "a candidate naming nothing");
    assert!(!is_pending_suggestion(&target_read()));
    let mut read: Vec<(NoteId, String, bool)> = (0..20)
        .map(|n| (NoteId::parse(&nth_id(n)).unwrap(), OLD_REV.to_string(), false))
        .collect();
    read.insert(3, (NoteId::parse(TARGET).unwrap(), TARGET_REV.to_string(), false));
    read.push((NoteId::parse(SUGGESTION).unwrap(), OLD_REV.to_string(), true));
    let ordered = forget_order(read.clone());
    assert_eq!(ordered.len(), 22);
    assert_eq!(ordered[0], (NoteId::parse(SUGGESTION).unwrap(), OLD_REV.to_string()));
    let rest: Vec<(NoteId, String)> = read[..21]
        .iter()
        .map(|(id, revision, _)| (*id, revision.clone()))
        .collect();
    assert_eq!(ordered[1..], rest[..], "the others keep the order given");
}

/// Close-out repair round 1 (RT1 2, RG1 5, RC1 3): a refused request no
/// longer hides what landed. The lines of the requests before it were
/// printed as each returned; the refused one's ids are read back — a forget
/// that landed before the target that failed is gone — and the ids after it
/// were never sent. The refusal is still the command's error.
#[tokio::test]
async fn a_refused_forget_request_still_reports_what_landed() {
    let ids: Vec<String> = (0..21).map(nth_id).collect();
    // A second request refused: the first request's twenty lines stand.
    let (addr, server) = serve(vec![
        json_answer("200 OK", OLD_REV, &forgotten_answer(&ids[..20])),
        refused_answer(),
        json_answer("200 OK", OLD_REV, &read_answer().to_string()),
    ]);
    let api = Api::for_tests(DaemonClient::for_tests(&format!("http://{addr}"), "token"));
    let targets: Vec<(NoteId, String)> = ids
        .iter()
        .map(|id| (NoteId::parse(id).unwrap(), OLD_REV.to_string()))
        .collect();
    let mut lines = Vec::new();
    let err = forget_notes(&api, &targets, &mut |l| lines.push(l))
        .await
        .expect_err("the second request is refused");
    assert!(format!("{err:#}").contains("MEMORY_QUARANTINED"), "{err:#}");
    assert_eq!(lines.len(), 22, "{lines:?}");
    for (line, id) in lines.iter().zip(&ids[..20]) {
        assert!(line.contains(id.as_str()) && line.contains("forgotten"), "{lines:?}");
    }
    assert!(lines[20].contains("Session logs are separate."), "{lines:?}");
    assert!(
        lines[21].contains(ids[20].as_str()) && lines[21].contains("not forgotten"),
        "{lines:?}"
    );
    let requests = server.join().expect("the server finishes");
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert!(requests[2].starts_with(&format!("GET /v1/memories/{} ", ids[20])), "{}", requests[2]);

    // The first request refused after its first target landed: that one
    // reads back gone, the second is still there, and the one id past the
    // request was never sent.
    let gone = json_answer(
        "404 Not Found",
        OLD_REV,
        &serde_json::json!({ "error": { "code": "MEMORY_NOT_FOUND", "message": "no such note" } })
            .to_string(),
    );
    let mut answers = vec![refused_answer(), gone];
    answers.extend((1..20).map(|_| json_answer("200 OK", OLD_REV, &read_answer().to_string())));
    let (addr, server) = serve(answers);
    let api = Api::for_tests(DaemonClient::for_tests(&format!("http://{addr}"), "token"));
    let mut lines = Vec::new();
    forget_notes(&api, &targets, &mut |l| lines.push(l))
        .await
        .expect_err("the request is refused");
    assert_eq!(lines.len(), 21, "{lines:?}");
    assert!(
        lines[0].contains(ids[0].as_str())
            && lines[0].contains("forgotten")
            && !lines[0].contains("not forgotten"),
        "{lines:?}"
    );
    for (line, id) in lines[1..20].iter().zip(&ids[1..20]) {
        assert!(line.contains(id.as_str()) && line.contains("not forgotten"), "{lines:?}");
    }
    assert!(lines[20].contains("1 more id") && lines[20].contains("not sent"), "{lines:?}");
    let requests = server.join().expect("the server finishes");
    assert_eq!(requests.len(), 21, "{requests:?}");
}

/// Close-out repair round 2 (RT2 nit): the verb's own path, not only its
/// helpers — the reads it makes before the question, and the order those
/// reads give the requests. Twenty-one ids typed, the last read back as a
/// pending suggestion: the listing keeps the order typed, and the
/// suggestion is the first target of the first request, the others after
/// it in the order typed.
#[tokio::test]
async fn the_forget_verb_sends_the_pending_suggestion_it_read_first() {
    colored::control::set_override(false);
    let typed: Vec<String> = (0..20)
        .map(nth_id)
        .chain([SUGGESTION.to_string()])
        .collect();
    let mut answers: Vec<String> = typed
        .iter()
        .map(|id| {
            let mut read = if id == SUGGESTION {
                suggestion_read(&[TARGET])
            } else {
                read_answer()
            };
            read["id"] = serde_json::json!(id);
            json_answer("200 OK", OLD_REV, &read.to_string())
        })
        .collect();
    let sent: Vec<String> = std::iter::once(SUGGESTION.to_string())
        .chain(typed[..20].iter().cloned())
        .collect();
    answers.push(json_answer("200 OK", OLD_REV, &forgotten_answer(&sent[..20])));
    answers.push(json_answer("200 OK", OLD_REV, &forgotten_answer(&sent[20..])));
    let (addr, server) = serve(answers);
    let api = Api::for_tests(DaemonClient::for_tests(&format!("http://{addr}"), "token"));
    let mut lines = Vec::new();
    forget_verb(&api, &typed, true, &mut |l| lines.push(l))
        .await
        .expect("the forget runs");
    let requests = server.join().expect("the server finishes");
    assert_eq!(requests.len(), 23, "{requests:?}");
    for (request, id) in requests[..21].iter().zip(&typed) {
        assert!(request.starts_with(&format!("GET /v1/memories/{id} ")), "{request}");
    }
    let targets: Vec<String> = requests[21..]
        .iter()
        .flat_map(|r| {
            assert!(r.starts_with("POST /v1/memories/forget "), "{r}");
            post_body(r)["targets"].as_array().unwrap().clone()
        })
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(targets[0], SUGGESTION, "the suggestion goes first");
    assert_eq!(targets, sent, "the others keep the order typed");
    for (line, id) in lines[..21].iter().zip(&typed) {
        assert!(line.contains(id.as_str()), "the listing keeps the order typed: {lines:?}");
    }
    assert!(lines[21].contains(SUGGESTION) && lines[21].contains("forgotten"), "{lines:?}");
}

/// The L8 close-out's A4: a suggestion that changes the note's metadata —
/// here the assistant's, which gives no reason — says so on a line under
/// its own, each value through the one-line escape, and the count under it
/// includes those values, as the Memory view's card and the chat preview
/// count them.
#[test]
fn a_suggestions_metadata_changes_are_listed_under_it_and_counted() {
    colored::control::set_override(false);
    let item = serde_json::json!({
        "id": SUGGESTION,
        "title": "Editor",
        "knowledge": "Helix.",
        "reason": null,
        "changes": [
            { "field": "tags", "from": [], "to": ["tools\u{202e}"] },
            { "field": "aliases", "from": ["vi"], "to": ["vi", "hx"] },
            { "field": "valid_to", "from": [], "to": ["2026-12-31T00:00:00Z"] },
        ],
        "origin": "extracted_user",
        "created_at": "2026-09-25T10:00:00Z",
        "observed_at": null,
        "older_count": 0,
        "target": { "id": TARGET, "title": "Editor", "origin": "user", "excerpt": "Helix." },
    });
    let text = suggestions_text(&serde_json::json!({ "items": [item] }), OutputFormat::Table, false);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "b1c2d3e4  replace “Editor” (saved by you) → “Editor”  suggested by the assistant, 2026-09-25"
    );
    assert_eq!(
        lines[1],
        "  changes: tags none → tools<U+202E>; aliases vi → vi, hx; valid until none → 2026-12-31T00:00:00Z"
    );
    assert_eq!(
        lines[2],
        "  Note: this request contains 1 hidden character, shown as <U+…>."
    );
}

/// The L8 close-out's A4: `memory show` prints a note's validity bounds
/// beside its key, aliases and tags — a suggestion that bounds a note in
/// time differs from it there alone.
#[test]
fn show_prints_the_validity_bounds() {
    colored::control::set_override(false);
    let mut note = suggestion_read(&[TARGET]);
    note["effective_status"] = serde_json::json!("candidate");
    note["frontmatter"]["valid_from"] = serde_json::json!("2026-01-01T00:00:00Z");
    note["frontmatter"]["valid_to"] = serde_json::json!("2026-12-31T00:00:00Z");
    let text = note_text(&note, false);
    let (head, _) = text.split_once("\n\n").unwrap();
    assert!(
        head.contains("valid from 2026-01-01T00:00:00Z  valid until 2026-12-31T00:00:00Z"),
        "{head}"
    );
    let mut open = read_answer();
    open["frontmatter"]["valid_to"] = serde_json::json!("2027-06-30T00:00:00Z");
    let text = note_text(&open, false);
    assert!(text.contains("valid until 2027-06-30T00:00:00Z"), "{text}");
    assert!(!text.contains("valid from"), "{text}");
}
