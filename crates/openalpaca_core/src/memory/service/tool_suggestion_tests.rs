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

    // Every other surface keeps ruling R4: a target in the other partition
    // is `invalid_input`, and the caller names the scope.
    let err = h
        .service
        .store(
            &h.user(ALICE, Some(&ws)),
            StoreInput {
                title: "Laptop".into(),
                body: "# Laptop\n\nA Framework 13.\n".into(),
                kind: Some(NoteKind::Fact),
                supersedes: Some((laptop.id, laptop.revision.clone())),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.class(), "invalid_input");
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

/// A note learning wrote from what the user said, under `key`, in the
/// partition `ctx` names by `scope`.
fn learned(
    h: &Harness,
    workspace: Option<&Path>,
    scope: Option<NoteScope>,
    key: &str,
    title: &str,
    knowledge: &str,
) -> StoreOutput {
    let row = h.message_row(knowledge);
    h.service
        .store(
            &h.ctx(
                ALICE,
                workspace,
                Surface::Extraction,
                Capabilities::EXTRACTION,
            )
            .unwrap(),
            StoreInput {
                title: title.into(),
                body: format!("# {title}\n\n{knowledge}\n"),
                kind: Some(NoteKind::Preference),
                fact_key: Some(key.into()),
                scope,
                sources: vec![NoteSource {
                    kind: SourceType::Message,
                    id: row.to_string(),
                    observed_at: None,
                }],
                observed_at: Some(chrono::Utc::now()),
                provenance: Some(ExtractionProvenance::UserWords),
                ..Default::default()
            },
        )
        .unwrap()
}

/// Repair round 1 of the L8 product slice: an exact duplicate of a note the
/// user saved — the same kind, title, text, key, aliases and tags — is that
/// note, as `memory_store`'s description says, whether the chat model names
/// it in `supersedes` or only shares its key. It is no "replace X with X"
/// suggestion.
#[test]
fn an_exact_duplicate_of_a_saved_note_returns_it() {
    let h = Harness::new();
    let laptop = saved(&h, "user.laptop", "Laptop", "A ThinkPad X1 Carbon.");
    let (_, tool) = chat_turn(&h, "My laptop is a ThinkPad X1 Carbon.", None);
    let same = |supersedes: Option<(NoteId, String)>| StoreInput {
        title: "Laptop".into(),
        body: "# Laptop\n\nA ThinkPad X1 Carbon.\n".into(),
        kind: Some(NoteKind::Preference),
        fact_key: Some("user.laptop".into()),
        supersedes,
        ..Default::default()
    };
    for supersedes in [None, Some((laptop.id, laptop.revision.clone()))] {
        let out = h.service.store(&tool, same(supersedes.clone())).unwrap();
        assert_eq!(out.dedup, Dedup::Existing, "{supersedes:?}");
        assert!(!out.suggestion, "{supersedes:?}");
        assert_eq!(out.id, laptop.id, "{supersedes:?}");
        assert!(out.superseded.is_empty(), "{supersedes:?}");
    }
    assert!(
        h.service
            .suggestions(&h.user(ALICE, None), None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(read_note(&h, laptop.id).revision, laptop.revision);
}

/// Repair round 1 of the L8 product slice: the note a store meets under its
/// key is the R9 winner of the partition it would land in — per partition,
/// as R9 is — so a learned note holding the key in the project does not
/// hide the note the user saved under it at home. The chat model's
/// preference (a global kind) under that key is a suggestion replacing the
/// saved note, not a second active note the saved one silently outranks.
#[test]
fn a_learned_note_in_the_project_does_not_hide_a_saved_note_at_home() {
    let h = Harness::new();
    let ws = h.workspace("lantern", Some(".git"));
    learned(
        &h,
        Some(&ws),
        Some(NoteScope::Project),
        "user.editor",
        "Editor here",
        "In this project the user edits with vim.",
    );
    let editor = saved(&h, "user.editor", "Editor", "The user edits with vim.");
    let (_, tool) = chat_turn(&h, "I use Helix everywhere now.", Some(&ws));
    let out = h
        .service
        .store(
            &tool,
            StoreInput {
                title: "Editor".into(),
                body: "# Editor\n\nThe user edits with Helix.\n".into(),
                kind: Some(NoteKind::Preference),
                fact_key: Some("user.editor".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(out.suggestion, "{out:?}");
    assert_eq!(out.superseded, vec![editor.id]);
    assert_eq!(out.scope, NoteScope::Global);
    assert_eq!(active_under(&h, "user.editor"), vec![editor.id]);
}

/// Repair round 1 of the L8 product slice: a replacement that names a
/// learned note — or the chat model's own earlier suggestion — under the
/// key of a note the user saved is a replacement of the saved note, so a
/// suggestion naming it: replacing the named note alone would leave the new
/// one under the saved note's key, silently outranked by it (or, for a note
/// edited by hand or imported, outranking it).
#[test]
fn a_replacement_under_a_saved_notes_key_is_a_suggestion_for_the_saved_note() {
    let h = Harness::new();
    let editor = saved(&h, "user.editor", "Editor", "The user edits with vim.");
    let loser = learned(
        &h,
        None,
        None,
        "user.editor",
        "Editor again",
        "The user edits with vim, still.",
    );
    let (_, tool) = chat_turn(&h, "I use Helix now.", None);
    let store = |title: &str, knowledge: &str, target: &StoreOutput| {
        h.service
            .store(
                &tool,
                StoreInput {
                    title: title.into(),
                    body: format!("# {title}\n\n{knowledge}\n"),
                    kind: Some(NoteKind::Preference),
                    fact_key: Some("user.editor".into()),
                    supersedes: Some((target.id, target.revision.clone())),
                    ..Default::default()
                },
            )
            .unwrap()
    };
    let first = store("Editor", "The user edits with Helix.", &loser);
    assert!(first.suggestion, "{first:?}");
    assert_eq!(first.superseded, vec![editor.id]);
    // The model names its own suggestion next: the same.
    let second = store("Editor", "The user edits with Helix 25.", &first);
    assert!(second.suggestion, "{second:?}");
    assert_eq!(second.superseded, vec![editor.id]);
    assert_eq!(
        active_under(&h, "user.editor")
            .into_iter()
            .collect::<HashSet<_>>(),
        HashSet::from([editor.id, loser.id])
    );
    assert_eq!(read_note(&h, editor.id).revision, editor.revision);
}

/// Repair round 1 of the L8 product slice (design 09 §4.2 rule 5): the
/// in-chat card and the `<memory_suggestions>` line say the chat model's
/// suggestion is the assistant's — its text is the model's, not the user's
/// words as learning wrote them — as the Memory view and the CLI do. The
/// date is still the turn's typed row's. Learning's suggestion keeps "from
/// what you said".
#[test]
fn the_card_says_the_assistant_suggested_the_chat_models_suggestion() {
    let h = Harness::new();
    let laptop = saved(&h, "user.laptop", "Laptop", "A ThinkPad X1 Carbon.");
    let (_, tool) = chat_turn(&h, "I switched to a MacBook Air M3.", None);
    let out = h
        .service
        .store(
            &tool,
            StoreInput {
                title: "Laptop".into(),
                body: "# Laptop\n\nA MacBook Air M3.\n".into(),
                kind: Some(NoteKind::Preference),
                fact_key: Some("user.laptop".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(out.suggestion);
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let pending = h
        .service
        .pending_suggestions(&tool, 3, &HashSet::new())
        .unwrap();
    let [line] = &pending[..] else {
        panic!("one pending suggestion: {pending:?}");
    };
    let rendered = line.render();
    assert!(
        rendered.ends_with(&format!("(suggested by the assistant on {today})\n")),
        "{rendered}"
    );
    assert!(!rendered.contains("what you said"), "{rendered}");
    let preview = h
        .service
        .suggestion_preview(&tool, out.id, &out.revision, &laptop.revision)
        .unwrap();
    assert!(
        preview
            .text
            .contains(&format!("Suggested by the assistant on {today}.")),
        "{}",
        preview.text
    );
    assert!(!preview.text.contains("what you said"), "{}", preview.text);
}
