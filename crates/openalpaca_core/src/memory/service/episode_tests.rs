//! Conversation summaries on the service (design 09 §13.3, owner decision
//! MD-16, slice L10c): an `episode` note of origin `extracted_other` that is
//! **active** — so `memory_search`, `memory_timeline`, `memory_list` and the
//! Memory view find it — and that automatic recall never delivers, per-turn
//! excerpts and the index block alike. Forgetting one forgets the summary,
//! not what was said; deleting or clearing its conversation forgets it.

use super::tests::{ALICE, Harness};
use super::*;
use crate::daemon_config::RecallMode;
use openalpaca_storage::memory_files::{
    EffectiveStatus, NoteKind, NoteScope, NoteSource, NoteStatus, Origin, SourceType,
};
use openalpaca_storage::store::StoreScope;
use std::collections::HashSet;

fn message(id: i64) -> NoteSource {
    NoteSource {
        kind: SourceType::Message,
        id: id.to_string(),
        observed_at: None,
    }
}

fn extraction(h: &Harness, workspace: Option<&std::path::Path>) -> MemoryAccessContext {
    h.ctx(
        ALICE,
        workspace,
        Surface::Extraction,
        Capabilities::EXTRACTION,
    )
    .unwrap()
}

/// A summary as the episode writer stores it.
fn summary_input(title: &str, knowledge: &str, rows: &[i64]) -> StoreInput {
    StoreInput {
        title: title.to_string(),
        body: format!("# {title}\n\n## Knowledge\n{knowledge}\n"),
        kind: Some(NoteKind::Episode),
        sources: rows.iter().copied().map(message).collect(),
        observed_at: Some(chrono::Utc::now()),
        provenance: Some(ExtractionProvenance::Summary),
        ..Default::default()
    }
}

fn summary(h: &Harness, title: &str, knowledge: &str, rows: &[i64]) -> StoreOutput {
    h.service
        .store(&extraction(h, None), summary_input(title, knowledge, rows))
        .unwrap()
}

/// A note learned from the user's words, citing `rows`.
fn learned(h: &Harness, title: &str, knowledge: &str, rows: &[i64]) -> StoreOutput {
    h.service
        .store(
            &extraction(h, None),
            StoreInput {
                title: title.to_string(),
                body: format!("# {title}\n\n## Knowledge\n{knowledge}\n"),
                kind: Some(NoteKind::Fact),
                sources: rows.iter().copied().map(message).collect(),
                provenance: Some(ExtractionProvenance::UserWords),
                ..Default::default()
            },
        )
        .unwrap()
}

fn search_ids(h: &Harness, ctx: &MemoryAccessContext, query: &str) -> Vec<NoteId> {
    h.service
        .search(
            ctx,
            SearchInput {
                query: query.to_string(),
                ..Default::default()
            },
        )
        .unwrap()
        .results
        .into_iter()
        .map(|r| r.id)
        .collect()
}

/// **§13.3, *Authority and visibility*.** The writer's provenance records
/// origin `extracted_other` — the model's own wording, the lowest rank — and
/// the note is `active`, not a candidate: `memory_list` (and so the Memory
/// view) lists it among the active notes.
#[test]
fn a_conversation_summary_is_written_active_with_origin_extracted_other() {
    let h = Harness::new();
    let row = h.message_row("We planned the Lisbon trip.");
    let out = summary(
        &h,
        "Conversation on 2026-09-20: Lisbon trip",
        "Discussed: the Lisbon trip.",
        &[row],
    );
    assert_eq!(out.origin, Origin::ExtractedOther);
    assert!(!out.suggestion);

    let items = h
        .service
        .list(&h.user(ALICE, None), ListInput::default())
        .unwrap()
        .items;
    let item = items
        .iter()
        .find(|i| i.id == out.id)
        .expect("listed among the active notes");
    assert_eq!(item.status, NoteStatus::Active);
    assert_eq!(item.effective_status, EffectiveStatus::Active);
    assert_eq!(item.origin, Origin::ExtractedOther);
    assert_eq!(item.kind, NoteKind::Episode);
}

/// The one active `extracted_other` note is a plain `episode`: the summary
/// provenance is refused for any other kind, a `fact_key`, a `supersedes`
/// or a replace policy — a summary "can supersede nothing" — and outside
/// the extractor's own surface.
#[test]
fn the_summary_provenance_writes_only_a_plain_episode() {
    let h = Harness::new();
    let row = h.message_row("We planned the Lisbon trip.");
    let base = || {
        summary_input(
            "Conversation on 2026-09-20: Lisbon trip",
            "Discussed.",
            &[row],
        )
    };
    let target = h.store(
        &h.user(ALICE, None),
        "Trip",
        "# Trip\n\n## Knowledge\nA trip.\n",
        NoteKind::Fact,
    );
    for (why, input) in [
        (
            "kind fact",
            StoreInput {
                kind: Some(NoteKind::Fact),
                ..base()
            },
        ),
        (
            "a fact_key",
            StoreInput {
                fact_key: Some("user.trip".to_string()),
                ..base()
            },
        ),
        (
            "a supersedes",
            StoreInput {
                supersedes: Some((target.id, target.revision.clone())),
                ..base()
            },
        ),
        (
            "a replace policy",
            StoreInput {
                replace_policy: ReplacePolicy::SuggestUnlessLearned,
                ..base()
            },
        ),
    ] {
        let err = h
            .service
            .store(&extraction(&h, None), input)
            .expect_err(why);
        assert!(matches!(err, MemoryError::InvalidInput(_)), "{why}: {err}");
    }
    let err = h
        .service
        .store(&h.user(ALICE, None), base())
        .expect_err("a user-directed write carries no provenance");
    assert!(matches!(err, MemoryError::InvalidInput(_)), "{err}");
}

/// **§13.3: "automatic recall excludes" a summary, "per-turn recall and the
/// index block alike".** With a summary, an episode the user saved and a
/// fact all matching the turn, neither recall mode delivers or lists the
/// summary — while the user's own episode and the fact are recalled — and
/// `memory_search`, `memory_timeline` and `memory_list` all return it.
#[test]
fn a_conversation_summary_is_never_recalled_but_search_timeline_and_list_find_it() {
    let h = Harness::new();
    let user = h.user(ALICE, None);
    let row = h.message_row("We planned the Lisbon trip.");
    let summary = summary(
        &h,
        "Conversation on 2026-09-20: Lisbon trip planning",
        "Discussed: flights to Lisbon and a hotel near Alfama. Decided: fly on Friday.",
        &[row],
    );
    let own_episode = h.store(
        &user,
        "Lisbon office visit",
        "# Lisbon office visit\n\n## Knowledge\nI visited the Lisbon office in May.\n",
        NoteKind::Episode,
    );
    let fact = h.store(
        &user,
        "Lisbon hotel",
        "# Lisbon hotel\n\n## Knowledge\nI stay near Alfama in Lisbon.\n",
        NoteKind::Fact,
    );

    let tool = h.tool(ALICE, None);
    for mode in [RecallMode::Index, RecallMode::Excerpts] {
        let recalled = h
            .service
            .recall(&tool, "Lisbon trip", RecallBudget::for_mode(mode))
            .unwrap();
        let delivered: HashSet<NoteId> = recalled
            .notes
            .iter()
            .map(|n| n.id)
            .chain(recalled.listed.iter().map(|l| l.id))
            .collect();
        assert!(
            !delivered.contains(&summary.id),
            "{mode:?}: the summary was recalled on its own"
        );
        assert!(
            delivered.contains(&own_episode.id),
            "{mode:?}: {delivered:?}"
        );
        assert!(delivered.contains(&fact.id), "{mode:?}: {delivered:?}");
    }

    assert!(
        search_ids(&h, &tool, "Lisbon trip").contains(&summary.id),
        "memory_search finds it"
    );
    let timeline = h
        .service
        .timeline(
            &tool,
            TimelineInput {
                anchor: TimelineAnchor::Note(own_episode.id),
                before: Some(5),
                after: Some(5),
            },
        )
        .unwrap();
    assert!(
        timeline.rows.iter().any(|r| r.id == summary.id),
        "memory_timeline lists it"
    );
    let listed = h.list_ids(
        &user,
        ListInput {
            kind: Some(NoteKind::Episode),
            ..Default::default()
        },
    );
    assert_eq!(listed, HashSet::from([summary.id, own_episode.id]));
}

/// A summary never enters the extractor's prompt either (it "never enters a
/// prompt unless the model or the user asks for it"): learning's known-notes
/// lookup neither shows it nor counts it among this window's notes, even
/// when it cites the chunk's own rows and matches its words.
#[test]
fn a_conversation_summary_is_never_shown_to_the_extractor() {
    let h = Harness::new();
    let row = h.message_row("We planned the Lisbon trip.");
    let summary = summary(
        &h,
        "Conversation on 2026-09-20: Lisbon trip planning",
        "Discussed: flights to Lisbon.",
        &[row],
    );
    let fact = learned(
        &h,
        "Lisbon trip",
        "The user flies to Lisbon on Friday.",
        &[row],
    );

    let view = h
        .service
        .learning_view(&extraction(&h, None), &["Lisbon trip".to_string()], &[row])
        .unwrap();
    let shown: Vec<NoteId> = view.notes.iter().map(|n| n.id).collect();
    let window: Vec<NoteId> = view.from_this_window.iter().map(|n| n.id).collect();
    assert!(!shown.contains(&summary.id), "{shown:?}");
    assert!(!window.contains(&summary.id), "{window:?}");
    assert!(
        shown.contains(&fact.id),
        "the learned note still is: {shown:?}"
    );
    assert!(window.contains(&fact.id), "{window:?}");
}

/// **§13.3, *Forgetting*.** Forgetting a summary — from the Memory view, a
/// tool or `/forget` — takes the summary, and only it: its tombstone
/// suppresses no source, so what the same messages taught stays learned and
/// recallable instead of turning `source_forgotten`.
#[test]
fn forgetting_a_summary_forgets_the_summary_not_what_was_said() {
    let h = Harness::new();
    let row = h.message_row("I fly to Lisbon on Friday.");
    let summary = summary(
        &h,
        "Conversation on 2026-09-20: Lisbon trip planning",
        "Discussed: flights to Lisbon.",
        &[row],
    );
    let fact = learned(
        &h,
        "Lisbon flight",
        "The user flies to Lisbon on Friday.",
        &[row],
    );

    let out = h
        .service
        .forget(
            &h.user(ALICE, None),
            ForgetInput {
                targets: vec![(summary.id, summary.revision.clone())],
                reason: None,
            },
        )
        .unwrap();
    assert_eq!(out.results[0].status, ForgetItemStatus::Forgotten);

    let snapshot = h
        .snapshot(&StoreScope::Home, ALICE)
        .expect("the home partition has a snapshot");
    let tomb = snapshot
        .tombstones
        .iter()
        .find(|t| t.id == summary.id)
        .expect("the summary is tombstoned");
    assert!(tomb.suppressed_sources.is_empty(), "{tomb:?}");
    assert!(
        h.service
            .suppressed_sources(&extraction(&h, None), &[format!("message:{row}")])
            .unwrap()
            .is_empty()
    );

    let tool = h.tool(ALICE, None);
    let hits = h
        .service
        .search(
            &tool,
            SearchInput {
                query: "Lisbon flight".to_string(),
                ..Default::default()
            },
        )
        .unwrap()
        .results;
    let hit = hits
        .iter()
        .find(|r| r.id == fact.id)
        .expect("the fact is still found");
    assert!(!hit.source_forgotten, "the fact's row was not forgotten");
    assert!(
        !hits.iter().any(|r| r.id == summary.id),
        "the summary is gone"
    );
    let recalled = h
        .service
        .recall(
            &tool,
            "Lisbon flight",
            RecallBudget::for_mode(RecallMode::Excerpts),
        )
        .unwrap();
    assert!(recalled.notes.iter().any(|n| n.id == fact.id));
}

/// A model tool's edit or archive leaves a summary a summary (review of
/// L10c, finding 2). A tool call records `extracted_user` for what it
/// writes, and a summary re-recorded so would be recalled on its own, read
/// as learned from what the user said, outlive its conversation's delete
/// and, forgotten, suppress the conversation's rows. The assistant's
/// rewording of the assistant's record keeps origin `extracted_other`; the
/// user's own edit makes it the user's note, as any user edit does.
#[test]
fn a_model_edit_or_archive_leaves_a_summary_a_summary() {
    let h = Harness::new();
    let row = h.message_row("We planned the Lisbon trip.");
    let (tool, user) = (h.tool(ALICE, None), h.user(ALICE, None));
    let origin = |id: NoteId| {
        h.service
            .read(
                &user,
                ReadInput {
                    id,
                    ..Default::default()
                },
            )
            .unwrap()
            .origin
    };
    let update = |ctx: &MemoryAccessContext, note: &StoreOutput, action: UpdateAction| {
        h.service
            .update(
                ctx,
                UpdateInput {
                    id: note.id,
                    expected_revision: note.revision.clone(),
                    action,
                    reason: None,
                },
            )
            .unwrap()
    };
    let body = |title: &str| Patch {
        body: Some(format!(
            "# {title}\n\n## Knowledge\nDiscussed: flights to Lisbon and a hotel near Alfama.\n"
        )),
        ..Default::default()
    };

    let title = "Conversation on 2026-09-20: Lisbon trip planning";
    let edited = summary(&h, title, "Discussed: flights to Lisbon.", &[row]);
    update(&tool, &edited, UpdateAction::Edit(body(title)));
    assert_eq!(origin(edited.id), Origin::ExtractedOther, "a model edit");
    for mode in [RecallMode::Index, RecallMode::Excerpts] {
        let recalled = h
            .service
            .recall(&tool, "Lisbon hotel Alfama", RecallBudget::for_mode(mode))
            .unwrap();
        assert!(
            !recalled.notes.iter().any(|n| n.id == edited.id)
                && !recalled.listed.iter().any(|l| l.id == edited.id),
            "{mode:?}: the edited summary was recalled on its own"
        );
    }
    let archived = summary(
        &h,
        "Conversation on 2026-09-21: Lisbon",
        "Discussed.",
        &[row],
    );
    update(&tool, &archived, UpdateAction::Archive);
    assert_eq!(
        origin(archived.id),
        Origin::ExtractedOther,
        "a model archive"
    );

    let mine = "Conversation on 2026-09-22: Lisbon";
    let owned = summary(&h, mine, "Discussed.", &[row]);
    update(&user, &owned, UpdateAction::Edit(body(mine)));
    assert_eq!(origin(owned.id), Origin::User, "the user's own edit");

    assert_eq!(
        h.service
            .forget_conversation_summaries(ALICE, None, &[row])
            .unwrap(),
        2,
        "the delete forgets both summaries, not the user's note"
    );
    assert!(
        h.service
            .suppressed_sources(&extraction(&h, None), &[format!("message:{row}")])
            .unwrap()
            .is_empty(),
        "and suppresses none of the conversation's rows"
    );
}

/// A summary holds no key for learning (review of L10c, R1). Since owner
/// decision LQ10 a model's edit may not put a `fact_key` on one — it is
/// `authority_conflict` — but a summary an older build let a model key stays
/// a summary; were it the key's target, the user's own later statement
/// under that key would be parked as a suggestion to replace the
/// assistant's record instead of being learned. It is neither a key's
/// target nor among the known keys.
#[test]
fn a_key_on_a_summary_is_no_key_for_learning() {
    let h = Harness::new();
    let row = h.message_row("We planned the Lisbon trip.");
    let learning = extraction(&h, None);
    let refused = summary(
        &h,
        "Conversation on 2026-09-19: Lisbon",
        "Discussed.",
        &[row],
    );
    let err = h
        .service
        .update(
            &h.tool(ALICE, None),
            UpdateInput {
                id: refused.id,
                expected_revision: refused.revision.clone(),
                action: UpdateAction::Edit(Patch {
                    fact_key: Some(Some("user.editor".to_string())),
                    ..Default::default()
                }),
                reason: None,
            },
        )
        .unwrap_err();
    assert_eq!(err.class(), "authority_conflict", "{err}");

    // What an older build let a model's edit leave, written below the
    // service's rules.
    let title = "Conversation on 2026-09-20: Lisbon trip planning";
    let note = h
        .service
        .open_partition(&StoreScope::Home, ALICE)
        .unwrap()
        .create(
            NewNote {
                title: title.to_string(),
                body: format!("# {title}\n\n## Knowledge\nDiscussed: flights to Lisbon.\n"),
                kind: Some(NoteKind::Episode),
                status: Some(NoteStatus::Active),
                fact_key: Some("user.editor".to_string()),
                sources: vec![message(row)],
                ..Default::default()
            },
            files::Actor::Extraction {
                origin: Origin::ExtractedOther,
            },
        )
        .unwrap();
    h.service.rebuild_index().unwrap();
    let read = h
        .service
        .read(
            &h.user(ALICE, None),
            ReadInput {
                id: note.id,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        (read.origin, read.status),
        (Origin::ExtractedOther, NoteStatus::Active),
        "still a summary"
    );
    assert_eq!(read.frontmatter["fact_key"], "user.editor");

    let targets = h
        .service
        .learning_key_targets(&learning, &["user.editor".to_string()])
        .unwrap();
    assert!(
        targets.is_empty(),
        "a summary is no key's target: {targets:?}"
    );
    let view = h.service.learning_view(&learning, &[], &[row]).unwrap();
    assert!(view.keys.is_empty(), "known keys: {:?}", view.keys);
}

/// An `episode` candidate the extractor proposed from the assistant's words
/// is origin `extracted_other` too, and a candidate like any other, not a
/// conversation summary (review of L10c, finding 1): it counts as no
/// summary covering its rows, learning's re-run guard sees it among the
/// window's notes, deleting its conversation leaves it, and forgetting it
/// is an ordinary forget — the messages it cites are suppressed.
#[test]
fn a_learned_episode_candidate_is_not_a_conversation_summary() {
    let h = Harness::new();
    let (a, b) = (
        h.message_row("Let's plan the Lisbon trip."),
        h.message_row("Happy to help with Lisbon."),
    );
    let learning = extraction(&h, None);
    let candidate = h
        .service
        .store(
            &learning,
            StoreInput {
                title: "Lisbon trip talk".to_string(),
                body: "# Lisbon trip talk\n\n## Knowledge\nWe started planning a Lisbon trip.\n"
                    .to_string(),
                kind: Some(NoteKind::Episode),
                sources: vec![message(a), message(b)],
                provenance: Some(ExtractionProvenance::OtherOutput),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(candidate.origin, Origin::ExtractedOther);
    let user = h.user(ALICE, None);
    let candidates = || {
        h.list_ids(
            &user,
            ListInput {
                status: Some(NoteStatus::Candidate),
                ..Default::default()
            },
        )
    };
    assert!(candidates().contains(&candidate.id));

    assert_eq!(
        h.service.summary_coverage(&learning, &[a, b]).unwrap(),
        None,
        "no summary covers these rows"
    );
    let view = h.service.learning_view(&learning, &[], &[a, b]).unwrap();
    assert!(
        view.from_this_window.iter().any(|n| n.id == candidate.id),
        "the re-run guard sees it: {:?}",
        view.from_this_window
    );
    assert_eq!(
        h.service
            .forget_conversation_summaries(ALICE, None, &[a, b])
            .unwrap(),
        0
    );
    assert!(candidates().contains(&candidate.id), "the delete left it");

    h.service
        .forget(
            &user,
            ForgetInput {
                targets: vec![(candidate.id, candidate.revision.clone())],
                reason: None,
            },
        )
        .unwrap();
    let keys = [format!("message:{a}"), format!("message:{b}")];
    assert_eq!(
        h.service.suppressed_sources(&learning, &keys).unwrap(),
        keys.into_iter().collect::<HashSet<_>>(),
        "an ordinary forget suppresses what it cites"
    );
}

/// **§13.3: deleting or clearing a conversation "also forgets the episodes
/// whose sources all lie in that session"** — in the home partition and in
/// the conversation's project — and nothing else: not a summary that also
/// cites another conversation's row, not an episode the user saved citing
/// the same rows, not a note learned from them.
#[test]
fn deleting_a_conversation_forgets_the_summaries_whose_rows_all_lie_in_it() {
    let h = Harness::new();
    let project = h.workspace("proj", Some(".git"));
    let (a, b) = (h.message_row("one"), h.message_row("two"));
    let elsewhere = h.message_row("another conversation");

    let whole = summary(
        &h,
        "Conversation on 2026-09-20: one",
        "Discussed: one.",
        &[a, b],
    );
    let straddling = summary(
        &h,
        "Conversation on 2026-09-20: two",
        "Discussed: two.",
        &[b, elsewhere],
    );
    let in_project = h
        .service
        .store(
            &extraction(&h, Some(&project)),
            summary_input(
                "Conversation on 2026-09-20: build",
                "Discussed: the build.",
                &[a],
            ),
        )
        .unwrap();
    assert_eq!(in_project.scope, NoteScope::Project);
    let saved = h
        .service
        .store(
            &h.user(ALICE, None),
            StoreInput {
                title: "Moved the office".to_string(),
                body: "# Moved the office\n\n## Knowledge\nWe moved.\n".to_string(),
                kind: Some(NoteKind::Episode),
                sources: vec![message(a)],
                ..Default::default()
            },
        )
        .unwrap();
    let fact = learned(&h, "One", "The user said one.", &[a]);

    let forgotten = h
        .service
        .forget_conversation_summaries(ALICE, Some(&project), &[a, b])
        .unwrap();
    assert_eq!(forgotten, 2);

    let user = h.user(ALICE, Some(&project));
    let left = h.list_ids(&user, ListInput::default());
    assert!(!left.contains(&whole.id));
    assert!(!left.contains(&in_project.id));
    for kept in [straddling.id, saved.id, fact.id] {
        assert!(left.contains(&kept), "{kept} was forgotten too: {left:?}");
    }
    assert_eq!(
        h.service
            .forget_conversation_summaries(ALICE, Some(&project), &[a, b])
            .unwrap(),
        0,
        "nothing is left to forget"
    );
    assert_eq!(
        h.service
            .forget_conversation_summaries(ALICE, None, &[])
            .unwrap(),
        0
    );
}
