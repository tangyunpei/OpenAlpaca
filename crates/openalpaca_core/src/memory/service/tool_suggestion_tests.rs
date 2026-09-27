//! Owner decision LQ11 (2026-09-26; the L8 product slice's P1): a chat
//! model's replacement of a note the user saved, edited by hand or imported
//! is a **suggestion** — a candidate naming that note, written through the
//! path learning's suggestions take (design 09 §3.1–§3.2, S1–S4) — where it
//! used to be `authority_conflict` (09 §3.1 row 5), after which the model
//! stored the new value as a second, contradicting active note. Nothing
//! active changes until the user confirms: in the Memory view, with
//! `openalpaca memory confirm`, or on the in-chat card of a later turn,
//! which is why the suggestion cites the turn's own row. The lead's
//! reading: a store under such a note's `fact_key` and an edit of such a
//! note are the same replacement. A tool call still never mints `user`,
//! never retires a note the user saved and never confirms (G2).

use super::tests::{ALICE, Harness};
use super::*;
use openalpaca_storage::memory_files::{EffectiveStatus, NoteKind, NoteScope, Origin, SourceType};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

const LANE: &str = "u_example_alice:gui";

/// A chat turn of the app: its user row, and the model-tool context its
/// main loop's memory tools run with — the turn's request id, lane and
/// derived source, as `context_for_tool` builds it.
fn chat_turn(h: &Harness, text: &str, workspace: Option<&Path>) -> (i64, MemoryAccessContext) {
    let request = uuid::Uuid::new_v4();
    let row =
        h.db.with_connection(|c| {
            c.execute(
                "INSERT INTO conversation_messages (lane_key, role, content, source, request_id)
                 VALUES (?1, 'user', ?2, 'gui', ?3)",
                rusqlite::params![LANE, text, request.to_string()],
            )?;
            Ok(c.last_insert_rowid())
        })
        .unwrap();
    let hint = match workspace {
        Some(p) => WorkspaceHint::Path(p),
        None => WorkspaceHint::None,
    };
    let ctx = h
        .service
        .context(
            ALICE,
            hint,
            Surface::ModelTool,
            Capabilities::READ_WRITE,
            Attribution {
                lane_key: Some(LANE.to_string()),
                task_id: None,
                request_id: Some(request),
                source: Some("gui".to_string()),
            },
        )
        .unwrap();
    (row, ctx)
}

/// A note the user saved (origin `user`), global, under `key`.
fn saved(h: &Harness, key: &str, title: &str, knowledge: &str) -> StoreOutput {
    h.service
        .store(
            &h.user(ALICE, None),
            StoreInput {
                title: title.into(),
                body: format!("# {title}\n\n{knowledge}\n"),
                kind: Some(NoteKind::Preference),
                fact_key: Some(key.into()),
                ..Default::default()
            },
        )
        .unwrap()
}

fn read_note(h: &Harness, id: NoteId) -> ReadOutput {
    h.service
        .read(
            &h.user(ALICE, None),
            ReadInput {
                id,
                ..Default::default()
            },
        )
        .unwrap()
}

/// The active notes that carry `key`, as the user lists and reads them.
fn active_under(h: &Harness, key: &str) -> Vec<NoteId> {
    h.service
        .list(&h.user(ALICE, None), ListInput::default())
        .unwrap()
        .items
        .into_iter()
        .filter(|i| read_note(h, i.id).frontmatter["fact_key"] == key)
        .map(|i| i.id)
        .collect()
}

/// The L-S shape: the user saved their laptop, then said in chat that it
/// changed, and the model stored the new value with `supersedes` naming the
/// saved note. That is now one suggestion naming it: the saved note stays
/// the one in force, no second active note says otherwise, and the
/// suggestion is an ordinary one — listed for the Memory view, raised in
/// chat, and confirmed by the user's own act.
#[test]
fn a_chat_models_replacement_of_a_saved_note_is_a_suggestion() {
    let h = Harness::new();
    let laptop = saved(
        &h,
        "user.laptop",
        "Laptop",
        "The user's laptop is a ThinkPad X1 Carbon on Fedora.",
    );
    let (row, tool) = chat_turn(&h, "I switched to a MacBook Air M3 last week.", None);
    let out = h
        .service
        .store(
            &tool,
            StoreInput {
                title: "Laptop".into(),
                body: "# Laptop\n\nThe user's laptop is now a MacBook Air M3.\n".into(),
                kind: Some(NoteKind::Fact),
                fact_key: Some("user.laptop".into()),
                supersedes: Some((laptop.id, laptop.revision.clone())),
                ..Default::default()
            },
        )
        .expect("a suggestion, not authority_conflict");
    assert!(out.suggestion);
    assert_eq!(out.origin, Origin::ExtractedUser, "never `user`");
    assert_eq!(out.superseded, vec![laptop.id]);
    assert_eq!(read_note(&h, out.id).status, NoteStatus::Candidate);
    assert_eq!(read_note(&h, laptop.id).revision, laptop.revision);
    assert_eq!(active_under(&h, "user.laptop"), vec![laptop.id]);

    // An ordinary suggestion: listed, naming the saved note, citing the
    // turn's own row — so a later attended app turn may raise it.
    let listed = h.service.suggestions(&h.user(ALICE, None), None).unwrap();
    let [suggestion] = &listed[..] else {
        panic!("one suggestion: {listed:?}");
    };
    assert_eq!((suggestion.id, suggestion.target.id), (out.id, laptop.id));
    assert_eq!(suggestion.origin, Origin::ExtractedUser);
    assert_eq!(suggestion.observed_at, None, "the extractor's alone");
    assert!(
        suggestion
            .sources
            .iter()
            .any(|s| s.kind == SourceType::Message && s.id == row.to_string()),
        "{:?}",
        suggestion.sources
    );
    let pending = h
        .service
        .pending_suggestions(&tool, 3, &HashSet::new())
        .unwrap();
    assert_eq!(
        pending.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![out.id]
    );

    // Only the user's confirm replaces their note.
    let confirmed = h
        .service
        .update(
            &h.user(ALICE, None),
            UpdateInput {
                id: out.id,
                expected_revision: out.revision.clone(),
                action: UpdateAction::Confirm {
                    target_revision: Some(laptop.revision.clone()),
                },
                reason: None,
            },
        )
        .unwrap();
    assert_eq!(confirmed.superseded, vec![laptop.id]);
    assert_eq!(active_under(&h, "user.laptop"), vec![out.id]);
}

/// Ruling LR10 on the tool path: a replacement named from a project chat,
/// with no `scope`, goes to the partition of the note it replaces — a
/// global saved note gets a global suggestion, not a refusal.
#[test]
fn a_replacement_named_from_a_project_chat_goes_to_the_saved_notes_partition() {
    let h = Harness::new();
    let ws = h.workspace("lantern", Some(".git"));
    let laptop = saved(&h, "user.laptop", "Laptop", "A ThinkPad X1 Carbon.");
    let (_, tool) = chat_turn(&h, "My laptop is a MacBook Air M3 now.", Some(&ws));
    let out = h
        .service
        .store(
            &tool,
            StoreInput {
                title: "Laptop".into(),
                body: "# Laptop\n\nA MacBook Air M3.\n".into(),
                kind: Some(NoteKind::Fact),
                supersedes: Some((laptop.id, laptop.revision.clone())),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(out.suggestion);
    assert_eq!(out.scope, NoteScope::Global);
}

/// The lead's reading of LQ11: a store with no `supersedes` whose
/// `fact_key` names a note the user saved is the same replacement — a
/// suggestion naming it — where it used to land as a same-key loser the
/// user never saw. A key only learned notes hold stays an ordinary store.
#[test]
fn a_chat_models_note_under_a_saved_notes_key_is_a_suggestion() {
    let h = Harness::new();
    let ws = h.workspace("lantern", Some(".git"));
    let commute = saved(
        &h,
        "user.commute",
        "Commute",
        "The user drives a 2018 Honda Fit.",
    );
    let (_, tool) = chat_turn(&h, "I sold the Fit; I take the metro now.", Some(&ws));
    let out = h
        .service
        .store(
            &tool,
            StoreInput {
                title: "Commute".into(),
                body: "# Commute\n\nThe user takes the metro, 40 minutes.\n".into(),
                kind: Some(NoteKind::Fact),
                fact_key: Some("user.commute".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(out.suggestion);
    assert_eq!(out.superseded, vec![commute.id]);
    assert_eq!(out.scope, NoteScope::Global, "the saved note's partition");
    assert_eq!(active_under(&h, "user.commute"), vec![commute.id]);

    // A key no saved note holds: an ordinary active note, as before.
    let plain = h
        .service
        .store(
            &tool,
            StoreInput {
                title: "Coffee".into(),
                body: "# Coffee\n\nThe user drinks flat whites.\n".into(),
                kind: Some(NoteKind::Preference),
                fact_key: Some("user.coffee".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!plain.suggestion);
    assert!(plain.superseded.is_empty());
    assert_eq!(read_note(&h, plain.id).status, NoteStatus::Active);
}

/// The lead's reading of LQ11: an edit of a note the user saved is a
/// suggestion carrying the edited content — the note itself is untouched —
/// where it used to be `authority_conflict`. An edit that changes nothing
/// is no replacement and is refused as before, and so is archive.
#[test]
fn a_chat_models_edit_of_a_saved_note_is_a_suggestion_carrying_the_edit() {
    let h = Harness::new();
    let editor = saved(&h, "user.editor", "Editor", "The user edits with vim.");
    let (_, tool) = chat_turn(&h, "I use Helix now.", None);
    let edit = |body: &str| UpdateInput {
        id: editor.id,
        expected_revision: editor.revision.clone(),
        action: UpdateAction::Edit(Patch {
            body: Some(body.into()),
            ..Default::default()
        }),
        reason: None,
    };
    let out = h
        .service
        .update(&tool, edit("# Editor\n\nThe user edits with Helix.\n"))
        .expect("a suggestion, not authority_conflict");
    assert_eq!(out.suggested_for, Some(editor.id));
    assert_ne!(out.id, editor.id, "a new note, the suggestion");
    assert_eq!(out.status, NoteStatus::Candidate);
    let suggestion = read_note(&h, out.id);
    assert_eq!(suggestion.body, "# Editor\n\nThe user edits with Helix.\n");
    assert_eq!(suggestion.frontmatter["title"], "Editor");
    assert_eq!(suggestion.frontmatter["fact_key"], "user.editor");
    assert_eq!(
        suggestion.frontmatter["supersedes"],
        serde_json::json!([editor.id.to_string()])
    );
    assert_eq!(read_note(&h, editor.id).revision, editor.revision);
    assert_eq!(active_under(&h, "user.editor"), vec![editor.id]);

    // An edit that changes nothing replaces nothing: refused, as before.
    let err = h
        .service
        .update(&tool, edit("# Editor\n\nThe user edits with vim.\n"))
        .unwrap_err();
    assert_eq!(err.class(), "authority_conflict");
    assert_eq!(
        h.service
            .suggestions(&h.user(ALICE, None), None)
            .unwrap()
            .len(),
        1
    );

    // A stale revision is a conflict, as for any edit.
    let stale = UpdateInput {
        expected_revision: "f".repeat(64),
        ..edit("# Editor\n\nThe user edits with Kakoune.\n")
    };
    let err = h.service.update(&tool, stale).unwrap_err();
    assert_eq!(err.class(), "revision_conflict");

    let err = h
        .service
        .update(
            &tool,
            UpdateInput {
                id: editor.id,
                expected_revision: editor.revision.clone(),
                action: UpdateAction::Archive,
                reason: None,
            },
        )
        .unwrap_err();
    assert_eq!(err.class(), "authority_conflict");
    assert_eq!(read_note(&h, editor.id).status, NoteStatus::Active);
}

/// Ruling LR9 on the tool path: a note edited by hand (`external`) is the
/// user's too, so the chat model's replacement of it is a suggestion; its
/// replacement of a learned note stays automatic.
#[test]
fn a_hand_edited_note_gets_a_suggestion_and_a_learned_one_is_replaced() {
    let h = Harness::new();
    let style = saved(&h, "user.style", "Style", "Sentence case.");
    let file = h
        .home_notes_dir(super::tests::ALICE_KEY)
        .read_dir()
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.to_string_lossy().contains(&style.id.to_string()))
        .unwrap();
    let text = fs::read_to_string(&file).unwrap();
    fs::write(
        &file,
        text.replace("Sentence case.", "Sentence case, always."),
    )
    .unwrap();
    h.service.reconcile_owner(ALICE).unwrap();
    let edited = read_note(&h, style.id);
    assert_eq!(edited.origin, Origin::External);

    let (_, tool) = chat_turn(&h, "Use title case from now on.", None);
    let out = h
        .service
        .store(
            &tool,
            StoreInput {
                title: "Style".into(),
                body: "# Style\n\nTitle case.\n".into(),
                kind: Some(NoteKind::Preference),
                supersedes: Some((style.id, edited.revision.clone())),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(out.suggestion);
    assert_eq!(read_note(&h, style.id).revision, edited.revision);

    let learned = h
        .service
        .store(
            &tool,
            StoreInput {
                title: "Tea".into(),
                body: "# Tea\n\nGreen tea.\n".into(),
                kind: Some(NoteKind::Preference),
                ..Default::default()
            },
        )
        .unwrap();
    let replaced = h
        .service
        .store(
            &tool,
            StoreInput {
                title: "Tea".into(),
                body: "# Tea\n\nOolong.\n".into(),
                kind: Some(NoteKind::Preference),
                supersedes: Some((learned.id, learned.revision.clone())),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!replaced.suggestion);
    assert_eq!(read_note(&h, replaced.id).status, NoteStatus::Active);
    let learned_now = h
        .service
        .list(
            &h.user(ALICE, None),
            ListInput {
                include_history: true,
                ..Default::default()
            },
        )
        .unwrap()
        .items
        .into_iter()
        .find(|i| i.id == learned.id)
        .unwrap();
    assert_eq!(learned_now.effective_status, EffectiveStatus::Superseded);
}
