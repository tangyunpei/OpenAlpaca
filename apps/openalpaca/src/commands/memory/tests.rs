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
