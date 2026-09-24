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
