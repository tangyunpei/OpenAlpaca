//! Conversation summaries (design 09 §13.3, owner decision MD-16, slice
//! L10c): the writer, its triggers and its gates, over the learning tests'
//! scene — a persisted window, a scripted model, a manual queue unless a
//! test builds an automatic one.

use super::*;
use crate::daemon_config::RecallMode;
use crate::memory::learning::episode::EPISODE_LABEL;
use crate::memory::service::{RecallBudget, SearchInput, TimelineAnchor, TimelineInput};

/// The scripted model's summary.
fn summary(topic: &str, discussed: &str, decided: &str, open: &str) -> Say {
    Say::Text(
        serde_json::json!({
            "topic": topic,
            "discussed": discussed,
            "decided": decided,
            "open": open,
        })
        .to_string(),
    )
}

fn trip_summary() -> Say {
    summary(
        "Lisbon trip planning",
        "Flights to Lisbon and a hotel near Alfama.",
        "Fly out on Friday 2 October.",
        "Which museum to visit.",
    )
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

fn episode_calls_today(scene: &Scene) -> i64 {
    openalpaca_storage::LlmUsageRepository::new(&scene.db)
        .usage_today(EPISODE_LABEL)
        .unwrap()
        .0
}

fn usage(label: &str, requests: i32, cost: f64) -> openalpaca_storage::LlmUsageDaily {
    openalpaca_storage::LlmUsageDaily {
        agent_id: label.to_string(),
        ..usage_row(chrono::Utc::now().date_naive(), requests, cost)
    }
}

// ── The note (§13.3, *What*, *Authority and visibility*) ──────────────────

/// "Only when at least 4 user turns have accumulated since the session's
/// last episode": three turns call no model and write nothing; the fourth
/// makes one call and one episode, and the episode cursor moves past it.
#[tokio::test]
async fn no_episode_before_four_user_turns() {
    let scene = Scene::new(vec![trip_summary()]);
    let s = scene.session();
    for (i, text) in TURNS[..3].iter().enumerate() {
        scene.turn(&s, text, &format!("Answer {i}."));
    }
    let early = scene.queue.flush_episodes(&s).await;
    assert_eq!(early.stopped, None, "{early:?}");
    assert_eq!((early.calls, early.episodes), (0, 0), "{early:?}");
    assert_eq!(scene.model.calls(), 0);
    assert_eq!(episode_cursor(&scene, &s), 0);

    let (_, last) = scene.turn(&s, TURNS[3], "Answer 3.");
    let report = scene.queue.flush_episodes(&s).await;
    assert_eq!((report.calls, report.episodes), (1, 1), "{report:?}");
    assert_eq!(report.stopped, None);
    assert_eq!(episodes(&scene).len(), 1);
    assert_eq!(episode_cursor(&scene, &s), last);

    let again = scene.queue.flush_episodes(&s).await;
    assert_eq!((again.calls, again.episodes), (0, 0), "{again:?}");
}

/// §13.3's note: `kind: episode`, the title `Conversation on <YYYY-MM-DD>:
/// <topic>`, what was discussed, decided and left open in at most 1 KiB of
/// prose, `observed_at` the last turn's time, the window's rows as sources,
/// no `fact_key`, no aliases, no `supersedes`; origin `extracted_other`,
/// status `active`. The call is one request with no reasoning, billed under
/// the summaries' own label, reading the window's rows and nothing else.
#[tokio::test]
async fn an_episode_is_one_active_extracted_other_note_with_the_designs_fields() {
    let scene = Scene::new(vec![trip_summary()]);
    let s = scene.session();
    let rows = four_turns(&scene, &s);
    let report = scene.queue.flush_episodes(&s).await;
    assert_eq!(report.episodes, 1, "{report:?}");

    let listed = episodes(&scene);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].origin, Origin::ExtractedOther);
    assert_eq!(listed[0].status, NoteStatus::Active);
    let note = scene.read(&scene.user_ctx(), listed[0].id);
    assert_eq!(
        note.title,
        "Conversation on 2026-09-20: Lisbon trip planning"
    );
    let fm = &note.frontmatter;
    assert_eq!(fm["kind"], "episode");
    assert_eq!(fm["observed_at"], "2026-09-20T10:03:30Z");
    assert_eq!(fm["fact_key"], serde_json::Value::Null);
    assert_eq!(fm["aliases"], serde_json::json!([]));
    assert_eq!(fm["supersedes"], serde_json::json!([]));
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
    let prompt = &scene.model.prompts()[0];
    for text in TURNS {
        assert!(prompt.contains(text), "{prompt}");
    }
    assert!(prompt.contains("<today>2026-09-20</today>"), "{prompt}");
    assert_eq!(episode_calls_today(&scene), 1);
}

/// A long conversation's episode still cites at most eight rows (design 03
/// §7a's cap on `sources`), spread from its first row to its last.
#[tokio::test]
async fn a_long_conversation_cites_eight_rows_first_to_last() {
    let scene = Scene::new(vec![trip_summary()]);
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
    let scene = Scene::new(vec![trip_summary()]);
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
    let scene = Scene::new(vec![trip_summary()]);
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
/// is left out of the summary: its text never reaches the model and the
/// note does not cite it (design 03 §6's hard guarantee: no replay
/// resurrects a forgotten record).
#[tokio::test]
async fn a_forgotten_turn_is_left_out_of_the_episode() {
    let scene = Scene::new(vec![items(&[vim_item(&[1])]), trip_summary()]);
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
    let prompt = scene.model.prompts().last().cloned().unwrap();
    assert!(!prompt.contains("vim"), "{prompt}");
    let note = scene.read(&scene.user_ctx(), episodes(&scene)[0].id);
    assert!(
        note.sources.iter().all(|src| src.id != secret.to_string()),
        "{:?}",
        note.sources
    );
}

/// A re-run after a crash between the write and the episode cursor's
/// advance writes no second episode — and makes no second call.
#[tokio::test]
async fn a_rerun_after_a_crash_writes_no_second_episode() {
    let scene = Scene::new(vec![trip_summary(), trip_summary()]);
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

/// An unusable answer holds the window with a backoff, like learning's
/// (ruling LR20); the third in a row moves the cursor past the chunk with
/// no episode.
#[tokio::test]
async fn three_unusable_answers_skip_the_window() {
    let scene = Scene::new(vec![
        Say::Text("not json".into()),
        Say::Text(r#"{"topic": ""}"#.into()),
        Say::Spent,
    ]);
    let s = scene.session();
    let rows = four_turns(&scene, &s);
    for attempt in 1..=2 {
        let held = scene.queue.flush_episodes(&s).await;
        assert_eq!(
            held.stopped,
            Some(FlushStop::BadOutput),
            "{attempt}: {held:?}"
        );
        assert_eq!(episode_cursor(&scene, &s), 0);
    }
    let skipped = scene.queue.flush_episodes(&s).await;
    assert_eq!(skipped.stopped, None, "{skipped:?}");
    assert_eq!(skipped.skipped_unusable, 1);
    assert_eq!(episode_cursor(&scene, &s), *rows.last().unwrap());
    assert!(episodes(&scene).is_empty());
    assert_eq!(scene.model.calls(), 3);
}

// ── The gates (§13.3, *Cost*; §1.8) ───────────────────────────────────────

/// `episode_max_per_day` binds on today's UTC episode calls in
/// `llm_usage_daily`: no call, the cursor stays, the conversation stays due.
/// `0` writes none.
#[tokio::test]
async fn the_episode_daily_cap_holds() {
    let mut config = DaemonConfig::default();
    config.orchestrator.learning.episode_max_per_day = 1;
    let scene = Scene::with(Scripted::new(vec![trip_summary()]), config.clone());
    openalpaca_storage::LlmUsageRepository::new(&scene.db)
        .upsert_daily_usage(&usage(EPISODE_LABEL, 1, 0.0))
        .unwrap();
    let s = scene.session();
    four_turns(&scene, &s);
    let capped = scene.queue.flush_episodes(&s).await;
    assert_eq!(capped.stopped, Some(FlushStop::Cap), "{capped:?}");
    assert_eq!(scene.model.calls(), 0);
    assert_eq!(episode_cursor(&scene, &s), 0);
    assert!(scene.queue.is_episode_due(&s), "still queued");

    config.orchestrator.learning.episode_max_per_day = 0;
    scene.config.store(Arc::new(config.clone()));
    let off = scene.queue.flush_episodes(&s).await;
    assert_eq!(off.stopped, Some(FlushStop::Disabled), "{off:?}");
    assert_eq!(scene.model.calls(), 0);

    config.orchestrator.learning.episode_max_per_day = 2;
    scene.config.store(Arc::new(config));
    let written = scene.queue.flush_episodes(&s).await;
    assert_eq!(written.episodes, 1, "{written:?}");
}

/// The summaries count under the extraction's own daily caps (§13.3: "the
/// same durable daily gates as extraction"): chat learning's calls and the
/// summaries' together reach `extract_max_calls_per_day`, and then neither
/// a summary nor a learning flush makes a call.
#[tokio::test]
async fn episodes_and_learning_share_the_extraction_daily_caps() {
    let mut config = DaemonConfig::default();
    config.orchestrator.costs.extract_max_calls_per_day = 2;
    let scene = Scene::with(Scripted::new(vec![trip_summary()]), config);
    let usage_repo = openalpaca_storage::LlmUsageRepository::new(&scene.db);
    usage_repo
        .upsert_daily_usage(&usage(super::extractor::AGENT_LABEL, 1, 0.0))
        .unwrap();
    let s = scene.session();
    four_turns(&scene, &s);
    let first = scene.queue.flush_episodes(&s).await;
    assert_eq!(
        first.episodes, 1,
        "one call left under the shared cap: {first:?}"
    );

    let other = scene.session_on("user1:cli");
    scene.turn(&other, PREF, "Noted.");
    let learning = scene.queue.flush_session(&other).await;
    assert_eq!(learning.stopped, Some(FlushStop::Cap), "{learning:?}");
    for text in TURNS {
        scene.turn(&s, text, "ok");
    }
    let second = scene.queue.flush_episodes(&s).await;
    assert_eq!(second.stopped, Some(FlushStop::Cap), "{second:?}");
    assert_eq!(scene.model.calls(), 1);

    let mut cost_capped = DaemonConfig::default();
    cost_capped.orchestrator.costs.extract_max_daily_cost_usd = 0.10;
    scene.config.store(Arc::new(cost_capped));
    usage_repo
        .upsert_daily_usage(&usage(EPISODE_LABEL, 0, 0.20))
        .unwrap();
    let by_cost = scene.queue.flush_episodes(&s).await;
    assert_eq!(by_cost.stopped, Some(FlushStop::Cap), "{by_cost:?}");
}

// ── Sharing the model with chat (§1.7) ────────────────────────────────────

/// On a local model a chat turn that starts cancels the summary's call,
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
    .expect("the summary's call never went out");
    let _chat = scene.queue.turn_started(uuid::Uuid::new_v4(), LANE, true);
    let report = tokio::time::timeout(std::time::Duration::from_secs(10), flush)
        .await
        .expect("the summary did not yield")
        .unwrap();
    assert_eq!(report.stopped, Some(FlushStop::Yielded), "{report:?}");
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

/// The daemon's path for the quiet period, end to end: the sweeper learns
/// the conversation first, then writes its one episode.
#[tokio::test(start_paused = true)]
async fn the_sweeper_writes_one_episode_after_the_quiet_period() {
    let model = Scripted::new(vec![items(&[]), trip_summary()]);
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
    assert_eq!(scene.model.calls(), 2, "learning, then the summary");
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
    let model = Scripted::new(vec![items(&[]), trip_summary()]);
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
    let scene = Scene::new(vec![items(&[]), trip_summary()]);
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
    let scene = Scene::new(vec![trip_summary()]);
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

/// The prompt is the committed fixture: one fixed window renders byte for
/// byte to the golden file beside it.
#[test]
fn the_episode_prompt_matches_its_fixture() {
    let row = |id: i64, role: &str, content: &str, request: &str, at: &str| {
        openalpaca_storage::LearningRow {
            id,
            role: role.to_string(),
            content: content.to_string(),
            source: Some("gui".to_string()),
            created_at: at.to_string(),
            request_id: Some(request.to_string()),
            workspace_root: None,
        }
    };
    let rows = [
        row(
            1,
            "user",
            "Let's plan my Lisbon trip.",
            "a",
            "2026-09-20 10:00:00",
        ),
        row(2, "assistant", "Happy to help.", "a", "2026-09-20 10:00:20"),
        row(
            3,
            "user",
            "Book the Friday flight.",
            "b",
            "2026-09-20 10:01:00",
        ),
        row(
            4,
            "assistant",
            "Booked: Friday 2 October.",
            "b",
            "2026-09-20 10:01:30",
        ),
    ];
    let window::NextChunk::Chunk(chunk) = window::next_chunk(
        &rows,
        &[],
        &std::collections::HashSet::new(),
        24 * 1024,
        400,
    ) else {
        panic!("a chunk");
    };
    let (transcript, _) = chunk.transcript();
    let (system, user) = super::episode::render_prompt(&chunk.today(), &transcript);
    let rendered = format!("{system}\n=== rendered user message ===\n{user}\n");
    assert!(
        !rendered.contains("{{"),
        "a placeholder was left: {rendered}"
    );
    assert_eq!(rendered, include_str!("../episode_prompt.golden.txt"));
}
