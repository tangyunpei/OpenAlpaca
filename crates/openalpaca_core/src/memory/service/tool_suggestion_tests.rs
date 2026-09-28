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
/// date is the day it was made (repair round 2). Learning's suggestion
/// keeps "from what you said".
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

/// A cited message, as a note's source.
fn message(row: i64) -> NoteSource {
    NoteSource {
        kind: SourceType::Message,
        id: row.to_string(),
        observed_at: None,
    }
}

/// Repair round 2 of the L8 product slice: the chat model's edit of a
/// saved note is a copy of that note's text, so it cites what the note
/// cites — the note's sources, then the turn's own typed row (the L8
/// close-out's A5 put the note's first) — and a later forget of a sibling
/// citing the same row takes the confirmed copy
/// out as it would have taken the note (design §6, the sibling cost C2
/// accepts). Before, the copy cited the turn's row alone and kept the
/// forgotten row's text in automatic recall.
#[test]
fn a_chat_models_edit_suggestion_cites_the_saved_notes_sources() {
    let h = Harness::new();
    let said = h.message_row("I live at 1 Elm Street, the blue house.");
    let user = h.user(ALICE, None);
    let home = h
        .service
        .store(
            &user,
            StoreInput {
                title: "Home address".into(),
                body: "# Home address\n\nThe user lives at 1 Elm Street.\n".into(),
                kind: Some(NoteKind::Fact),
                fact_key: Some("user.address".into()),
                sources: vec![message(said)],
                ..Default::default()
            },
        )
        .unwrap();
    let sibling = h
        .service
        .store(
            &h.ctx(ALICE, None, Surface::Extraction, Capabilities::EXTRACTION)
                .unwrap(),
            StoreInput {
                title: "House colour".into(),
                body: "# House colour\n\nThe user's house is blue.\n".into(),
                kind: Some(NoteKind::Fact),
                sources: vec![message(said)],
                observed_at: Some(chrono::Utc::now()),
                provenance: Some(ExtractionProvenance::UserWords),
                ..Default::default()
            },
        )
        .unwrap();
    let (turn, tool) = chat_turn(&h, "Note that it is the blue house.", None);
    let out = h
        .service
        .update(
            &tool,
            UpdateInput {
                id: home.id,
                expected_revision: home.revision.clone(),
                action: UpdateAction::Edit(Patch {
                    body: Some(
                        "# Home address\n\nThe user lives at 1 Elm Street, the blue house.\n"
                            .into(),
                    ),
                    ..Default::default()
                }),
                reason: None,
            },
        )
        .unwrap();
    assert_eq!(out.suggested_for, Some(home.id));
    let cited: Vec<String> = read_note(&h, out.id)
        .sources
        .iter()
        .map(NoteSource::suppression_key)
        .collect();
    assert_eq!(
        cited,
        vec![format!("message:{said}"), format!("message:{turn}")],
        "what the saved note cites, then the turn's typed row"
    );

    let confirmed = h
        .service
        .update(
            &user,
            UpdateInput {
                id: out.id,
                expected_revision: out.revision.clone(),
                action: UpdateAction::Confirm {
                    target_revision: Some(home.revision.clone()),
                },
                reason: None,
            },
        )
        .unwrap();
    assert_eq!(confirmed.status, NoteStatus::Active);
    assert!(!read_note(&h, out.id).source_forgotten);
    h.service
        .forget(
            &user,
            ForgetInput {
                targets: vec![(sibling.id, sibling.revision)],
                reason: None,
            },
        )
        .unwrap();
    assert!(
        read_note(&h, out.id).source_forgotten,
        "the forgotten row's text goes out with the copy that carries it"
    );
}

/// Repair round 2 of the L8 product slice: forgetting a note withdraws the
/// pending suggestions that would replace it — they can never be confirmed
/// now — so the chat model's edit copy of a forgotten note's text is not
/// left behind as a candidate the model can list and read back. The
/// store path's suggestion for the same note goes too. A withdrawal
/// suppresses nothing: the turn's row stays unforgotten.
#[test]
fn forgetting_a_note_withdraws_the_suggestions_that_would_replace_it() {
    let h = Harness::new();
    let editor = saved(&h, "user.editor", "Editor", "The user edits with vim.");
    let (typed, tool) = chat_turn(&h, "I use the vim keymap in every editor.", None);
    let keymap = h
        .service
        .store(
            &h.ctx(ALICE, None, Surface::Extraction, Capabilities::EXTRACTION)
                .unwrap(),
            StoreInput {
                title: "Keymap".into(),
                body: "# Keymap\n\nThe user uses the vim keymap everywhere.\n".into(),
                kind: Some(NoteKind::Preference),
                sources: vec![message(typed)],
                observed_at: Some(chrono::Utc::now()),
                provenance: Some(ExtractionProvenance::UserWords),
                ..Default::default()
            },
        )
        .unwrap();
    let copy = h
        .service
        .update(
            &tool,
            UpdateInput {
                id: editor.id,
                expected_revision: editor.revision.clone(),
                action: UpdateAction::Edit(Patch {
                    body: Some(
                        "# Editor\n\nThe user edits with vim, and its keymap everywhere.\n".into(),
                    ),
                    ..Default::default()
                }),
                reason: None,
            },
        )
        .unwrap();
    let (_, later) = chat_turn(&h, "I moved to Helix.", None);
    let stored = h
        .service
        .store(
            &later,
            StoreInput {
                title: "Editor".into(),
                body: "# Editor\n\nThe user edits with Helix.\n".into(),
                kind: Some(NoteKind::Preference),
                supersedes: Some((editor.id, editor.revision.clone())),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(copy.suggested_for, Some(editor.id));
    assert!(stored.suggestion);
    let user = h.user(ALICE, None);
    let candidates = |ctx: &MemoryAccessContext| -> Vec<NoteId> {
        h.service
            .list(
                ctx,
                ListInput {
                    status: Some(NoteStatus::Candidate),
                    ..Default::default()
                },
            )
            .unwrap()
            .items
            .into_iter()
            .map(|i| i.id)
            .collect()
    };
    assert_eq!(candidates(&tool).len(), 2);

    let out = h
        .service
        .forget(
            &user,
            ForgetInput {
                targets: vec![(editor.id, editor.revision.clone())],
                reason: None,
            },
        )
        .unwrap();
    assert_eq!(out.results[0].status, ForgetItemStatus::Forgotten);
    assert!(candidates(&tool).is_empty(), "{:?}", candidates(&tool));
    assert!(candidates(&user).is_empty());
    for id in [copy.id, stored.id] {
        let err = h
            .service
            .read(
                &tool,
                ReadInput {
                    id,
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert_eq!(err.class(), "not_found", "{id}");
    }
    assert!(
        !read_note(&h, keymap.id).source_forgotten,
        "a withdrawn suggestion suppresses nothing it cited"
    );
}

/// Repair round 2 of the L8 product slice: the assistant's suggestion is
/// dated the day it was made, on the card and the line, as the Memory view
/// and the CLI date it — not by the newest typed row it cites, which for an
/// edit made on a turn with no typed row of its own is the older row the
/// copied note learned from.
#[test]
fn the_assistants_edit_suggestion_is_dated_the_day_it_was_made() {
    let h = Harness::new();
    let (said, _) = chat_turn(&h, "My editor is vim.", None);
    h.db.with_connection(|c| {
        c.execute(
            "UPDATE conversation_messages SET created_at = '2026-01-02 03:04:05' WHERE id = ?1",
            [said],
        )?;
        Ok(())
    })
    .unwrap();
    let editor = h
        .service
        .store(
            &h.user(ALICE, None),
            StoreInput {
                title: "Editor".into(),
                body: "# Editor\n\nThe user edits with vim.\n".into(),
                kind: Some(NoteKind::Preference),
                sources: vec![message(said)],
                ..Default::default()
            },
        )
        .unwrap();
    // A model tool with no request of its own: no turn row to cite.
    let tool = h.tool(ALICE, None);
    let out = h
        .service
        .update(
            &tool,
            UpdateInput {
                id: editor.id,
                expected_revision: editor.revision.clone(),
                action: UpdateAction::Edit(Patch {
                    body: Some("# Editor\n\nThe user edits with vim in a terminal.\n".into()),
                    ..Default::default()
                }),
                reason: None,
            },
        )
        .unwrap();
    assert_eq!(out.suggested_for, Some(editor.id));
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let preview = h
        .service
        .suggestion_preview(&tool, out.id, &out.revision, &editor.revision)
        .unwrap();
    assert!(
        preview
            .text
            .contains(&format!("Suggested by the assistant on {today}.")),
        "{}",
        preview.text
    );
    assert!(!preview.text.contains("2026-01-02"), "{}", preview.text);
}

/// An RFC 3339 instant, for a validity bound.
fn instant(text: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&chrono::Utc)
}

/// Close-out A1: the chat model's edit of a note the user saved that
/// changes any one field a suggestion carries — title, text, aliases, tags,
/// key, or either validity bound — is a suggestion carrying that change,
/// and the result says so. The exact-duplicate test used to leave validity
/// out, so a validity-only edit was answered with the saved note itself:
/// nothing written, nothing waiting for the user, and a result that read as
/// a successful update.
#[test]
fn a_chat_models_edit_of_any_one_field_of_a_saved_note_is_a_suggestion() {
    let cases: Vec<(&str, Patch)> = vec![
        (
            "title",
            Patch {
                title: Some("Editor of choice".into()),
                ..Default::default()
            },
        ),
        (
            "body",
            Patch {
                body: Some("# Editor\n\nThe user edits with Helix.\n".into()),
                ..Default::default()
            },
        ),
        (
            "aliases",
            Patch {
                aliases: Some(vec!["vi".into()]),
                ..Default::default()
            },
        ),
        (
            "tags",
            Patch {
                tags: Some(vec!["tools".into()]),
                ..Default::default()
            },
        ),
        (
            "fact_key",
            Patch {
                fact_key: Some(Some("user.editor.main".into())),
                ..Default::default()
            },
        ),
        (
            "valid_from",
            Patch {
                valid_from: Some(Some(instant("2026-01-01T00:00:00Z"))),
                ..Default::default()
            },
        ),
        (
            "valid_to",
            Patch {
                valid_to: Some(Some(instant("2026-12-31T00:00:00Z"))),
                ..Default::default()
            },
        ),
    ];
    for (field, patch) in cases {
        let h = Harness::new();
        let editor = saved(&h, "user.editor", "Editor", "The user edits with vim.");
        let (_, tool) = chat_turn(&h, "Change my editor note.", None);
        let out = h
            .service
            .update(
                &tool,
                UpdateInput {
                    id: editor.id,
                    expected_revision: editor.revision.clone(),
                    action: UpdateAction::Edit(patch),
                    reason: None,
                },
            )
            .unwrap_or_else(|e| panic!("{field}: {e}"));
        assert_eq!(out.suggested_for, Some(editor.id), "{field}: {out:?}");
        assert_ne!(out.id, editor.id, "{field}");
        assert_eq!(out.status, NoteStatus::Candidate, "{field}");
        let suggestion = read_note(&h, out.id);
        let original = read_note(&h, editor.id);
        assert_eq!(original.revision, editor.revision, "{field}: untouched");
        if field == "body" {
            assert_ne!(suggestion.body, original.body, "{field}");
        } else {
            assert_ne!(
                suggestion.frontmatter[field], original.frontmatter[field],
                "{field}: the suggestion carries the change"
            );
        }
        let listed = h.service.suggestions(&h.user(ALICE, None), None).unwrap();
        assert_eq!(
            listed.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![out.id],
            "{field}"
        );
    }
}

/// Close-out A1, on the store path and S4: a `supersedes` of a saved note
/// with its exact text but another validity window is a suggestion, not the
/// saved note; restating that suggestion is the same suggestion, and
/// restating it with yet another window is a second one.
#[test]
fn a_replacement_differing_only_in_validity_is_a_suggestion() {
    let h = Harness::new();
    let editor = saved(&h, "user.editor", "Editor", "The user edits with vim.");
    let (_, tool) = chat_turn(&h, "Only until the end of the year.", None);
    let store = |valid_to: &str| {
        h.service
            .store(
                &tool,
                StoreInput {
                    title: "Editor".into(),
                    body: "# Editor\n\nThe user edits with vim.\n".into(),
                    kind: Some(NoteKind::Preference),
                    fact_key: Some("user.editor".into()),
                    valid_to: Some(instant(valid_to)),
                    supersedes: Some((editor.id, editor.revision.clone())),
                    ..Default::default()
                },
            )
            .unwrap()
    };
    let first = store("2026-12-31T00:00:00Z");
    assert!(first.suggestion, "{first:?}");
    assert_eq!(first.dedup, Dedup::Created);
    assert_ne!(first.id, editor.id);
    let again = store("2026-12-31T00:00:00Z");
    assert_eq!((again.id, again.dedup), (first.id, Dedup::Existing));
    assert!(again.suggestion);
    let other = store("2027-06-30T00:00:00Z");
    assert!(other.suggestion);
    assert_eq!(other.dedup, Dedup::Created);
    assert_ne!(other.id, first.id);
    assert_eq!(read_note(&h, editor.id).revision, editor.revision);
}

/// Whether automatic recall of "Where does the user live?" delivers or
/// lists `id`.
fn recalled(h: &Harness, id: NoteId) -> bool {
    let r = h
        .service
        .recall(
            &h.user(ALICE, None),
            "Where does the user live? Elm Street address",
            RecallBudget::default(),
        )
        .unwrap();
    r.notes.iter().any(|n| n.id == id) || r.listed.iter().any(|n| n.id == id)
}

/// Close-out A2 (design §4: a saved note a suggestion names "stays in force
/// until the user confirms"): the chat model's edit suggestion copies the
/// saved note's sources, so a forget of the suggestion — the model's
/// retraction of it under its card, or the person's own — used to suppress
/// them and take the saved note out of automatic recall, still active and
/// never confirmed. A pending suggestion's forget now suppresses only the
/// rows its target does not cite: here the turn's own row, which a note
/// learned from the same row follows out (the sibling cost C2 accepts).
#[test]
fn forgetting_the_assistants_edit_suggestion_leaves_the_saved_note_in_recall() {
    for forget_by_the_tool in [true, false] {
        let h = Harness::new();
        let said = h.message_row("I live at 1 Elm Street.");
        let home = h
            .service
            .store(
                &h.user(ALICE, None),
                StoreInput {
                    title: "Home address".into(),
                    body: "# Home address\n\nThe user lives at 1 Elm Street.\n".into(),
                    kind: Some(NoteKind::Fact),
                    fact_key: Some("user.address".into()),
                    sources: vec![message(said)],
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(recalled(&h, home.id));
        let (turn, tool) = chat_turn(&h, "Add that it is the blue house.", None);
        let sibling = h
            .service
            .store(
                &h.ctx(ALICE, None, Surface::Extraction, Capabilities::EXTRACTION)
                    .unwrap(),
                StoreInput {
                    title: "House colour".into(),
                    body: "# House colour\n\nThe user's house is blue.\n".into(),
                    kind: Some(NoteKind::Fact),
                    sources: vec![message(turn)],
                    observed_at: Some(chrono::Utc::now()),
                    provenance: Some(ExtractionProvenance::UserWords),
                    ..Default::default()
                },
            )
            .unwrap();
        let out = h
            .service
            .update(
                &tool,
                UpdateInput {
                    id: home.id,
                    expected_revision: home.revision.clone(),
                    action: UpdateAction::Edit(Patch {
                        body: Some(
                            "# Home address\n\nThe user lives at 1 Elm Street, the blue house.\n"
                                .into(),
                        ),
                        ..Default::default()
                    }),
                    reason: None,
                },
            )
            .unwrap();
        assert_eq!(out.suggested_for, Some(home.id));
        let forgetter = if forget_by_the_tool {
            h.tool(ALICE, None)
        } else {
            h.user(ALICE, None)
        };
        let forget = h
            .service
            .forget(
                &forgetter,
                ForgetInput {
                    targets: vec![(out.id, out.revision.clone())],
                    reason: Some("the user declined the change".into()),
                },
            )
            .unwrap();
        assert_eq!(forget.results[0].status, ForgetItemStatus::Forgotten);
        let w = read_note(&h, home.id);
        assert!(
            !w.source_forgotten,
            "by the tool: {forget_by_the_tool}: the saved note stays in force"
        );
        assert_eq!(w.status, NoteStatus::Active);
        assert!(recalled(&h, home.id), "by the tool: {forget_by_the_tool}");
        assert!(
            read_note(&h, sibling.id).source_forgotten,
            "by the tool: {forget_by_the_tool}: the suggestion's own row is forgotten"
        );
    }
}

/// Close-out A2, the round-3 nit it re-checked (design §6: a forget keeps
/// the notes citing its rows "out of automatic recall until it is edited"):
/// a note the user saved, which a forget of a sibling citing the same row
/// took out of recall, is not the chat model's to edit. Its edit suggestion
/// would copy the forgotten row's text and cite the row — written after the
/// forget, so, once confirmed, back in recall — and a direct edit of one
/// edited by hand would lift the suppression at once. Refused, naming the
/// user's own ways; nothing is written.
#[test]
fn a_saved_note_a_forget_took_out_of_recall_is_not_the_models_to_edit() {
    let h = Harness::new();
    let said = h.message_row("I live at 1 Elm Street, the blue house.");
    let user = h.user(ALICE, None);
    let home = h
        .service
        .store(
            &user,
            StoreInput {
                title: "Home address".into(),
                body: "# Home address\n\nThe user lives at 1 Elm Street.\n".into(),
                kind: Some(NoteKind::Fact),
                sources: vec![message(said)],
                ..Default::default()
            },
        )
        .unwrap();
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
        text.replace(
            "sources: []",
            &format!("sources:\n  - type: message\n    id: '{said}'"),
        )
        .replace("Sentence case.", "Sentence case, always."),
    )
    .unwrap();
    h.service.reconcile_owner(ALICE).unwrap();
    let style = read_note(&h, style.id);
    assert_eq!(style.origin, Origin::External);
    assert_eq!(style.sources.len(), 1, "{:?}", style.sources);
    let sibling = h
        .service
        .store(
            &h.ctx(ALICE, None, Surface::Extraction, Capabilities::EXTRACTION)
                .unwrap(),
            StoreInput {
                title: "House colour".into(),
                body: "# House colour\n\nThe user's house is blue.\n".into(),
                kind: Some(NoteKind::Fact),
                sources: vec![message(said)],
                observed_at: Some(chrono::Utc::now()),
                provenance: Some(ExtractionProvenance::UserWords),
                ..Default::default()
            },
        )
        .unwrap();
    h.service
        .forget(
            &user,
            ForgetInput {
                targets: vec![(sibling.id, sibling.revision)],
                reason: None,
            },
        )
        .unwrap();
    assert!(read_note(&h, home.id).source_forgotten);
    assert!(read_note(&h, style.id).source_forgotten);

    let (_, tool) = chat_turn(&h, "Also call it my home.", None);
    let edits = [
        (
            home.id,
            home.revision.clone(),
            Patch {
                aliases: Some(vec!["home".into()]),
                ..Default::default()
            },
        ),
        (
            style.id,
            style.revision.clone(),
            Patch {
                review_after: Some(Some(instant("2027-01-01T00:00:00Z"))),
                ..Default::default()
            },
        ),
    ];
    for (id, revision, patch) in edits {
        let err = h
            .service
            .update(
                &tool,
                UpdateInput {
                    id,
                    expected_revision: revision.clone(),
                    action: UpdateAction::Edit(patch),
                    reason: None,
                },
            )
            .unwrap_err();
        assert_eq!(err.class(), "authority_conflict", "{id}: {err}");
        assert!(err.to_string().contains("asked to forget"), "{err}");
        assert!(
            err.to_string()
                .contains(&format!("openalpaca memory edit {id}")),
            "{err}"
        );
        let after = read_note(&h, id);
        assert_eq!(after.revision, revision, "nothing written");
        assert!(after.source_forgotten, "still out of recall");
    }
    assert!(
        h.service
            .list(
                &user,
                ListInput {
                    status: Some(NoteStatus::Candidate),
                    ..Default::default()
                },
            )
            .unwrap()
            .items
            .is_empty()
    );
}

/// Close-out A3: one forget naming a saved note and the assistant's pending
/// suggestion for it forgets both, in either order. A forgotten note's
/// suggestions are withdrawn after the forget — but not one the same call
/// names: it is forgotten as asked, so its own row is suppressed and a note
/// learned from that row follows it out. Before, `[note, suggestion]`
/// withdrew the suggestion first and answered it `not_found`.
#[test]
fn forgetting_a_note_and_its_suggestion_together_forgets_both_in_either_order() {
    for note_first in [true, false] {
        let h = Harness::new();
        let editor = saved(&h, "user.editor", "Editor", "The user edits with vim.");
        let (typed, tool) = chat_turn(&h, "I use the vim keymap in every editor.", None);
        let keymap = h
            .service
            .store(
                &h.ctx(ALICE, None, Surface::Extraction, Capabilities::EXTRACTION)
                    .unwrap(),
                StoreInput {
                    title: "Keymap".into(),
                    body: "# Keymap\n\nThe user uses the vim keymap everywhere.\n".into(),
                    kind: Some(NoteKind::Preference),
                    sources: vec![message(typed)],
                    observed_at: Some(chrono::Utc::now()),
                    provenance: Some(ExtractionProvenance::UserWords),
                    ..Default::default()
                },
            )
            .unwrap();
        let copy = h
            .service
            .update(
                &tool,
                UpdateInput {
                    id: editor.id,
                    expected_revision: editor.revision.clone(),
                    action: UpdateAction::Edit(Patch {
                        body: Some(
                            "# Editor\n\nThe user edits with vim, and its keymap everywhere.\n"
                                .into(),
                        ),
                        ..Default::default()
                    }),
                    reason: None,
                },
            )
            .unwrap();
        assert_eq!(copy.suggested_for, Some(editor.id));
        let note = (editor.id, editor.revision.clone());
        let suggestion = (copy.id, copy.revision.clone());
        let targets = if note_first {
            vec![note, suggestion]
        } else {
            vec![suggestion, note]
        };
        let out = h
            .service
            .forget(
                &h.user(ALICE, None),
                ForgetInput {
                    targets,
                    reason: None,
                },
            )
            .unwrap();
        let statuses: Vec<(NoteId, ForgetItemStatus)> = out
            .results
            .iter()
            .map(|r| (r.id, r.status.clone()))
            .collect();
        for id in [editor.id, copy.id] {
            assert!(
                statuses.contains(&(id, ForgetItemStatus::Forgotten)),
                "note first: {note_first}: {statuses:?}"
            );
        }
        assert!(
            read_note(&h, keymap.id).source_forgotten,
            "note first: {note_first}: the suggestion's own row is forgotten"
        );
    }
}

/// Close-out repair round 1 (RT1-P1, RG1 P1): a forget whose later target
/// fails still withdraws the suggestions of the notes it already forgot.
/// A3 deferred the withdrawals past the loop, and an error on a later
/// target — a quarantined partition, a file it could not remove — returned
/// before them: the saved note was gone, and the chat model's copy of its
/// text stayed a candidate the model lists and reads, with nothing left to
/// withdraw it (a retry answers the note `not_found`).
#[test]
fn a_forget_that_fails_on_a_later_target_still_withdraws_what_it_owes() {
    for variant in ["quarantined", "read-only"] {
        let h = Harness::new();
        let ws = h.workspace("lantern", Some(".git"));
        let editor = saved(&h, "user.editor", "Editor", "The user edits with vim.");
        let (_, tool) = chat_turn(&h, "I use the vim keymap in every editor.", None);
        let copy = h
            .service
            .update(
                &tool,
                UpdateInput {
                    id: editor.id,
                    expected_revision: editor.revision.clone(),
                    action: UpdateAction::Edit(Patch {
                        body: Some("# Editor\n\nThe user edits with vim, and its keymap.\n".into()),
                        ..Default::default()
                    }),
                    reason: None,
                },
            )
            .unwrap();
        assert_eq!(copy.suggested_for, Some(editor.id));
        let project = h.store(
            &h.user(ALICE, Some(&ws)),
            "Lantern",
            "# Lantern\n\nThe project is Lantern.\n",
            NoteKind::Fact,
        );
        let (snapshot, notes_dir) = {
            let resolved = h.service.resolve(&h.user(ALICE, Some(&ws))).unwrap();
            let p = resolved.project.as_ref().unwrap();
            (p.snapshot_path().to_path_buf(), p.notes_dir().to_path_buf())
        };
        let restore = if variant == "quarantined" {
            fs::remove_file(&snapshot).unwrap();
            None
        } else {
            use std::os::unix::fs::PermissionsExt;
            let before = fs::metadata(&notes_dir).unwrap().permissions();
            fs::set_permissions(&notes_dir, fs::Permissions::from_mode(0o555)).unwrap();
            Some(before)
        };
        let result = h.service.forget(
            &h.user(ALICE, Some(&ws)),
            ForgetInput {
                targets: vec![
                    (editor.id, editor.revision.clone()),
                    (project.id, project.revision.clone()),
                ],
                reason: None,
            },
        );
        if let Some(before) = restore {
            fs::set_permissions(&notes_dir, before).unwrap();
        }
        assert!(result.is_err(), "{variant}: the later target fails");
        let gone = |id: NoteId| {
            h.service
                .read(
                    &h.tool(ALICE, None),
                    ReadInput {
                        id,
                        ..Default::default()
                    },
                )
                .map(|o| o.status)
                .map_err(|e| e.class())
        };
        assert_eq!(gone(editor.id), Err("not_found"), "{variant}: W landed");
        assert_eq!(
            gone(copy.id),
            Err("not_found"),
            "{variant}: the model's copy of W's text is withdrawn"
        );
        assert!(
            h.service
                .list(
                    &h.tool(ALICE, None),
                    ListInput {
                        status: Some(NoteStatus::Candidate),
                        ..Default::default()
                    },
                )
                .unwrap()
                .items
                .is_empty(),
            "{variant}: no candidate is left for the model to list"
        );
    }
}

/// Close-out A5 (round 3's nit on `edit_sources`' cap): the chat model's
/// edit suggestion cites every row the saved note cites — the rows a later
/// forget must follow the copy out by — and the turn's own row only where
/// the eight-source cap leaves room. At the cap the copy used to drop the
/// note's last row for the turn's, so a forget of a sibling citing that row
/// left the confirmed copy, and the row's text, in automatic recall.
#[test]
fn the_edit_suggestion_keeps_every_row_the_saved_note_cites_at_the_cap() {
    for cited in [8usize, 3] {
        let h = Harness::new();
        let rows: Vec<i64> = (0..cited)
            .map(|i| h.message_row(&format!("Home detail {i}.")))
            .collect();
        let user = h.user(ALICE, None);
        let home = h
            .service
            .store(
                &user,
                StoreInput {
                    title: "Home address".into(),
                    body: "# Home address\n\nThe user lives at 1 Elm Street.\n".into(),
                    kind: Some(NoteKind::Fact),
                    sources: rows.iter().map(|r| message(*r)).collect(),
                    ..Default::default()
                },
            )
            .unwrap();
        let last = *rows.last().unwrap();
        let sibling = h
            .service
            .store(
                &h.ctx(ALICE, None, Surface::Extraction, Capabilities::EXTRACTION)
                    .unwrap(),
                StoreInput {
                    title: "House colour".into(),
                    body: "# House colour\n\nThe user's house is blue.\n".into(),
                    kind: Some(NoteKind::Fact),
                    sources: vec![message(last)],
                    observed_at: Some(chrono::Utc::now()),
                    provenance: Some(ExtractionProvenance::UserWords),
                    ..Default::default()
                },
            )
            .unwrap();
        let (turn, tool) = chat_turn(&h, "Add that it is the blue house.", None);
        let out = h
            .service
            .update(
                &tool,
                UpdateInput {
                    id: home.id,
                    expected_revision: home.revision.clone(),
                    action: UpdateAction::Edit(Patch {
                        aliases: Some(vec!["blue house".into()]),
                        ..Default::default()
                    }),
                    reason: None,
                },
            )
            .unwrap();
        assert_eq!(out.suggested_for, Some(home.id));
        let mut expected: Vec<String> = rows.iter().map(|r| format!("message:{r}")).collect();
        if cited < 8 {
            expected.push(format!("message:{turn}"));
        }
        let cites: Vec<String> = read_note(&h, out.id)
            .sources
            .iter()
            .map(NoteSource::suppression_key)
            .collect();
        assert_eq!(cites, expected, "{cited} rows");

        h.service
            .update(
                &user,
                UpdateInput {
                    id: out.id,
                    expected_revision: out.revision.clone(),
                    action: UpdateAction::Confirm {
                        target_revision: Some(home.revision.clone()),
                    },
                    reason: None,
                },
            )
            .unwrap();
        h.service
            .forget(
                &user,
                ForgetInput {
                    targets: vec![(sibling.id, sibling.revision)],
                    reason: None,
                },
            )
            .unwrap();
        assert!(
            read_note(&h, out.id).source_forgotten,
            "{cited} rows: the confirmed copy follows its last row out"
        );
    }
}

/// Close-out A4: the assistant's edit suggestion copies the saved note's
/// body — here with learning's evidence sentence from an earlier change —
/// so its card and the chat preview gave that old sentence as the "Why" of
/// the assistant's change, and a change of metadata alone showed nowhere:
/// "Yours" and "New" read the same. The assistant's suggestion gives no
/// reason of its body's, and the preview says what else changes.
#[test]
fn the_assistants_metadata_edit_shows_what_changes_and_no_borrowed_reason() {
    let h = Harness::new();
    let (said, _) = chat_turn(&h, "I edit with Helix since June.", None);
    let editor = h
        .service
        .store(
            &h.user(ALICE, None),
            StoreInput {
                title: "Editor".into(),
                body: "# Editor\n\n## Knowledge\nSince June the user edits with Helix.\n\n\
                       ## Evidence and limits\nFrom what you said on 2026-06-14. Replaces \
                       “Editor” (saved by you): The user said they switched from vim to Helix \
                       in June.\n"
                    .into(),
                kind: Some(NoteKind::Preference),
                fact_key: Some("user.editor".into()),
                sources: vec![message(said)],
                ..Default::default()
            },
        )
        .unwrap();
    let (_, tool) = chat_turn(&h, "Tag my editor note as tools, until the end of 2026.", None);
    let out = h
        .service
        .update(
            &tool,
            UpdateInput {
                id: editor.id,
                expected_revision: editor.revision.clone(),
                action: UpdateAction::Edit(Patch {
                    tags: Some(vec!["tools".into()]),
                    valid_to: Some(Some(instant("2026-12-31T00:00:00Z"))),
                    ..Default::default()
                }),
                reason: Some("the user asked for a tag".into()),
            },
        )
        .unwrap();
    assert_eq!(out.suggested_for, Some(editor.id));
    let listed = h.service.suggestions(&h.user(ALICE, None), None).unwrap();
    let [suggestion] = &listed[..] else {
        panic!("one suggestion: {listed:?}");
    };
    assert_eq!(suggestion.reason, None, "not the copied note's old reason");
    assert_eq!(
        suggestion.changes,
        vec![
            FieldChange {
                field: "tags",
                from: vec![],
                to: vec!["tools".into()],
            },
            FieldChange {
                field: "valid_to",
                from: vec![],
                to: vec!["2026-12-31T00:00:00Z".into()],
            },
        ]
    );
    assert_eq!(
        wire::suggestions_json(&listed)["items"][0]["changes"],
        serde_json::json!([
            { "field": "tags", "from": [], "to": ["tools"] },
            { "field": "valid_to", "from": [], "to": ["2026-12-31T00:00:00Z"] },
        ])
    );
    let preview = h
        .service
        .suggestion_preview(&tool, out.id, &out.revision, &editor.revision)
        .unwrap()
        .text;
    assert!(!preview.contains("vim to Helix"), "{preview}");
    assert!(
        preview.contains("\nChanges: tags none → tools; valid until none → 2026-12-31T00:00:00Z\n"),
        "{preview}"
    );
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    assert!(
        preview.contains(&format!("\nSuggested by the assistant on {today}.\n")),
        "{preview}"
    );
}
