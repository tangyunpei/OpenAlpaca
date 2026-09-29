//! A conversation's close (design 11 §2, owner decision CQ-2): its working
//! state's refresh — one summariser call, only when a user turn lies past
//! what the state covers — and the conversation summary projected from the
//! state (design 09 §13.3, owner decision MD-16, slice L10c), with its
//! triggers and its gates, over the learning tests' scene — a persisted
//! window, a scripted model, a manual queue unless a test builds an
//! automatic one.

use super::*;
use crate::daemon_config::RecallMode;
use crate::memory::service::{
    Patch, RecallBudget, SearchInput, TimelineAnchor, TimelineInput, UpdateAction, UpdateInput,
};
use crate::orchestrator::working_state::{USAGE_LABEL, WorkingState};

/// The working state the scripted summariser answers.
fn trip() -> WorkingState {
    WorkingState {
        v: 1,
        topic: "Lisbon trip planning".into(),
        goal: "Flights to Lisbon and a hotel near Alfama.".into(),
        done: Vec::new(),
        decisions: vec!["Fly out on Friday 2 October.".into()],
        open: vec!["Which museum to visit.".into()],
        next: String::new(),
        refs: Vec::new(),
    }
}

fn trip_state() -> Say {
    Say::Text(serde_json::to_string(&trip()).unwrap())
}

const TURNS: [&str; 4] = [
    "Let's plan my Lisbon trip.",
    "I'd rather stay near Alfama.",
    "Book the Friday 2 October flight.",
    "Which museum should I see?",
];

/// Four of the user's turns and their answers, the last at 10:03 on
/// 2026-09-20 UTC; the rows, in order.
fn four_turns(scene: &Scene, s: &str) -> Vec<i64> {
    let mut rows = Vec::new();
    for (i, text) in TURNS.iter().enumerate() {
        let (q, a) = scene.turn(s, text, &format!("Answer {i}."));
        scene.set_created_at(q, &format!("2026-09-20 10:0{i}:00"));
        scene.set_created_at(a, &format!("2026-09-20 10:0{i}:30"));
        rows.extend([q, a]);
    }
    rows
}

fn episode_cursor(scene: &Scene, s: &str) -> i64 {
    LearningCursorRepository::new(&scene.db)
        .episode_through(s)
        .unwrap()
}

/// The episodes of the owner's home partition.
fn episodes(scene: &Scene) -> Vec<ListItem> {
    scene
        .active()
        .into_iter()
        .filter(|i| i.kind == NoteKind::Episode)
        .collect()
}

/// The summariser's calls today, from `llm_usage_daily`.
fn summariser_calls_today(scene: &Scene) -> i64 {
    openalpaca_storage::LlmUsageRepository::new(&scene.db)
        .usage_today(USAGE_LABEL)
        .unwrap()
        .0
}

/// The session row: its working state, `through` and `episode_note_id`.
fn session_row(scene: &Scene, s: &str) -> openalpaca_storage::Conversation {
    scene.repo().get_session(s).unwrap().unwrap()
}

fn state_on_row(scene: &Scene, s: &str) -> Option<WorkingState> {
    let row = session_row(scene, s);
    (!row.working_state.is_empty()).then(|| serde_json::from_str(&row.working_state).unwrap())
}

fn usage(label: &str, requests: i32, cost: f64) -> openalpaca_storage::LlmUsageDaily {
    openalpaca_storage::LlmUsageDaily {
        agent_id: label.to_string(),
        ..usage_row(chrono::Utc::now().date_naive(), requests, cost)
    }
}

// ── The refresh (design 11 §2, §3.1) ──────────────────────────────────────

/// Design 11 §2's close row: "1 only if ≥ 1 user turn lies past `through`,
/// else 0". Two turns past an empty state make one call, whose state lands
/// on the row with `through` at the newest row; a second close makes none,
/// nor does a row past `through` that is no user turn; the next user turn
/// makes one again.
#[tokio::test]
async fn the_close_calls_the_summariser_once_when_stale_and_not_when_current() {
    let scene = Scene::new(vec![trip_state(), trip_state()]);
    let s = scene.session();
    scene.turn(&s, TURNS[0], "Answer 0.");
    let (_, last) = scene.turn(&s, TURNS[1], "Answer 1.");

    let stale = scene.queue.flush_episodes(&s).await;
    assert_eq!((stale.calls, stale.episodes), (1, 0), "{stale:?}");
    assert_eq!(stale.stopped, None);
    assert_eq!(state_on_row(&scene, &s), Some(trip()));
    let row = session_row(&scene, &s);
    assert_eq!(
        (row.working_state_through, row.working_state_version),
        (last, 1)
    );
    assert_eq!(summariser_calls_today(&scene), 1);

    let current = scene.queue.flush_episodes(&s).await;
    assert_eq!((current.calls, current.stopped), (0, None), "{current:?}");
    scene.row(&s, "assistant", "A notice.", "gui", None, None);
    let notice = scene.queue.flush_episodes(&s).await;
    assert_eq!(notice.calls, 0, "no user turn past through: {notice:?}");
    assert_eq!(scene.model.calls(), 1);

    let (_, newest) = scene.turn(&s, TURNS[2], "Answer 2.");
    let again = scene.queue.flush_episodes(&s).await;
    assert_eq!(again.calls, 1, "{again:?}");
    let row = session_row(&scene, &s);
    assert_eq!(
        (row.working_state_through, row.working_state_version),
        (newest, 2)
    );
    let prompt = scene.model.prompts().last().cloned().unwrap();
    assert!(prompt.contains(TURNS[2]), "{prompt}");
    assert!(
        !prompt.contains(TURNS[0]),
        "a row the state covers: {prompt}"
    );
}

/// The refresh reads the rows up to a turn still in flight, whose answer
/// has not landed: its user row is not read, and `through` stays before it,
/// so the next close reads it with its answer.
#[tokio::test]
async fn the_refresh_reads_up_to_a_turn_in_flight() {
    let scene = Scene::new(vec![trip_state()]);
    let s = scene.session();
    let (_, answered) = scene.turn(&s, TURNS[0], "Answer 0.");
    let request = uuid::Uuid::new_v4();
    let turn = scene.queue.turn_started(request, LANE, true);
    turn.attach_session(&s);
    scene.row(
        &s,
        "user",
        "A question still being answered.",
        "gui",
        Some(&request.to_string()),
        None,
    );

    let report = scene.queue.flush_episodes(&s).await;
    assert_eq!(report.calls, 1, "{report:?}");
    let prompt = &scene.model.prompts()[0];
    assert!(prompt.contains(TURNS[0]), "{prompt}");
    assert!(!prompt.contains("still being answered"), "{prompt}");
    assert_eq!(session_row(&scene, &s).working_state_through, answered);
    drop(turn);
}

// ── The note (§13.3, *What*, *Authority and visibility*) ──────────────────

/// "Only when at least 4 user turns have accumulated since the session's
/// last episode": three turns get the state refreshed and no episode; the
/// fourth gets the episode, and the episode cursor moves past it; three more
/// get the state refreshed again and no second episode.
#[tokio::test]
async fn no_episode_before_four_user_turns() {
    let scene = Scene::new(vec![trip_state(), trip_state(), trip_state()]);
    let s = scene.session();
    for (i, text) in TURNS[..3].iter().enumerate() {
        scene.turn(&s, text, &format!("Answer {i}."));
    }
    let early = scene.queue.flush_episodes(&s).await;
    assert_eq!(early.stopped, None, "{early:?}");
    assert_eq!((early.calls, early.episodes), (1, 0), "{early:?}");
    assert!(episodes(&scene).is_empty());
    assert_eq!(episode_cursor(&scene, &s), 0);
    assert_eq!(session_row(&scene, &s).episode_note_id, None);

    let (_, last) = scene.turn(&s, TURNS[3], "Answer 3.");
    let report = scene.queue.flush_episodes(&s).await;
    assert_eq!((report.calls, report.episodes), (1, 1), "{report:?}");
    assert_eq!(report.stopped, None);
    assert_eq!(episodes(&scene).len(), 1);
    assert_eq!(episode_cursor(&scene, &s), last);

    let again = scene.queue.flush_episodes(&s).await;
    assert_eq!((again.calls, again.episodes), (0, 0), "{again:?}");

    for text in &TURNS[..3] {
        scene.turn(&s, text, "Later.");
    }
    let three_more = scene.queue.flush_episodes(&s).await;
    assert_eq!(
        (three_more.calls, three_more.episodes),
        (1, 0),
        "{three_more:?}"
    );
    assert_eq!(episodes(&scene).len(), 1);
    assert_eq!(episode_cursor(&scene, &s), last);
}

/// §13.3's note, MD-16's frontmatter unchanged: `kind: episode`, the title
/// `Conversation on <YYYY-MM-DD>: <topic>`, what was discussed, decided and
/// left open in at most 1 KiB of prose — projected from the working state
/// (design 11 §3.4) — `observed_at` the last turn's time, the window's rows
/// as sources, no `fact_key`, no aliases, no `supersedes`; origin
/// `extracted_other`, status `active`; its id on the session row. The one
/// call is the summariser's: no reasoning, temperature 0, 1 536 tokens,
/// recorded under its label, reading the rows the state did not cover.
#[tokio::test]
async fn an_episode_is_one_active_extracted_other_note_with_the_designs_fields() {
    let scene = Scene::new(vec![trip_state()]);
    let s = scene.session();
    let rows = four_turns(&scene, &s);
    let report = scene.queue.flush_episodes(&s).await;
    assert_eq!((report.calls, report.episodes), (1, 1), "{report:?}");

    let listed = episodes(&scene);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].origin, Origin::ExtractedOther);
    assert_eq!(listed[0].status, NoteStatus::Active);
    let note = scene.read(&scene.user_ctx(), listed[0].id);
    assert_eq!(
        note.title,
        "Conversation on 2026-09-20: Lisbon trip planning"
    );
    // Every field but the id and the write times, as the episode writer
    // before the projection wrote them (its note, on this window, compared
    // field by field at the parent commit).
    let mut fm = note.frontmatter.clone();
    for volatile in ["id", "created_at", "updated_at"] {
        assert!(fm[volatile].is_string(), "{volatile}: {fm}");
        fm.as_object_mut().unwrap().remove(volatile);
    }
    let sources: Vec<serde_json::Value> = rows
        .iter()
        .zip([
            "10:00:00", "10:00:30", "10:01:00", "10:01:30", "10:02:00", "10:02:30", "10:03:00",
            "10:03:30",
        ])
        .map(|(id, at)| {
            serde_json::json!({
                "id": id.to_string(),
                "observed_at": format!("2026-09-20T{at}Z"),
                "type": "message",
            })
        })
        .collect();
    assert_eq!(
        fm,
        serde_json::json!({
            "aliases": [],
            "fact_key": null,
            "kind": "episode",
            "observed_at": "2026-09-20T10:03:30Z",
            "pinned": false,
            "review_after": null,
            "schema": 1,
            "sources": sources,
            "status": "active",
            "supersedes": [],
            "tags": [],
            "title": "Conversation on 2026-09-20: Lisbon trip planning",
            "valid_from": null,
            "valid_to": null,
        })
    );
    let cited: Vec<i64> = note
        .sources
        .iter()
        .map(|s| {
            assert_eq!(s.kind, SourceType::Message);
            s.id.parse().unwrap()
        })
        .collect();
    assert_eq!(cited, rows, "the window's eight rows");
    let knowledge = openalpaca_storage::memory_files::knowledge_text(&note.body);
    assert_eq!(
        knowledge,
        "Discussed: Flights to Lisbon and a hotel near Alfama.\n\
         Decided: Fly out on Friday 2 October.\n\
         Left open: Which museum to visit."
    );
    assert!(knowledge.len() <= EPISODE_MAX_KNOWLEDGE_BYTES);
    assert_eq!(
        session_row(&scene, &s).episode_note_id,
        Some(listed[0].id.to_string()),
        "the session names the summary projected from its state"
    );

    let request = scene.model.first_request();
    assert!(
        matches!(
            request.thinking,
            Some(openalpaca_llm::ThinkingConfig::Disabled)
        ),
        "{:?}",
        request.thinking
    );
    assert_eq!(request.temperature, Some(0.0));
    assert_eq!(request.max_tokens, Some(1536));
    let prompt = &scene.model.prompts()[0];
    for text in TURNS {
        assert!(prompt.contains(&format!("[user]: {text}")), "{prompt}");
    }
    assert_eq!(summariser_calls_today(&scene), 1);
}

/// A long conversation's episode still cites at most eight rows (design 03
/// §7a's cap on `sources`), spread from its first row to its last.
#[tokio::test]
async fn a_long_conversation_cites_eight_rows_first_to_last() {
    let scene = Scene::new(vec![trip_state()]);
    let s = scene.session();
    let mut rows = Vec::new();
    for i in 0..10 {
        let (q, a) = scene.turn(&s, &format!("Turn {i} about the trip."), "ok");
        rows.extend([q, a]);
    }
    scene.queue.flush_episodes(&s).await;
    let note = scene.read(&scene.user_ctx(), episodes(&scene)[0].id);
    let cited: Vec<i64> = note.sources.iter().map(|s| s.id.parse().unwrap()).collect();
    assert_eq!(cited.len(), 8, "{cited:?}");
    assert_eq!(cited.first(), rows.first());
    assert_eq!(cited.last(), rows.last());
}

/// §13.3: "an episode is returned by search and timeline and **never** by
/// per-turn recall" — the writer's own note, in both recall modes.
#[tokio::test]
async fn an_episode_is_found_by_search_and_timeline_and_never_by_recall() {
    let scene = Scene::new(vec![trip_state()]);
    let s = scene.session();
    four_turns(&scene, &s);
    scene.queue.flush_episodes(&s).await;
    let id = episodes(&scene)[0].id;

    let ctx = scene.user_ctx();
    let hits = scene
        .service
        .search(
            &ctx,
            SearchInput {
                query: "Lisbon trip".to_string(),
                ..Default::default()
            },
        )
        .unwrap()
        .results;
    assert!(hits.iter().any(|h| h.id == id), "{hits:?}");
    let timeline = scene
        .service
        .timeline(
            &ctx,
            TimelineInput {
                anchor: TimelineAnchor::Query("Lisbon".to_string()),
                before: None,
                after: None,
            },
        )
        .unwrap();
    assert_eq!(timeline.anchor_note, Some(id));
    assert!(timeline.rows.iter().any(|r| r.id == id));
    for mode in [RecallMode::Index, RecallMode::Excerpts] {
        let recalled = scene
            .service
            .recall(&ctx, "Lisbon trip", RecallBudget::for_mode(mode))
            .unwrap();
        assert!(recalled.is_empty(), "{mode:?}: {recalled:?}");
    }
}

/// LQ4 and §13.3: deleting the conversation — its message ids read first,
/// as the route reads them — forgets its episode.
#[tokio::test]
async fn deleting_the_conversation_forgets_its_episode() {
    let scene = Scene::new(vec![trip_state()]);
    let s = scene.session();
    four_turns(&scene, &s);
    scene.queue.flush_episodes(&s).await;
    assert_eq!(episodes(&scene).len(), 1);

    let ids = scene.repo().message_ids(&s).unwrap();
    assert!(scene.repo().delete_session(&s).unwrap());
    let forgotten = scene
        .service
        .forget_conversation_summaries(OWNER, None, &ids)
        .unwrap();
    assert_eq!(forgotten, 1);
    assert!(episodes(&scene).is_empty());
}

/// A turn whose row a forget suppressed — the user said "forget that" —
/// is left out of the summary's window: the note does not cite it, nor its
/// answer. (Its text is the working state's to carry: the state is
/// conversation data, as the transcript is, and the note is projected from
/// it — design 11 §3.4.)
#[tokio::test]
async fn a_forgotten_turn_is_not_cited_by_the_episode() {
    let scene = Scene::new(vec![items(&[vim_item(&[1])]), trip_state()]);
    let s = scene.session();
    let (secret, _) = scene.turn(&s, PREF, "Noted.");
    scene.queue.flush_session(&s).await;
    let learned = scene.active();
    assert_eq!(learned.len(), 1, "{learned:?}");
    scene
        .service
        .forget(
            &scene.user_ctx(),
            ForgetInput {
                targets: vec![(learned[0].id, learned[0].revision.clone())],
                reason: None,
            },
        )
        .unwrap();
    four_turns(&scene, &s);

    let report = scene.queue.flush_episodes(&s).await;
    assert_eq!(report.episodes, 1, "{report:?}");
    let note = scene.read(&scene.user_ctx(), episodes(&scene)[0].id);
    assert!(
        note.sources
            .iter()
            .all(|src| src.id != secret.to_string() && src.id != (secret + 1).to_string()),
        "{:?}",
        note.sources
    );
    assert_eq!(note.sources.len(), 8, "the four turns after it");
}

/// A re-run after a crash between the write and the episode cursor's
/// advance writes no second episode — and makes no second call: the state
/// is current.
#[tokio::test]
async fn a_rerun_after_a_crash_writes_no_second_episode() {
    let scene = Scene::new(vec![trip_state(), trip_state()]);
    let s = scene.session();
    let rows = four_turns(&scene, &s);
    scene.queue.set_store_hooks(StoreHooks {
        skip_advance: true,
        ..Default::default()
    });
    let crashed = scene.queue.flush_episodes(&s).await;
    assert_eq!(crashed.episodes, 1, "{crashed:?}");
    assert_eq!(crashed.stopped, Some(FlushStop::StoreError));
    assert_eq!(episode_cursor(&scene, &s), 0);

    scene.queue.set_store_hooks(StoreHooks::default());
    let rerun = scene.queue.flush_episodes(&s).await;
    assert_eq!((rerun.calls, rerun.episodes), (0, 0), "{rerun:?}");
    assert_eq!(rerun.stopped, None);
    assert_eq!(scene.model.calls(), 1);
    assert_eq!(episodes(&scene).len(), 1);
    assert_eq!(episode_cursor(&scene, &s), *rows.last().unwrap());
}

/// L1 re-review, as for learning: a project directory deleted while the
/// summariser refreshes the state is not re-created by the summary's write:
/// the window is skipped — no episode, the directory stays deleted,
/// `skipped_root_gone`, the episode cursor past it.
#[tokio::test]
async fn a_project_deleted_during_the_close_is_not_recreated() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let root = dir.path().canonicalize().unwrap();
    let root_str = root.to_string_lossy().into_owned();
    let model = Scripted::new(vec![trip_state()]);
    let scene = Scene::with(model.clone(), DaemonConfig::default());
    let s = scene.session();
    let mut last = 0;
    for (i, text) in TURNS.iter().enumerate() {
        last = scene
            .turn_in(&s, text, &format!("Answer {i}."), Some(&root_str))
            .1;
    }
    let doomed = root.clone();
    *model.on_call.lock().unwrap() = Some(Arc::new(move |_| {
        std::fs::remove_dir_all(&doomed).unwrap();
    }));

    let report = scene.queue.flush_episodes(&s).await;
    assert_eq!(
        model.calls(),
        1,
        "the root was there when the call went out"
    );
    assert!(!root.exists(), "the summary re-created the deleted project");
    assert_eq!(report.stopped, None, "{report:?}");
    assert_eq!(report.episodes, 0, "{report:?}");
    assert_eq!(report.skipped_root_gone, 1, "{report:?}");
    assert_eq!(episode_cursor(&scene, &s), last);
    assert!(episodes(&scene).is_empty());
}

/// The same crash, with turns that arrived after it: the re-run passes
/// only the rows the summary already covers — its sources run to its
/// window's last row — and summarises the turns after them, instead of
/// passing the whole chunk it reads now.
#[tokio::test]
async fn a_rerun_after_a_crash_still_summarises_the_turns_after_the_summary() {
    let scene = Scene::new(vec![trip_state(), trip_state()]);
    let s = scene.session();
    four_turns(&scene, &s);
    scene.queue.set_store_hooks(StoreHooks {
        skip_advance: true,
        ..Default::default()
    });
    let crashed = scene.queue.flush_episodes(&s).await;
    assert_eq!(crashed.episodes, 1, "{crashed:?}");
    scene.queue.set_store_hooks(StoreHooks::default());
    let mut later = Vec::new();
    for text in TURNS {
        let (q, a) = scene.turn(&s, text, "Later.");
        later.extend([q, a]);
    }

    let rerun = scene.queue.flush_episodes(&s).await;
    assert_eq!(
        (rerun.skipped_summarized, rerun.calls, rerun.episodes),
        (1, 1, 1),
        "{rerun:?}"
    );
    assert_eq!(episodes(&scene).len(), 2);
    assert_eq!(episode_cursor(&scene, &s), *later.last().unwrap());
    let newest = scene.model.prompts().last().cloned().unwrap();
    assert!(newest.contains("Later."), "{newest}");
    assert!(!newest.contains("Answer 0."), "the covered turns: {newest}");
}

/// An `episode` the extractor proposed from the assistant's words is a
/// candidate of origin `extracted_other` — not a conversation summary. It
/// does not count as a summary covering the rows it cites: the
/// conversation still gets its episode note, over every one of its rows.
#[tokio::test]
async fn a_learned_episode_candidate_does_not_hold_back_the_episode_note() {
    let scene = Scene::new(vec![
        items(&[item(
            "Lisbon trip talk",
            "The user and the assistant started planning a Lisbon trip.",
            "episode",
            "assistant",
            &[1, 2],
        )]),
        trip_state(),
    ]);
    let s = scene.session();
    let rows = four_turns(&scene, &s);
    let learned = scene.queue.flush_session(&s).await;
    assert_eq!(learned.candidates, 1, "{learned:?}");
    let candidates = scene.candidates();
    assert_eq!(
        (candidates[0].kind, candidates[0].origin),
        (NoteKind::Episode, Origin::ExtractedOther)
    );

    let report = scene.queue.flush_episodes(&s).await;
    assert_eq!(
        (report.skipped_summarized, report.calls, report.episodes),
        (0, 1, 1),
        "{report:?}"
    );
    let note = scene.read(&scene.user_ctx(), episodes(&scene)[0].id);
    let cited: Vec<i64> = note.sources.iter().map(|s| s.id.parse().unwrap()).collect();
    assert_eq!(cited, rows, "the whole window");
    assert_eq!(episode_cursor(&scene, &s), *rows.last().unwrap());
}

/// A model may not put a key on a summary (owner decision LQ10): its edit
/// is `authority_conflict`, so the extractor is not told the key is known,
/// and the user's own later statement under it is learned as an active note
/// that replaces nothing — not parked as a suggestion to replace the
/// assistant's record of the conversation (review of L10c, R1; a summary an
/// older build let a model key is pinned by the service's
/// `a_key_on_a_summary_is_no_key_for_learning`).
#[tokio::test]
async fn a_model_may_not_key_a_summary_and_the_fact_under_the_key_is_learned_active() {
    let scene = Scene::new(vec![
        items(&[]),
        trip_state(),
        items(&[full_item(serde_json::json!({
            "title": "Editor",
            "content": "The user edits code with vim.",
            "fact_key": "user.editor",
            "rows": [3],
        }))]),
    ]);
    let s = scene.session();
    four_turns(&scene, &s);
    scene.queue.flush_session(&s).await;
    let report = scene.queue.flush_episodes(&s).await;
    assert_eq!(report.episodes, 1, "{report:?}");
    let summary = episodes(&scene)[0].clone();
    let tool = scene
        .service
        .context(
            OWNER,
            WorkspaceHint::None,
            Surface::ModelTool,
            Capabilities::ALL,
            Attribution::default(),
        )
        .unwrap();
    let err = scene
        .service
        .update(
            &tool,
            UpdateInput {
                id: summary.id,
                expected_revision: summary.revision.clone(),
                action: UpdateAction::Edit(Patch {
                    fact_key: Some(Some("user.editor".to_string())),
                    ..Default::default()
                }),
                reason: None,
            },
        )
        .unwrap_err();
    assert_eq!(err.class(), "authority_conflict", "{err}");

    scene.turn(
        &s,
        "By the way, I always edit my code with vim these days, so please remember that about me.",
        "Noted, you edit with vim.",
    );
    let learned = scene.queue.flush_session(&s).await;
    assert_eq!(
        (
            learned.active,
            learned.replaced,
            learned.candidates,
            learned.suggestions
        ),
        (1, 0, 0, 0),
        "{learned:?}"
    );
    let prompt = scene.model.prompts().last().cloned().unwrap();
    assert!(prompt.contains("<known_keys></known_keys>"), "{prompt}");
    let editor = scene
        .active()
        .into_iter()
        .find(|i| i.title == "Editor")
        .expect("the user's statement is an active note");
    assert_eq!(editor.origin, Origin::ExtractedUser);
    assert!(editor.supersedes.is_empty(), "{:?}", editor.supersedes);
    let still = scene.read(&scene.user_ctx(), summary.id);
    assert_eq!(
        (still.origin, still.effective_status),
        (Origin::ExtractedOther, EffectiveStatus::Active),
        "the summary stands"
    );
}

/// An unusable answer leaves the row untouched (design 11 §3.1) and holds
/// the close with a backoff, like learning's window (ruling LR20); the third
/// in a row gives the refresh up, and the close projects what the state
/// covers — here nothing, so no episode, and the episode cursor stays.
#[tokio::test]
async fn three_unusable_answers_give_the_refresh_up() {
    let scene = Scene::new(vec![
        Say::Text("not json".into()),
        Say::Text(r#"{"topic": ""}"#.into()),
        Say::Spent,
    ]);
    let s = scene.session();
    four_turns(&scene, &s);
    for attempt in 1..=2 {
        let held = scene.queue.flush_episodes(&s).await;
        assert_eq!(
            held.stopped,
            Some(FlushStop::BadOutput),
            "{attempt}: {held:?}"
        );
        assert_eq!(state_on_row(&scene, &s), None);
    }
    let given_up = scene.queue.flush_episodes(&s).await;
    assert_eq!(given_up.stopped, None, "{given_up:?}");
    assert_eq!(given_up.skipped_unusable, 1);
    let row = session_row(&scene, &s);
    assert_eq!(
        (
            row.working_state.as_str(),
            row.working_state_version,
            row.working_state_through
        ),
        ("", 0, 0),
        "the row untouched"
    );
    assert_eq!(episode_cursor(&scene, &s), 0);
    assert!(episodes(&scene).is_empty());
    assert_eq!(scene.model.calls(), 3);
}

/// A refresh given up still projects what the state already covers: a
/// state covering four turns, then a fifth whose refresh the model answers
/// unusably three times — the episode covers the four.
#[tokio::test]
async fn a_refresh_given_up_projects_what_the_state_covers() {
    let scene = Scene::new(vec![
        Say::Text("not json".into()),
        Say::Text("not json".into()),
        Say::Text("not json".into()),
    ]);
    let s = scene.session();
    let rows = four_turns(&scene, &s);
    let covered = *rows.last().unwrap();
    assert!(
        scene
            .repo()
            .update_working_state(&s, 0, &serde_json::to_string(&trip()).unwrap(), covered)
            .unwrap()
    );
    scene.turn(&s, "And the return flight?", "On the 5th.");
    for _ in 1..=2 {
        let held = scene.queue.flush_episodes(&s).await;
        assert_eq!(held.stopped, Some(FlushStop::BadOutput), "{held:?}");
        assert!(episodes(&scene).is_empty());
    }
    let given_up = scene.queue.flush_episodes(&s).await;
    assert_eq!(
        (
            given_up.skipped_unusable,
            given_up.episodes,
            given_up.stopped
        ),
        (1, 1, None),
        "{given_up:?}"
    );
    assert_eq!(episode_cursor(&scene, &s), covered);
    let note = scene.read(&scene.user_ctx(), episodes(&scene)[0].id);
    let cited: Vec<i64> = note.sources.iter().map(|s| s.id.parse().unwrap()).collect();
    assert_eq!(cited, rows, "the four turns the state covers");
}

/// The L8 fix slice's P1, for the close's refresh: an answer stopped at the
/// output ceiling is reported as cut there (the writer's reason, design 11
/// §3.1), and holds the close like any unusable answer.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_refresh_cut_at_max_tokens_says_it_was_cut() {
    let scene = Scene::new(vec![Say::Cut(r#"{"topic": "Lisbon trip plan"#.into())]);
    let s = scene.session();
    four_turns(&scene, &s);
    let held = scene.queue.flush_episodes(&s).await;
    assert_eq!(held.stopped, Some(FlushStop::BadOutput), "{held:?}");
    assert_eq!(state_on_row(&scene, &s), None);
    assert_eq!(episode_cursor(&scene, &s), 0);
    logs_assert(|lines: &[&str]| {
        match lines
            .iter()
            .filter(|l| l.contains("the working state's refresh was unusable"))
            .collect::<Vec<_>>()
            .as_slice()
        {
            [one] if one.contains("cut at max_tokens (20)") => Ok(()),
            other => Err(format!("expected one line naming the cut, saw {other:?}")),
        }
    });
}

/// LQ13 keeps the L8 fix slice's P1 for the close: an answer stopped at the
/// output ceiling is not read out of the prose the model was still writing
/// after a complete state; it is reported cut, and nothing is written.
#[tokio::test]
async fn a_refresh_cut_in_the_prose_after_it_is_not_rescued() {
    let complete = serde_json::to_string(&trip()).unwrap();
    let scene = Scene::new(vec![Say::Cut(format!("{complete}\n\nNote: the conver"))]);
    let s = scene.session();
    four_turns(&scene, &s);
    let held = scene.queue.flush_episodes(&s).await;
    assert_eq!(held.stopped, Some(FlushStop::BadOutput), "{held:?}");
    let [cut] = &held.unusable[..] else {
        panic!("one unusable answer: {held:?}");
    };
    assert!(
        cut.reason.starts_with("cut at max_tokens (20)"),
        "{}",
        cut.reason
    );
    assert_eq!(state_on_row(&scene, &s), None);
    assert_eq!(episode_cursor(&scene, &s), 0);
    assert!(episodes(&scene).is_empty());
}

/// The L8 fix slice's P2, for the close: the report keeps each unusable
/// answer — its reason, length, finish and excerpt — and nothing else does.
#[tokio::test]
async fn an_unusable_refresh_is_kept_in_the_report() {
    let scene = Scene::new(vec![
        Say::Text("Here is what we talked about: Lisbon.".into()),
        Say::Spent,
    ]);
    let s = scene.session();
    four_turns(&scene, &s);
    let first = scene.queue.flush_episodes(&s).await;
    let [prose] = &first.unusable[..] else {
        panic!("one unusable answer: {first:?}");
    };
    assert_eq!(prose.reason, "the answer is not JSON");
    assert_eq!(prose.chars, 37);
    assert_eq!(prose.finish_reason, Some("stop"));
    assert_eq!(
        (prose.head.as_str(), prose.tail.as_str()),
        ("Here is what we talked about: Lisbon.", "")
    );
    let second = scene.queue.flush_episodes(&s).await;
    let [empty] = &second.unusable[..] else {
        panic!("one unusable answer: {second:?}");
    };
    assert_eq!((empty.chars, empty.finish_reason), (0, None));
}

// ── The gates (design 11 §3.1, §8; CQ-2) ──────────────────────────────────

/// The summariser's daily ceiling (`summary_max_daily_cost_usd`) is read
/// before the close's call: once it is spent, a stale state is not
/// refreshed — no call, the row as it was, the conversation still due.
#[tokio::test]
async fn the_summariser_daily_ceiling_holds_the_close() {
    let mut config = DaemonConfig::default();
    config.orchestrator.costs.summary_max_daily_cost_usd = 0.0;
    let scene = Scene::with(Scripted::new(vec![trip_state()]), config);
    let s = scene.session();
    let (_, first) = scene.turn(&s, TURNS[0], "Answer 0.");
    let spent = scene.queue.flush_episodes(&s).await;
    assert_eq!(
        (spent.calls, spent.stopped),
        (1, None),
        "nothing spent yet: {spent:?}"
    );

    scene.turn(&s, TURNS[1], "Answer 1.");
    let capped = scene.queue.flush_episodes(&s).await;
    assert_eq!(capped.stopped, Some(FlushStop::Cap), "{capped:?}");
    assert_eq!(scene.model.calls(), 1);
    assert_eq!(session_row(&scene, &s).working_state_through, first);
    assert!(scene.queue.is_episode_due(&s), "still queued");
}

/// The close's one call is the summariser's (design 11 §3.1, §8): it counts
/// under the summariser's ceiling, not under the extraction's daily caps —
/// with those spent, learning stops `Cap` and the close still refreshes and
/// writes the summary.
#[tokio::test]
async fn the_close_is_not_held_by_the_extraction_caps() {
    let mut config = DaemonConfig::default();
    config.orchestrator.costs.extract_max_calls_per_day = 1;
    let scene = Scene::with(Scripted::new(vec![trip_state()]), config);
    openalpaca_storage::LlmUsageRepository::new(&scene.db)
        .upsert_daily_usage(&usage(super::extractor::AGENT_LABEL, 1, 0.0))
        .unwrap();
    let s = scene.session();
    four_turns(&scene, &s);
    let learning = scene.queue.flush_session(&s).await;
    assert_eq!(learning.stopped, Some(FlushStop::Cap), "{learning:?}");

    let closed = scene.queue.flush_episodes(&s).await;
    assert_eq!((closed.calls, closed.episodes), (1, 1), "{closed:?}");
    assert_eq!(closed.stopped, None);
}

// ── Sharing the model with chat (§1.7) ────────────────────────────────────

/// On a local model a chat turn that starts cancels the close's call,
/// exactly as it cancels a learning chunk's: nothing written, still due.
#[tokio::test]
async fn a_local_model_episode_yields_to_a_chat_turn() {
    let model = Arc::new(Scripted {
        keyless: true,
        ..Arc::into_inner(Scripted::new(vec![Say::Hang])).unwrap()
    });
    let router = router_for(model.clone(), ProviderType::Ollama, "local-model");
    let scene = Scene::with_router(model, router, DaemonConfig::default());
    scene.no_quiet_period();
    let s = scene.session();
    four_turns(&scene, &s);

    let queue = scene.queue.clone();
    let session = s.clone();
    let flush = tokio::spawn(async move { queue.flush_episodes_automatic(&session).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while scene.model.calls() == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the close's call never went out");
    let _chat = scene.queue.turn_started(uuid::Uuid::new_v4(), LANE, true);
    let report = tokio::time::timeout(std::time::Duration::from_secs(10), flush)
        .await
        .expect("the close did not yield")
        .unwrap();
    assert_eq!(report.stopped, Some(FlushStop::Yielded), "{report:?}");
    assert_eq!(state_on_row(&scene, &s), None);
    assert!(episodes(&scene).is_empty());
    assert_eq!(episode_cursor(&scene, &s), 0);
    assert!(scene.queue.is_episode_due(&s));
}

// ── The triggers (§13.3, *When*) ──────────────────────────────────────────

/// A turn that ran through the gateway's hook, with its rows persisted.
fn fed_turn(scene: &Scene, s: &str, text: &str) {
    scene.turn(s, text, "ok");
    scene.finish_turn(s, true);
}

/// "At the end of a conversation's quiet period of at least 30 minutes":
/// the 120-second learning trigger does not make a summary due, 29 minutes
/// 59 seconds do not either, 30 minutes do — and only once the conversation
/// has four new turns of the user's since its last summary.
#[tokio::test(start_paused = true)]
async fn a_30_minute_quiet_period_makes_the_conversation_due_for_its_episode() {
    let scene = Scene::new(vec![]);
    let s = scene.session();
    for text in TURNS {
        fed_turn(&scene, &s, text);
    }
    tokio::time::advance(std::time::Duration::from_secs(121)).await;
    scene.queue.sweep().await;
    assert!(scene.queue.is_due(&s), "learning's idle trigger");
    assert!(
        !scene.queue.is_episode_due(&s),
        "two minutes is not the summary's quiet"
    );
    tokio::time::advance(std::time::Duration::from_secs(30 * 60 - 121 - 1)).await;
    scene.queue.sweep().await;
    assert!(!scene.queue.is_episode_due(&s), "29 min 59 s");
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    scene.queue.sweep().await;
    assert!(scene.queue.is_episode_due(&s), "30 minutes");

    let few = scene.session_on("user1:cli");
    let guard = scene
        .queue
        .turn_started(uuid::Uuid::new_v4(), "user1:cli", false);
    guard.attach_session(&few);
    drop(guard);
    tokio::time::advance(std::time::Duration::from_secs(31 * 60)).await;
    scene.queue.sweep().await;
    assert!(
        !scene.queue.is_episode_due(&few),
        "an assistant-written turn is not the user's"
    );
}

/// Nor does learning's other trigger: ten of the user's turns make the
/// conversation due for learning at once, and not for its episode note.
#[tokio::test]
async fn ten_turns_make_learning_due_and_not_the_episode_note() {
    let scene = Scene::new(vec![]);
    let s = scene.session();
    for i in 0..10 {
        fed_turn(&scene, &s, &format!("Turn {i} about the trip."));
    }
    assert!(scene.queue.is_due(&s), "learning's ten-turn trigger");
    assert!(
        !scene.queue.is_episode_due(&s),
        "ten turns are not the summary's trigger"
    );
}

/// The daemon's path for the quiet period, end to end: the sweeper learns
/// the conversation first, then writes its one episode.
#[tokio::test(start_paused = true)]
async fn the_sweeper_writes_one_episode_after_the_quiet_period() {
    let model = Scripted::new(vec![items(&[]), trip_state()]);
    let router = Arc::new(LlmRouter::single_provider(
        model.clone(),
        ProviderType::Anthropic,
        RECORDED_MODEL.to_string(),
    ));
    let scene = Scene::build(model, router, DaemonConfig::default(), QueueMode::Automatic);
    scene.no_quiet_period();
    let s = scene.session();
    for text in TURNS {
        fed_turn(&scene, &s, text);
    }
    tokio::time::advance(EPISODE_QUIET + std::time::Duration::from_secs(1)).await;
    scene.queue.sweep().await;
    scene.queue.wait_idle().await;
    assert_eq!(scene.model.calls(), 2, "learning, then the close");
    assert_eq!(episodes(&scene).len(), 1);
    assert!(!scene.queue.is_episode_due(&s));

    scene.queue.sweep().await;
    scene.queue.wait_idle().await;
    assert_eq!(episodes(&scene).len(), 1, "one episode, not one per sweep");
}

/// "On a conversation switch": "New chat" archives the conversation, and the
/// next sweep writes its episode — before any quiet period.
#[tokio::test]
async fn a_switch_writes_the_episode_of_the_conversation_left() {
    let model = Scripted::new(vec![items(&[]), trip_state()]);
    let router = Arc::new(LlmRouter::single_provider(
        model.clone(),
        ProviderType::Anthropic,
        RECORDED_MODEL.to_string(),
    ));
    let scene = Scene::build(model, router, DaemonConfig::default(), QueueMode::Automatic);
    scene.no_quiet_period();
    let s = scene.session();
    for text in TURNS {
        fed_turn(&scene, &s, text);
    }
    scene.queue.sweep().await;
    scene.queue.wait_idle().await;
    assert!(episodes(&scene).is_empty(), "still the active conversation");

    scene.session(); // "New chat" archives `s`.
    scene.queue.sweep().await;
    scene.queue.wait_idle().await;
    assert_eq!(episodes(&scene).len(), 1);
    assert!(!scene.queue.is_episode_due(&s));
}

/// Design 11 §2 (the panel's graft): a switch closes a short conversation
/// too — under four of the user's turns, its state is brought up to date for
/// the next conversation to read, and no episode is written.
#[tokio::test]
async fn a_switch_gives_a_short_conversation_a_current_state() {
    let model = Scripted::new(vec![items(&[]), trip_state()]);
    let router = Arc::new(LlmRouter::single_provider(
        model.clone(),
        ProviderType::Anthropic,
        RECORDED_MODEL.to_string(),
    ));
    let scene = Scene::build(model, router, DaemonConfig::default(), QueueMode::Automatic);
    scene.no_quiet_period();
    let s = scene.session();
    for text in &TURNS[..2] {
        fed_turn(&scene, &s, text);
    }
    scene.session(); // "New chat" archives `s`.
    scene.queue.sweep().await;
    scene.queue.wait_idle().await;
    assert_eq!(scene.model.calls(), 2, "learning, then the close");
    assert_eq!(state_on_row(&scene, &s), Some(trip()));
    assert!(episodes(&scene).is_empty());
    assert!(!scene.queue.is_episode_due(&s));
}

/// The switch's other half, in the gateway's hook: a lane's turn landing in
/// another session makes the one it left due for its episode at once.
#[tokio::test]
async fn a_turn_in_another_session_makes_the_one_it_left_due_for_its_episode() {
    let scene = Scene::new(vec![]);
    let left = scene.session();
    for text in TURNS {
        fed_turn(&scene, &left, text);
    }
    assert!(!scene.queue.is_episode_due(&left));
    let next = scene.session();
    let turn = scene.queue.turn_started(uuid::Uuid::new_v4(), LANE, true);
    turn.attach_session(&next);
    assert!(scene.queue.is_episode_due(&left));
    assert!(!scene.queue.is_episode_due(&next));
}

/// "In the shutdown drain": the drain learns what is pending and then writes
/// the conversation's episode, ignoring the quiet rules — the server has
/// already stopped.
#[tokio::test]
async fn the_stop_drain_writes_the_episode() {
    let scene = Scene::new(vec![items(&[]), trip_state()]);
    let s = scene.session();
    for text in TURNS {
        fed_turn(&scene, &s, text);
    }
    assert!(
        scene.queue.drain(SHUTDOWN_DRAIN).await,
        "the drain finished"
    );
    assert_eq!(scene.model.calls(), 2);
    assert_eq!(episodes(&scene).len(), 1);
    assert!(!scene.queue.is_episode_due(&s));
}

/// The drain's durable half: at boot, a conversation with four of the
/// user's turns past its episode cursor is due for its episode, one with
/// three is not.
#[tokio::test]
async fn boot_catchup_finds_conversations_to_summarize() {
    let scene = Scene::new(vec![]);
    let four = scene.session();
    four_turns(&scene, &four);
    let three = scene.session_on("user1:cli");
    for text in &TURNS[..3] {
        scene.turn(&three, text, "ok");
    }
    scene.queue.catch_up().await;
    assert!(scene.queue.is_episode_due(&four));
    assert!(!scene.queue.is_episode_due(&three));
}

/// In `Manual` mode no trigger writes an episode on its own — the harness
/// alone decides, through `flush_episodes`.
#[tokio::test]
async fn a_manual_queue_writes_no_episode_on_its_own() {
    let scene = Scene::new(vec![trip_state()]);
    scene.no_quiet_period();
    let s = scene.session();
    for text in TURNS {
        fed_turn(&scene, &s, text);
    }
    scene.session(); // a switch
    scene.queue.sweep().await;
    scene.queue.catch_up().await;
    scene.queue.wait_idle().await;
    assert_eq!(scene.model.calls(), 0);
    assert!(scene.queue.is_episode_due(&s));
    scene.queue.flush_episodes(&s).await;
    assert_eq!(episodes(&scene).len(), 1);
}
