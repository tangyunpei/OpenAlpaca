//! Learning v2's queue, window and extractor (design 09 §1, §8 L1).
//!
//! The extraction tests that used to drive the per-turn extractor
//! (`orchestrator/extraction.rs`) live here now, re-expressed over a
//! persisted window: the MD-2, cap, threshold, no-owner, cancellation,
//! source, dedup and thinking assertions keep their meaning.

use super::queue::StoreHooks;
use super::*;
use crate::daemon_config::DaemonConfig;
use crate::memory::service::{
    Attribution, Capabilities, ForgetInput, ListInput, ListItem, MemoryAccessContext,
    MemoryService, ReadInput, Surface, WorkspaceHint,
};
use crate::test_util::{HomeStoreGuard, RECORDED_MODEL};
use arc_swap::ArcSwap;
use openalpaca_llm::{
    ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, LlmRouter, ProviderType, Usage,
};
use openalpaca_storage::memory_files::{NoteKind, NoteStatus, Origin, SourceType};
use openalpaca_storage::memory_index::MemoryIndex;
use openalpaca_storage::{
    ConversationMessage, ConversationRepository, Database, IdentityRepository,
    LearningCursorRepository,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const OWNER: &str = "user1";
const LANE: &str = "user1:gui";

// ── A scripted model ─────────────────────────────────────────────────────

/// One answer of the scripted model.
enum Say {
    Text(String),
    /// Nothing, at the `max_tokens` cap: the router's `EmptyCompletion`.
    Spent,
    Fail(LlmError),
    /// Never answers.
    Hang,
}

/// A hook the scripted model runs when a call arrives: the call's number.
type OnCall = Arc<dyn Fn(usize) + Send + Sync>;

/// A provider that answers from a script, one entry per call, and keeps
/// every request. When the script runs out it answers `{"items": []}`.
struct Scripted {
    script: Mutex<VecDeque<Say>>,
    seen: Mutex<Vec<ChatRequest>>,
    keyless: bool,
    /// Runs when a call arrives, before it is answered.
    on_call: Mutex<Option<OnCall>>,
}

impl Scripted {
    fn new(script: Vec<Say>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            seen: Mutex::new(Vec::new()),
            keyless: false,
            on_call: Mutex::new(None),
        })
    }

    fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    /// The transcript of every call, in order.
    fn prompts(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|r| {
                r.messages
                    .iter()
                    .filter(|m| matches!(m.role, openalpaca_llm::Role::User))
                    .map(|m| m.content.clone())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect()
    }

    fn first_request(&self) -> ChatRequest {
        self.seen.lock().unwrap()[0].clone()
    }
}

#[async_trait::async_trait]
impl LlmProvider for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn requires_key(&self) -> bool {
        !self.keyless
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let n = {
            let mut seen = self.seen.lock().unwrap();
            seen.push(request.clone());
            seen.len()
        };
        let hook = self.on_call.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(n);
        }
        let say = self.script.lock().unwrap().pop_front();
        let (content, finish_reason) = match say {
            None => (r#"{"items": []}"#.to_string(), FinishReason::Stop),
            Some(Say::Text(t)) => (t, FinishReason::Stop),
            Some(Say::Spent) => (String::new(), FinishReason::MaxTokens),
            Some(Say::Fail(e)) => return Err(e),
            Some(Say::Hang) => std::future::pending().await,
        };
        Ok(ChatResponse {
            content,
            tool_calls: Vec::new(),
            model: request
                .model
                .clone()
                .unwrap_or_else(|| RECORDED_MODEL.into()),
            usage: Usage {
                input_tokens: 100,
                output_tokens: 20,
                ..Default::default()
            },
            finish_reason,
            thinking: None,
            parts: None,
        })
    }
}

/// One item of the model's answer.
fn item(title: &str, content: &str, kind: &str, stated_by: &str, rows: &[u32]) -> String {
    serde_json::json!({
        "title": title,
        "content": content,
        "kind": kind,
        "stated_by": stated_by,
        "confidence": 0.9,
        "rows": rows,
    })
    .to_string()
}

fn items(list: &[String]) -> Say {
    Say::Text(format!(r#"{{"items": [{}]}}"#, list.join(",")))
}

// ── The scene ────────────────────────────────────────────────────────────

/// A temp home store, a database with the owner's `global_user` row, a
/// service over an in-memory index, a scripted router and a manual queue.
struct Scene {
    _env: HomeStoreGuard,
    root: tempfile::TempDir,
    db: Database,
    service: Arc<MemoryService>,
    model: Arc<Scripted>,
    config: Arc<ArcSwap<DaemonConfig>>,
    queue: Arc<LearningQueue>,
}

impl Scene {
    fn new(script: Vec<Say>) -> Self {
        Self::with(Scripted::new(script), DaemonConfig::default())
    }

    fn with(model: Arc<Scripted>, config: DaemonConfig) -> Self {
        let router = Arc::new(LlmRouter::single_provider(
            model.clone(),
            ProviderType::Anthropic,
            RECORDED_MODEL.to_string(),
        ));
        Self::with_router(model, router, config)
    }

    fn with_router(model: Arc<Scripted>, router: Arc<LlmRouter>, config: DaemonConfig) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let env = HomeStoreGuard::set(&home.canonicalize().unwrap());
        let db = Database::open(&root.path().join("t.db")).unwrap();
        IdentityRepository::new(&db)
            .create_global_user(OWNER, None)
            .unwrap();
        let service = MemoryService::new(Some(db.clone()), MemoryIndex::open_in_memory().unwrap());
        service.rebuild_index().unwrap();
        let service = Arc::new(service);
        let config = Arc::new(ArcSwap::from_pointee(config));
        let queue = LearningQueue::new(
            LearningDeps {
                db: db.clone(),
                router,
                daemon_config: config.clone(),
                memory_service: service.clone(),
            },
            QueueMode::Manual,
        );
        Self {
            _env: env,
            root,
            db,
            service,
            model,
            config,
            queue,
        }
    }

    fn repo(&self) -> ConversationRepository<'_> {
        ConversationRepository::new(&self.db)
    }

    fn session(&self) -> String {
        self.session_on(LANE)
    }

    fn session_on(&self, lane: &str) -> String {
        self.repo()
            .create_session(lane, "gui", None, None)
            .unwrap()
            .id
    }

    /// A row, as a writer would store it.
    fn row(
        &self,
        session: &str,
        role: &str,
        content: &str,
        source: &str,
        request_id: Option<&str>,
        root: Option<&str>,
    ) -> i64 {
        self.repo()
            .insert(&ConversationMessage {
                lane_key: LANE.to_string(),
                role: role.to_string(),
                content: content.to_string(),
                source: Some(source.to_string()),
                session_id: Some(session.to_string()),
                request_id: request_id.map(str::to_string),
                workspace_root: root.map(str::to_string),
                ..Default::default()
            })
            .unwrap()
    }

    /// One turn — the user's row and its answer — as the gateway writes it.
    fn turn(&self, session: &str, user: &str, answer: &str) -> (i64, i64) {
        self.turn_in(session, user, answer, None)
    }

    fn turn_in(&self, session: &str, user: &str, answer: &str, root: Option<&str>) -> (i64, i64) {
        let request = uuid::Uuid::new_v4().to_string();
        let q = self.row(session, "user", user, "gui", Some(&request), root);
        let a = self.row(session, "assistant", answer, "gui", Some(&request), None);
        (q, a)
    }

    fn cursor(&self, session: &str) -> i64 {
        LearningCursorRepository::new(&self.db)
            .get(session)
            .unwrap()
            .extracted_through
    }

    fn set_cursor(&self, session: &str, to: i64) {
        self.db
            .with_connection(|c| {
                c.execute(
                    "INSERT INTO learning_cursor (session_id, extracted_through) VALUES (?1, ?2)
                     ON CONFLICT(session_id) DO UPDATE SET extracted_through = excluded.extracted_through",
                    (session, to),
                )?;
                Ok(())
            })
            .unwrap();
    }

    fn user_ctx(&self) -> MemoryAccessContext {
        self.ctx_in(None)
    }

    fn ctx_in(&self, root: Option<&std::path::Path>) -> MemoryAccessContext {
        self.service
            .context(
                OWNER,
                root.map_or(WorkspaceHint::None, WorkspaceHint::Root),
                Surface::UserSurface,
                Capabilities::ALL,
                Attribution::default(),
            )
            .unwrap()
    }

    fn list(&self, ctx: &MemoryAccessContext, status: Option<NoteStatus>) -> Vec<ListItem> {
        self.service
            .list(
                ctx,
                ListInput {
                    status,
                    include_candidates: status == Some(NoteStatus::Candidate),
                    ..Default::default()
                },
            )
            .unwrap()
            .items
    }

    fn active(&self) -> Vec<ListItem> {
        self.list(&self.user_ctx(), None)
    }

    fn candidates(&self) -> Vec<ListItem> {
        self.list(&self.user_ctx(), Some(NoteStatus::Candidate))
    }

    fn titles(&self) -> Vec<String> {
        self.active().into_iter().map(|i| i.title).collect()
    }
}

const PREF: &str = "I prefer vim for editing, always have.";

fn vim_item(rows: &[u32]) -> String {
    item(
        "Editor preference",
        "The user prefers vim for editing.",
        "preference",
        "user",
        rows,
    )
}

// ── The per-turn extractor's assertions, over a window (§8.10) ─────────

/// **MD-2.** A preference stated in the user's own words becomes an active
/// `extracted_user` note, in recall and on the recently-learned list.
#[tokio::test]
async fn a_user_stated_preference_becomes_an_active_extracted_user_note() {
    let scene = Scene::new(vec![items(&[vim_item(&[1])])]);
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");

    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.stopped, None, "{report:?}");
    assert_eq!((report.calls, report.stored, report.active), (1, 1, 1));

    let active = scene.active();
    assert_eq!(active.len(), 1, "{active:?}");
    assert_eq!(active[0].title, "Editor preference");
    assert_eq!(active[0].status, NoteStatus::Active);
    assert_eq!(active[0].origin, Origin::ExtractedUser);
    assert_eq!(active[0].kind, NoteKind::Preference);
    assert!(scene.candidates().is_empty());
    let learned = scene
        .service
        .recently_learned(&scene.user_ctx(), None, None)
        .unwrap();
    assert_eq!(learned.len(), 1);
    assert_eq!(learned[0].id, active[0].id);
}

/// **MD-2.** Something the model infers from the assistant's reply is a
/// candidate — outside the active list until it is confirmed.
#[tokio::test]
async fn an_assistant_claim_becomes_a_candidate() {
    let scene = Scene::new(vec![items(&[item(
        "Probably uses Rust",
        "The user's project is probably written in Rust.",
        "fact",
        "assistant",
        &[2],
    )])]);
    let s = scene.session();
    scene.turn(
        &s,
        "Can you look at my project and tell me what it does?",
        "It looks like a Rust workspace.",
    );
    let report = scene.queue.flush_session(&s).await;
    assert_eq!((report.stored, report.candidates), (1, 1), "{report:?}");
    assert!(scene.active().is_empty(), "a candidate is not active");
    let candidates = scene.candidates();
    assert_eq!(candidates.len(), 1, "{candidates:?}");
    assert_eq!(candidates[0].origin, Origin::ExtractedOther);
    assert!(
        scene
            .service
            .recently_learned(&scene.user_ctx(), None, None)
            .unwrap()
            .is_empty()
    );
}

/// **Rule 14.** `extract_max_items_per_call` notes per call: the items past
/// it are dropped and counted, never stored — in the model's order.
#[tokio::test]
async fn the_items_past_the_per_call_cap_are_dropped_and_counted() {
    let list: Vec<String> = (1..=6)
        .map(|i| {
            item(
                &format!("Preference {i}"),
                &format!("The user prefers option {i}."),
                "preference",
                "user",
                &[1],
            )
        })
        .collect();
    let mut config = DaemonConfig::default();
    assert_eq!(config.orchestrator.costs.extract_max_items_per_call, 12);
    config.orchestrator.costs.extract_max_items_per_call = 5;
    let scene = Scene::with(Scripted::new(vec![items(&list)]), config);
    let s = scene.session();
    scene.turn(&s, "Here are the six things I prefer, in order.", "Got it.");

    let report = scene.queue.flush_session(&s).await;
    assert_eq!(
        (report.stored, report.dropped_over_cap),
        (5, 1),
        "{report:?}"
    );
    let titles = scene.titles();
    assert_eq!(titles.len(), 5, "{titles:?}");
    assert!(!titles.contains(&"Preference 6".to_string()), "{titles:?}");
}

/// The acceptance floor: an item under `memory_confidence_threshold` is
/// dropped and counted, and the number itself is never stored.
#[tokio::test]
async fn an_item_under_the_confidence_floor_is_dropped_and_counted() {
    let scene = Scene::new(vec![Say::Text(
        r#"{"items": [{"title": "Maybe tea", "content": "The user might like tea.",
            "kind": "preference", "stated_by": "user", "confidence": 0.3, "rows": [1]}]}"#
            .to_string(),
    )]);
    let s = scene.session();
    scene.turn(&s, "I had a cup of something warm this morning.", "Nice.");
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.dropped_below_threshold, 1, "{report:?}");
    assert_eq!(report.stored, 0);
    assert!(scene.active().is_empty());
    assert!(scene.candidates().is_empty());
}

/// Design §3 and §1.1: the owner is the lane's user, and it must be a
/// `global_user` row. A `system` lane learns nothing, spends no model call —
/// and its cursor advances, so the window is never read again.
#[tokio::test]
async fn a_lane_with_no_owner_learns_nothing_calls_no_model_and_advances() {
    let scene = Scene::new(vec![items(&[vim_item(&[1])])]);
    let s = scene.session_on("system:internal");
    let (_, answer) = scene.turn(&s, PREF, "Noted.");
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.skipped_no_owner, 1, "{report:?}");
    assert_eq!(scene.model.calls(), 0);
    assert!(scene.active().is_empty());
    assert_eq!(scene.cursor(&s), answer);
}

/// A cancelled flush writes nothing and holds the window: the shutdown
/// token is observed before the model is asked.
#[tokio::test]
async fn a_cancelled_flush_writes_nothing_and_holds_the_window() {
    let scene = Scene::new(vec![items(&[vim_item(&[1])])]);
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");
    scene.queue.shutdown_token().cancel();
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.stopped, Some(FlushStop::Cancelled));
    assert_eq!(scene.model.calls(), 0);
    assert!(scene.active().is_empty());
    assert_eq!(scene.cursor(&s), 0, "the window holds");
}

/// Design §4: `sources` cite DB ids. The note cites the rows the model
/// named, each with its row's time — the user's own row, here.
#[tokio::test]
async fn the_note_cites_the_rows_the_model_named() {
    let scene = Scene::new(vec![items(&[vim_item(&[1])])]);
    let s = scene.session();
    let (question, _) = scene.turn(&s, PREF, "Noted.");
    scene.queue.flush_session(&s).await;

    let active = scene.active();
    assert_eq!(active.len(), 1);
    let note = scene
        .service
        .read(
            &scene.user_ctx(),
            ReadInput {
                id: active[0].id,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(note.sources.len(), 1, "{:?}", note.sources);
    assert_eq!(note.sources[0].kind, SourceType::Message);
    assert_eq!(note.sources[0].id, question.to_string());
    assert!(note.sources[0].observed_at.is_some(), "the row's own time");
}

/// The exact-dedup rule (§7b P8): the same item twice is one note, and the
/// second is counted as deduplicated.
#[tokio::test]
async fn the_same_item_twice_is_one_note() {
    let scene = Scene::new(vec![items(&[vim_item(&[1])])]);
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");
    scene.queue.flush_session(&s).await;
    scene.turn(&s, PREF, "Noted again.");
    // The second window's R1 is the older turn, shown for context; the new
    // question is R3. Cite it.
    *scene.model.script.lock().unwrap() = vec![items(&[vim_item(&[3])])].into();
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.deduplicated, 1, "{report:?}");
    assert_eq!(scene.active().len(), 1);
}

/// **M2.** The extractor asks for no reasoning, and its output budget comes
/// from the byte budget (§1.6), not a fixed 512 tokens.
#[tokio::test]
async fn the_extractor_asks_for_no_reasoning() {
    let scene = Scene::new(vec![]);
    let s = scene.session();
    scene.turn(
        &s,
        "I moved to Tokyo last week and I prefer concise answers.",
        "Noted.",
    );
    scene.queue.flush_session(&s).await;
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
    assert_eq!(
        request.max_tokens,
        Some(2800),
        "a large window's output budget"
    );
}

/// **M2, second half.** An answer the reasoning ate is the router's
/// `EmptyCompletion`, never filed as malformed JSON, and the window holds.
#[tokio::test]
async fn an_answer_the_reasoning_ate_is_not_filed_as_malformed_json() {
    let scene = Scene::new(vec![Say::Spent]);
    let s = scene.session();
    scene.turn(
        &s,
        "I moved to Tokyo last week and I prefer concise answers.",
        "Noted.",
    );
    let report = scene.queue.flush_session(&s).await;
    assert!(report.stopped.is_some(), "{report:?}");
    assert_eq!(scene.cursor(&s), 0, "the window holds");
    let logs = openalpaca_storage::LlmUsageRepository::new(&scene.db)
        .get_agent_usage(extractor::AGENT_LABEL, 10)
        .unwrap();
    assert!(
        logs.iter().all(|l| l
            .error_message
            .as_deref()
            .is_none_or(|m| !m.contains("JSON parse"))),
        "{:?}",
        logs.iter()
            .map(|l| l.error_message.clone())
            .collect::<Vec<_>>()
    );
}

/// §7b P10: a model that cannot answer holds the window — nothing is
/// written, nothing is skipped, and the turn was never affected.
#[tokio::test]
async fn an_unreachable_model_holds_the_window() {
    let scene = Scene::new(vec![Say::Fail(LlmError::Config(
        "the model is offline".into(),
    ))]);
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.stopped, Some(FlushStop::Unavailable), "{report:?}");
    assert_eq!(scene.cursor(&s), 0);
    assert!(scene.active().is_empty());
}

/// The off switch: `[orchestrator.learning] enabled = false` flushes
/// nothing and spends no call.
#[tokio::test]
async fn learning_switched_off_flushes_nothing() {
    let scene = Scene::new(vec![items(&[vim_item(&[1])])]);
    let mut config = DaemonConfig::default();
    config.orchestrator.learning.enabled = false;
    scene.config.store(Arc::new(config));
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.stopped, Some(FlushStop::Disabled));
    assert_eq!(scene.model.calls(), 0);
    assert_eq!(scene.cursor(&s), 0);
}

/// §1.9: the reply path never waits for learning — a turn completes while
/// a flush of its own session is stuck on a model that never answers.
#[tokio::test]
async fn the_gateway_returns_while_a_flush_is_stuck() {
    let scene = Scene::new(vec![Say::Hang]);
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");
    let queue = scene.queue.clone();
    let stuck_session = s.clone();
    let stuck = tokio::spawn(async move { queue.flush_session(&stuck_session).await });
    while scene.model.calls() == 0 {
        tokio::task::yield_now().await;
    }

    struct Echo;
    #[async_trait::async_trait]
    impl crate::gateway::MessageHandler for Echo {
        async fn handle(
            &self,
            request: crate::gateway::HandleRequest,
        ) -> Result<crate::gateway::HandleResult, String> {
            Ok(crate::gateway::HandleResult::text(request.content))
        }
    }
    let gateway = crate::gateway::Gateway::new(
        Arc::new(crate::context::SharedContext::new()),
        Arc::new(crate::lane::LaneManager::new()),
        Arc::new(Echo),
        crate::bus::EventBus::default(),
        Some(scene.db.clone()),
    )
    .with_learning(scene.queue.clone());
    let reply = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        gateway.handle_event(crate::gateway::GatewayRequest {
            source: openalpaca_api::events::EventSource::Gui {
                connection_id: OWNER.to_string(),
            },
            content: "another question".to_string(),
            attachments: Vec::new(),
            principal: crate::security::policy::Principal::User {
                global_id: OWNER.to_string(),
            },
            scope: crate::security::policy::Scope::Global,
            workspace_path: None,
            stream_id: None,
            lane_override: None,
            model_override: None,
            unattended: false,
            turn_sink: None,
        }),
    )
    .await
    .expect("the turn did not wait for the stuck flush");
    assert_eq!(reply.content, "another question");
    assert!(!stuck.is_finished(), "the flush is still stuck");
    stuck.abort();
}

// ── The window (§1.2) ────────────────────────────────────────────────────

/// MD-10: every persisted turn since the cursor reaches the extractor at the
/// next flush — not one in five, and not only the last one.
#[tokio::test]
async fn every_turn_since_the_cursor_reaches_one_flush() {
    let scene = Scene::new(vec![]);
    let s = scene.session();
    for i in 0..4 {
        scene.turn(
            &s,
            &format!("Turn {i}: something I want you to know."),
            "ok",
        );
    }
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.calls, 1, "{report:?}");
    let prompt = &scene.model.prompts()[0];
    for i in 0..4 {
        assert!(prompt.contains(&format!("Turn {i}:")), "{prompt}");
    }
    assert_eq!(scene.queue.flush_session(&s).await.calls, 0, "learned once");
}

/// **U2, re-expressed.** The window reads `content` only: an attachment
/// turn's extracted document text never reaches the extractor.
#[tokio::test]
async fn the_window_reads_content_never_content_json() {
    let scene = Scene::new(vec![]);
    let s = scene.session();
    let request = uuid::Uuid::new_v4().to_string();
    scene
        .repo()
        .insert_with_structured(
            &ConversationMessage {
                lane_key: LANE.to_string(),
                role: "user".to_string(),
                content: "Summarise the attached notes file for me, please.".to_string(),
                source: Some("gui".to_string()),
                session_id: Some(s.clone()),
                request_id: Some(request.clone()),
                ..Default::default()
            },
            r#"{"v":1,"parts":[{"type":"document","extracted_text":"The user prefers vim for editing."}]}"#,
            "Summarise the attached notes file for me, please.\n[Attachments: notes.txt]",
        )
        .unwrap();
    scene.row(
        &s,
        "assistant",
        "Here is a summary.",
        "gui",
        Some(&request),
        None,
    );
    scene.queue.flush_session(&s).await;
    let prompt = &scene.model.prompts()[0];
    assert!(
        prompt.contains("Summarise the attached notes file"),
        "{prompt}"
    );
    assert!(
        !prompt.contains("prefers vim"),
        "the document text reached the extractor"
    );
    assert!(
        !prompt.contains("[Attachments:"),
        "display_text reached the extractor"
    );
}

/// **F7.** A follow-up's re-entered turn is the assistant's own text: it is
/// labelled so, and an item that cites it is inferred — a candidate — even
/// when the model says the user stated it.
#[tokio::test]
async fn a_followup_row_is_assistant_written() {
    let scene = Scene::new(vec![items(&[vim_item(&[3])])]);
    let s = scene.session();
    scene.turn(&s, "Please set up my editor tomorrow.", "Will do.");
    let request = uuid::Uuid::new_v4().to_string();
    scene.row(
        &s,
        "user",
        "Next, remember the user prefers vim for editing and set it up.",
        INTERNAL_SOURCE,
        Some(&request),
        None,
    );
    scene.row(&s, "assistant", "Done.", "internal", Some(&request), None);
    let report = scene.queue.flush_session(&s).await;
    assert!(
        scene.model.prompts()[0].contains("R3 queued-task (written by the assistant)"),
        "{}",
        scene.model.prompts()[0]
    );
    assert_eq!((report.active, report.candidates), (0, 1), "{report:?}");
    assert!(scene.active().is_empty(), "no note in the user's name");
}

/// Rows no turn wrote — an extension notice, a `post_update`, a completion
/// report — stay out of the transcript (ruling LR2), and the cursor still
/// moves past them.
#[tokio::test]
async fn notices_post_updates_and_completion_reports_stay_out() {
    let scene = Scene::new(vec![]);
    let s = scene.session();
    scene.turn(
        &s,
        "Remember that I review pull requests on Fridays.",
        "Noted.",
    );
    scene.row(
        &s,
        "assistant",
        "NOTICE: the plugin was disabled",
        "gui",
        None,
        None,
    );
    scene.row(
        &s,
        "assistant",
        "UPDATE: the workflow is halfway",
        "gui",
        None,
        None,
    );
    let last = scene.row(
        &s,
        "assistant",
        "REPORT: the run finished",
        "gui",
        None,
        None,
    );
    scene.queue.flush_session(&s).await;
    let prompt = &scene.model.prompts()[0];
    for left_out in ["NOTICE", "UPDATE", "REPORT"] {
        assert!(
            !prompt.contains(left_out),
            "{left_out} reached the extractor: {prompt}"
        );
    }
    assert_eq!(scene.cursor(&s), last);
}

/// A `/slash` turn and its answer stay out, as the per-turn filter kept them.
#[tokio::test]
async fn slash_turns_and_their_answers_stay_out() {
    let scene = Scene::new(vec![]);
    let s = scene.session();
    scene.turn(&s, "/status", "SLASH-ANSWER: 3 tasks are running");
    scene.turn(
        &s,
        "I keep my notes in Obsidian, by the way.",
        "Good to know.",
    );
    scene.queue.flush_session(&s).await;
    let prompt = &scene.model.prompts()[0];
    assert!(!prompt.contains("/status"), "{prompt}");
    assert!(!prompt.contains("SLASH-ANSWER"), "{prompt}");
    assert!(prompt.contains("R1 user"), "the kept turn is R1: {prompt}");
}

/// A turn whose answer has not landed ends the window: the flush learns
/// what came before it and leaves the cursor in front of it.
#[tokio::test]
async fn an_unanswered_in_flight_turn_ends_the_window() {
    let scene = Scene::new(vec![]);
    let s = scene.session();
    let (_, answered) = scene.turn(&s, "I work from Lisbon now.", "Noted.");
    let request = uuid::Uuid::new_v4();
    let guard = scene.queue.turn_started(request, LANE, true);
    guard.attach_session(&s);
    scene.row(
        &s,
        "user",
        "IN-FLIGHT question still being answered",
        "gui",
        Some(&request.to_string()),
        None,
    );

    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.calls, 1, "{report:?}");
    assert!(!scene.model.prompts()[0].contains("IN-FLIGHT"));
    assert_eq!(
        scene.cursor(&s),
        answered,
        "the cursor stops before the row"
    );
    assert_eq!(report.stopped, Some(FlushStop::InFlight));
    drop(guard);
}

/// Flushes the turn's own session from inside the turn — after the gateway
/// persisted the user row, before the answer exists.
struct FlushMidTurn {
    queue: Arc<LearningQueue>,
    db: Database,
    report: Mutex<Option<FlushReport>>,
}

#[async_trait::async_trait]
impl crate::gateway::MessageHandler for FlushMidTurn {
    async fn handle(
        &self,
        request: crate::gateway::HandleRequest,
    ) -> Result<crate::gateway::HandleResult, String> {
        let session = ConversationRepository::new(&self.db)
            .active_session_id(&request.lane_key)
            .unwrap()
            .unwrap();
        let report = self.queue.flush_session(&session).await;
        *self.report.lock().unwrap() = Some(report);
        Ok(crate::gateway::HandleResult::text("answered".to_string()))
    }
}

/// Design §1.5 (C11): the turn is in flight **before** its user row exists,
/// so a flush that reads the window between the insert and the reply stops
/// before the row instead of learning a question whose answer has not
/// landed — and learns it once it has.
#[tokio::test]
async fn a_turn_is_in_flight_before_its_row_exists() {
    let scene = Scene::new(vec![]);
    let handler = Arc::new(FlushMidTurn {
        queue: scene.queue.clone(),
        db: scene.db.clone(),
        report: Mutex::new(None),
    });
    let gateway = crate::gateway::Gateway::new(
        Arc::new(crate::context::SharedContext::new()),
        Arc::new(crate::lane::LaneManager::new()),
        handler.clone(),
        crate::bus::EventBus::default(),
        Some(scene.db.clone()),
    )
    .with_learning(scene.queue.clone());
    gateway
        .handle_event(crate::gateway::GatewayRequest {
            source: openalpaca_api::events::EventSource::Gui {
                connection_id: OWNER.to_string(),
            },
            content: "MID-TURN I moved to Lisbon last month.".to_string(),
            attachments: Vec::new(),
            principal: crate::security::policy::Principal::User {
                global_id: OWNER.to_string(),
            },
            scope: crate::security::policy::Scope::Global,
            workspace_path: None,
            stream_id: None,
            lane_override: None,
            model_override: None,
            unattended: false,
            turn_sink: None,
        })
        .await;
    let mid_turn = handler.report.lock().unwrap().clone().expect("flushed");
    assert_eq!(mid_turn.stopped, Some(FlushStop::InFlight), "{mid_turn:?}");
    assert_eq!(
        scene.model.calls(),
        0,
        "the unanswered question was not learned"
    );

    // Once the answer landed, the next flush learns the whole turn.
    let session = scene.repo().active_session_id(LANE).unwrap().unwrap();
    let after = scene.queue.flush_session(&session).await;
    assert_eq!(after.calls, 1, "{after:?}");
    let prompt = &scene.model.prompts()[0];
    assert!(
        prompt.contains("MID-TURN") && prompt.contains("answered"),
        "{prompt}"
    );
}

// ── Crash semantics (§1.4) ──────────────────────────────────────────────

/// At-least-once: a crash after the store loop and before the cursor moves
/// repeats the chunk, and the repeat writes no duplicate (guard (a)).
#[tokio::test]
async fn a_crash_before_the_cursor_moves_reruns_without_a_duplicate() {
    let scene = Scene::new(vec![items(&[vim_item(&[1])]), items(&[vim_item(&[1])])]);
    let s = scene.session();
    let (_, answer) = scene.turn(&s, PREF, "Noted.");
    scene.queue.set_store_hooks(StoreHooks {
        skip_advance: true,
        ..StoreHooks::default()
    });
    let first = scene.queue.flush_session(&s).await;
    assert_eq!(first.stored, 1, "{first:?}");
    assert_eq!(
        scene.cursor(&s),
        0,
        "the crash left the cursor behind the writes"
    );

    scene.queue.set_store_hooks(StoreHooks::default());
    let rerun = scene.queue.flush_session(&s).await;
    assert_eq!((rerun.stored, rerun.deduplicated), (0, 1), "{rerun:?}");
    assert_eq!(scene.active().len(), 1, "no duplicate");
    assert_eq!(scene.cursor(&s), answer);
}

/// The store loop and the advance are one blocking task: a shutdown that
/// lands while it runs — the flush's own future aborted — still leaves the
/// cursor past the notes it wrote.
#[tokio::test]
async fn a_cancelled_flush_still_advances_after_its_store_loop() {
    let scene = Scene::new(vec![items(&[vim_item(&[1])])]);
    let s = scene.session();
    let (_, answer) = scene.turn(&s, PREF, "Noted.");

    let (reached_tx, reached_rx) = std::sync::mpsc::channel::<()>();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let go_rx = Arc::new(Mutex::new(go_rx));
    scene.queue.set_store_hooks(StoreHooks {
        after_store_loop: Arc::new(move || {
            reached_tx.send(()).unwrap();
            go_rx.lock().unwrap().recv().unwrap();
        }),
        skip_advance: false,
    });
    let queue = scene.queue.clone();
    let session = s.clone();
    let flush = tokio::spawn(async move { queue.flush_session(&session).await });
    tokio::task::spawn_blocking(move || reached_rx.recv().unwrap())
        .await
        .unwrap();
    // The store loop ran; cancel and abort the flush before the advance.
    scene.queue.shutdown_token().cancel();
    flush.abort();
    assert!(flush.await.unwrap_err().is_cancelled());
    go_tx.send(()).unwrap();

    // The closure finishes on its own thread.
    for _ in 0..200 {
        if scene.cursor(&s) == answer {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(scene.cursor(&s), answer, "the advance went with the writes");
    assert_eq!(scene.active().len(), 1);
}

/// Guard (d): a replay never resurrects a forgotten note — the forget
/// suppressed its row, and an item citing only that row is dropped.
#[tokio::test]
async fn a_rerun_never_resurrects_a_forgotten_note() {
    let scene = Scene::new(vec![items(&[vim_item(&[1])]), items(&[vim_item(&[1])])]);
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");
    scene.queue.flush_session(&s).await;
    let learned = scene.active();
    assert_eq!(learned.len(), 1);
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

    // A forced re-run: the cursor goes back and the chunk is read again.
    scene.set_cursor(&s, 0);
    let rerun = scene.queue.flush_session(&s).await;
    assert_eq!(
        (rerun.stored, rerun.dropped_suppressed),
        (0, 1),
        "{rerun:?}"
    );
    assert!(scene.active().is_empty(), "the forgotten note came back");
}

/// LQ4: a conversation deleted while its flush waits on the model writes
/// nothing when the answer arrives.
#[tokio::test]
async fn a_flush_that_finds_its_session_gone_writes_nothing() {
    let model = Scripted::new(vec![items(&[vim_item(&[1])])]);
    let scene = Scene::with(model.clone(), DaemonConfig::default());
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");
    let db = scene.db.clone();
    let doomed = s.clone();
    *model.on_call.lock().unwrap() = Some(Arc::new(move |_| {
        ConversationRepository::new(&db)
            .delete_session(&doomed)
            .unwrap();
    }));
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.stopped, Some(FlushStop::SessionGone), "{report:?}");
    assert_eq!(report.stored, 0);
    assert!(scene.active().is_empty());
}

/// Design §2: a quarantined partition holds the window **before** any model
/// call — no cost is spent on a chunk that cannot be written.
#[tokio::test]
async fn a_quarantined_partition_holds_the_window_before_any_model_call() {
    let scene = Scene::new(vec![items(&[vim_item(&[1])])]);
    // A note, so the partition has a snapshot to lose.
    scene
        .service
        .store(
            &scene.user_ctx(),
            crate::memory::service::StoreInput {
                title: "Seed".into(),
                body: "# Seed\n\nseed\n".into(),
                kind: Some(NoteKind::Fact),
                ..Default::default()
            },
        )
        .unwrap();
    let control = scene
        .root
        .path()
        .join("home")
        .canonicalize()
        .unwrap()
        .join("state/memory-control");
    for entry in std::fs::read_dir(&control).unwrap() {
        std::fs::write(entry.unwrap().path(), "{ not json").unwrap();
    }
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.stopped, Some(FlushStop::Quarantined), "{report:?}");
    assert_eq!(scene.model.calls(), 0, "no call was spent");
    assert_eq!(scene.cursor(&s), 0, "the window waits for the repair");
}

// ── Roots (§1.1, ruling LR1) ─────────────────────────────────────────────

/// A project directory the marker walk recognises, and its canonical root.
fn project() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let root = dir.path().canonicalize().unwrap();
    (dir, root)
}

/// Each row learns into its own turn's project: a turn that named no
/// project learns into the home partition even in a session bound to one.
#[tokio::test]
async fn a_projectless_turn_in_a_bound_session_learns_globally() {
    let (_dir, root) = project();
    let scene = Scene::new(vec![items(&[item(
        "Deploy procedure",
        "The user deploys with make ship.",
        "fact",
        "user",
        &[1],
    )])]);
    let s = scene
        .repo()
        .create_session(LANE, "gui", Some(&root.to_string_lossy()), None)
        .unwrap()
        .id;
    scene.turn_in(&s, "I deploy with make ship, always.", "Noted.", None);
    scene.queue.flush_session(&s).await;

    // A `fact` defaults to the project — but this turn named none.
    assert_eq!(scene.titles(), vec!["Deploy procedure".to_string()]);
    let in_project = scene.list(&scene.ctx_in(Some(&root)), None);
    assert!(
        in_project
            .iter()
            .all(|i| i.scope == openalpaca_storage::memory_files::NoteScope::Global),
        "{in_project:?}"
    );
}

/// A chunk never mixes roots: a project-less turn and a project turn are
/// two chunks, two calls, and the project turn's `fact` lands in the
/// project's partition.
#[tokio::test]
async fn a_chunk_never_mixes_roots() {
    let (_dir, root) = project();
    let root_str = root.to_string_lossy().to_string();
    let scene = Scene::new(vec![
        items(&[item(
            "Home fact",
            "The user lives in Porto.",
            "fact",
            "user",
            &[1],
        )]),
        // The second chunk's R1 and R2 are the first turn, shown for context.
        items(&[item(
            "Project fact",
            "The project ships on Tuesdays.",
            "fact",
            "user",
            &[3],
        )]),
    ]);
    let s = scene.session();
    scene.turn_in(&s, "I live in Porto, in the old town.", "Noted.", None);
    scene.turn_in(
        &s,
        "This project ships on Tuesdays.",
        "Noted.",
        Some(&root_str),
    );
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.calls, 2, "{report:?}");
    let prompts = scene.model.prompts();
    assert!(prompts[0].contains("Porto") && !prompts[0].contains("Tuesdays"));
    assert!(prompts[1].contains("Tuesdays"));

    let project_notes = scene.list(&scene.ctx_in(Some(&root)), None);
    let project_fact = project_notes
        .iter()
        .find(|i| i.title == "Project fact")
        .expect("learned");
    assert_eq!(
        project_fact.scope,
        openalpaca_storage::memory_files::NoteScope::Project
    );
    let home_fact = project_notes
        .iter()
        .find(|i| i.title == "Home fact")
        .expect("learned");
    assert_eq!(
        home_fact.scope,
        openalpaca_storage::memory_files::NoteScope::Global
    );
}

// ── Long text (§1.2, §1.6, ruling LR21) ─────────────────────────────────

/// MD-10: messages are no longer cut to 800 characters — the tail of a
/// 2,000-character message reaches the extractor.
#[tokio::test]
async fn a_long_message_reaches_the_extractor_whole_in_the_window() {
    let scene = Scene::new(vec![]);
    let s = scene.session();
    let tail = "TAIL-SENTINEL-the-end-of-the-message";
    let message = format!("{}{tail}", "a".repeat(2000 - tail.len()));
    scene.turn(&s, &message, "ok");
    scene.queue.flush_session(&s).await;
    assert!(scene.model.prompts()[0].contains(tail));
}

/// Ruling LR21: a user row over 8 KiB may quote other text — a piped email,
/// a pasted page — so an item it supports is inferred, never the user's own
/// words, whatever the model says.
#[tokio::test]
async fn a_long_user_row_yields_only_candidates() {
    let scene = Scene::new(vec![items(&[vim_item(&[1])])]);
    let s = scene.session();
    let long = format!(
        "Here is the thread. {}",
        "The user prefers vim. ".repeat(500)
    );
    assert!(long.len() > 8 * 1024);
    scene.turn(&s, &long, "Read it.");
    let report = scene.queue.flush_session(&s).await;
    assert!(
        scene.model.prompts()[0].contains("R1 user (long; may quote other text)"),
        "the row is labelled"
    );
    assert!(
        scene.model.prompts()[0].contains("bytes omitted"),
        "and cut 6 + 2 KiB"
    );
    assert_eq!((report.active, report.candidates), (0, 1), "{report:?}");
}

// ── Validation (§2.4, the L1 rules) ─────────────────────────────────────

/// Rule 4: rows are `3` or `"R3"`; a context row is removed and never a
/// source, and an item citing only context is invalid.
#[tokio::test]
async fn rows_accept_3_and_r3_and_drop_context_rows() {
    let scene = Scene::new(vec![]);
    let s = scene.session();
    scene.turn(&s, "I use a standing desk.", "Nice.");
    scene.queue.flush_session(&s).await;
    // Second window: R1, R2 are context; R3 is the new question.
    let (question, _) = scene.turn(&s, PREF, "Noted.");
    *scene.model.script.lock().unwrap() = vec![Say::Text(format!(
        r#"{{"items": [{}, {}]}}"#,
        serde_json::json!({"title": "Editor preference", "content": "The user prefers vim for editing.",
            "kind": "preference", "stated_by": "user", "confidence": 0.9, "rows": ["R1", "R3"]}),
        serde_json::json!({"title": "Desk", "content": "The user uses a standing desk.",
            "kind": "preference", "stated_by": "user", "confidence": 0.9, "rows": [1, 2]}),
    ))]
    .into();
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(
        (report.stored, report.dropped_invalid),
        (1, 1),
        "{report:?}"
    );
    let note_id = scene
        .active()
        .into_iter()
        .find(|i| i.title == "Editor preference")
        .unwrap()
        .id;
    let note = scene
        .service
        .read(
            &scene.user_ctx(),
            ReadInput {
                id: note_id,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        note.sources
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>(),
        vec![question.to_string()],
        "the context row is not a source"
    );
}

/// Rule 5 (ruling LR3): `stated_by: user` stands only when every cited row
/// is the user's own; citing the assistant's reply makes it inferred.
#[tokio::test]
async fn stated_by_user_citing_an_assistant_row_is_other_output() {
    let scene = Scene::new(vec![items(&[vim_item(&[1, 2])])]);
    let s = scene.session();
    scene.turn(&s, PREF, "So you prefer vim for editing.");
    let report = scene.queue.flush_session(&s).await;
    assert_eq!((report.active, report.candidates), (0, 1), "{report:?}");
}

/// Rule 1: an answer whose top level is not `{"items": [...]}` is a failed
/// attempt — nothing is written and the window holds.
#[tokio::test]
async fn an_answer_that_is_not_an_items_object_holds_the_window() {
    let scene = Scene::new(vec![Say::Text(r#"{"extractions": []}"#.into())]);
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.stopped, Some(FlushStop::BadOutput), "{report:?}");
    assert_eq!(scene.cursor(&s), 0);
}

/// The minimum, per chunk: less of the user's own text than
/// `extract_min_content_len` is not sent, and the cursor moves past it.
#[tokio::test]
async fn a_chunk_under_the_minimum_is_advanced_past_with_no_call() {
    let scene = Scene::new(vec![]);
    let s = scene.session();
    let (_, answer) = scene.turn(&s, "ok", "Great, anything else?");
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.skipped_short, 1, "{report:?}");
    assert_eq!(scene.model.calls(), 0);
    assert_eq!(scene.cursor(&s), answer);
}

// ── The budget (§1.6) ───────────────────────────────────────────────────

/// An 8,192-token local model gets 7,201 bytes of transcript, 3,086 bytes
/// of known notes and 9 items; a 128K model gets 24 KiB, 8 KiB and 12.
#[test]
fn budget_for_an_8k_window() {
    assert_eq!(
        Budget::for_window(Some(8192), 12, None),
        Budget {
            out_tokens: 2048,
            items: 9,
            transcript_bytes: 7201,
            known_bytes: 3086,
        }
    );
    assert_eq!(
        Budget::for_window(None, 12, None),
        Budget::for_window(Some(8192), 12, None),
        "an unknown window is Ollama's 8192 fallback"
    );
    assert_eq!(
        Budget::for_window(Some(128_000), 12, None),
        Budget {
            out_tokens: 2800,
            items: 12,
            transcript_bytes: 24 * 1024,
            known_bytes: 8 * 1024,
        }
    );
    // The default 40,000 TPM caps nothing a 128K model would send (62.5 KiB).
    assert_eq!(
        Budget::for_window(Some(128_000), 12, Some(40_000)),
        Budget::for_window(Some(128_000), 12, None)
    );
    // A key configured far below it does.
    let tight = Budget::for_window(Some(128_000), 12, Some(10_000));
    assert_eq!(tight.transcript_bytes, 16_000 * 7 / 10);
}
