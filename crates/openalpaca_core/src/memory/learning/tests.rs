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
    Attribution, Capabilities, ExtractionProvenance, ForgetInput, ListInput, ListItem,
    MemoryAccessContext, MemoryService, ReadInput, ReadOutput, StoreInput, StoreOutput, Surface,
    WorkspaceHint,
};
use crate::test_util::{HomeStoreGuard, RECORDED_MODEL};
use arc_swap::ArcSwap;
use openalpaca_llm::{
    ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, LlmRouter, ProviderType, Usage,
};
use openalpaca_storage::memory_files::{EffectiveStatus, NoteId, NoteScope};
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
        Self::build(model, router, config, QueueMode::Manual)
    }

    fn build(
        model: Arc<Scripted>,
        router: Arc<LlmRouter>,
        config: DaemonConfig,
        mode: QueueMode,
    ) -> Self {
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
            mode,
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

    /// A turn that ran through the gateway's hook and finished.
    fn finish_turn(&self, session: &str, user_authored: bool) {
        let guard = self
            .queue
            .turn_started(uuid::Uuid::new_v4(), LANE, user_authored);
        guard.attach_session(session);
    }

    /// The queue starts a flush the moment the daemon is quiet.
    fn no_quiet_period(&self) {
        self.queue.set_timings(super::queue::Timings {
            quiet: std::time::Duration::ZERO,
            ..Default::default()
        });
    }

    fn attempts(&self, session: &str) -> u32 {
        LearningCursorRepository::new(&self.db)
            .get(session)
            .unwrap()
            .attempts
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

    /// A note the user saved (origin `user`), with a `## Knowledge` body.
    fn save(&self, title: &str, knowledge: &str, key: Option<&str>) -> StoreOutput {
        self.service
            .store(
                &self.user_ctx(),
                StoreInput {
                    title: title.into(),
                    body: format!("# {title}\n\n## Knowledge\n{knowledge}\n"),
                    kind: Some(NoteKind::Fact),
                    fact_key: key.map(str::to_string),
                    ..Default::default()
                },
            )
            .unwrap()
    }

    /// A note learned earlier (origin `extracted_user`), in the home
    /// partition, with the body learning writes.
    fn learned(&self, title: &str, knowledge: &str, key: Option<&str>) -> StoreOutput {
        let ctx = self
            .service
            .context(
                OWNER,
                WorkspaceHint::None,
                Surface::Extraction,
                Capabilities::EXTRACTION,
                Attribution::default(),
            )
            .unwrap();
        self.service
            .store(
                &ctx,
                StoreInput {
                    title: title.into(),
                    body: format!(
                        "# {title}\n\n## Knowledge\n{knowledge}\n\n## Evidence and limits\nFrom what you said on 2026-09-01.\n"
                    ),
                    kind: Some(NoteKind::Fact),
                    fact_key: key.map(str::to_string),
                    provenance: Some(ExtractionProvenance::UserWords),
                    ..Default::default()
                },
            )
            .unwrap()
    }

    /// A note as the user's own surface reads it.
    fn read(&self, ctx: &MemoryAccessContext, id: NoteId) -> ReadOutput {
        self.service
            .read(
                ctx,
                ReadInput {
                    id,
                    ..Default::default()
                },
            )
            .unwrap()
    }

    /// Sets a row's `created_at` (UTC, SQLite's form).
    fn set_created_at(&self, row: i64, at: &str) {
        self.db
            .with_connection(|c| {
                c.execute(
                    "UPDATE conversation_messages SET created_at = ?1 WHERE id = ?2",
                    (at, row),
                )?;
                Ok(())
            })
            .unwrap();
    }
}

/// One item with every field §2.2 names; `extra` overrides or adds.
fn full_item(extra: serde_json::Value) -> String {
    let mut item = serde_json::json!({
        "title": "Editor preference",
        "content": "The user edits with Helix.",
        "kind": "preference",
        "stated_by": "user",
        "confidence": 0.9,
        "rows": [1],
        "aliases": [],
        "fact_key": null,
        "replaces": null,
        "replace_reason": null,
        "valid_from": null,
        "valid_to": null
    });
    for (k, v) in extra.as_object().expect("an object").iter() {
        item[k] = v.clone();
    }
    item.to_string()
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

/// Design 09 §2.2, §2.5 (the flipped characterization of D3): the aliases
/// — the other language included — the `fact_key` and the validity a model
/// sends reach the note, and the note records `observed_at`, the time of
/// its newest cited row.
#[tokio::test]
async fn aliases_and_fact_key_reach_the_note() {
    let scene = Scene::new(vec![Say::Text(
        serde_json::json!({"items": [{
            "title": "Editor preference",
            "content": "The user edits with Helix.",
            "kind": "preference",
            "stated_by": "user",
            "confidence": 0.9,
            "rows": [1],
            "aliases": ["Helix", "编辑器", "helix", " Editor preference "],
            "fact_key": "User.Editor",
            "valid_from": "2026-06-01",
            "valid_to": null
        }]})
        .to_string(),
    )]);
    let s = scene.session();
    let (question, _) = scene.turn(&s, "I edit with Helix these days.", "Noted.");
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.active, 1, "{report:?}");

    let active = scene.active();
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
    assert_eq!(
        note.frontmatter["aliases"],
        serde_json::json!(["Helix", "编辑器"]),
        "deduplicated without regard to case, the title removed"
    );
    assert_eq!(
        note.frontmatter["fact_key"],
        serde_json::json!("user.editor")
    );
    assert_eq!(
        note.frontmatter["valid_from"],
        serde_json::json!("2026-06-01T00:00:00Z")
    );
    let created: String = scene
        .db
        .with_connection(|c| {
            Ok(c.query_row(
                "SELECT created_at FROM conversation_messages WHERE id = ?1",
                [question],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(
        note.frontmatter["observed_at"],
        serde_json::json!(created.replace(' ', "T") + "Z")
    );
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
    // Guard (b), §1.4: the lookup finds the note this chunk already taught
    // through the index's `note_source`, and the item is dropped before any
    // write — exact dedup would answer the same, one guard later.
    assert_eq!(
        (rerun.stored, rerun.dropped_already_learned),
        (0, 1),
        "{rerun:?}"
    );
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
    // Not even attempted: the session check comes before the store loop,
    // not only `verify_sources` refusing each deleted row.
    assert_eq!(report.failed, 0, "no write was attempted: {report:?}");
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

/// A turn whose project directory was deleted before learning reached it is
/// skipped with no call, and the directory stays deleted: learning never
/// re-creates a project — or its `.openalpaca/` store — the user removed.
/// Its turns are dropped the way a deleted conversation's are (LQ4).
#[tokio::test]
async fn a_turn_whose_project_directory_is_gone_is_skipped() {
    let (dir, root) = project();
    let scene = Scene::new(vec![items(&[item(
        "Project fact",
        "The project ships on Tuesdays.",
        "fact",
        "user",
        &[1],
    )])]);
    let s = scene.session();
    let (_, answer) = scene.turn_in(
        &s,
        "This project ships on Tuesdays.",
        "Noted.",
        Some(&root.to_string_lossy()),
    );
    drop(dir);
    assert!(!root.exists());

    let report = scene.queue.flush_session(&s).await;
    assert!(!root.exists(), "learning re-created the deleted project");
    assert_eq!(scene.model.calls(), 0, "no call: nowhere to write");
    assert_eq!(report.stopped, None, "{report:?}");
    assert_eq!(report.skipped_root_gone, 1, "{report:?}");
    assert_eq!(scene.cursor(&s), answer, "the cursor moved past its turns");
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
    // The default 40,000 TPM caps nothing a 128K model would send (56.5 KiB
    // of rows and notes beside the request's own 6 KiB).
    assert_eq!(
        Budget::for_window(Some(128_000), 12, Some(40_000)),
        Budget::for_window(Some(128_000), 12, None)
    );
    // A key configured far below it does, leaving room for the prompt.
    let tight = Budget::for_window(Some(128_000), 12, Some(10_000));
    assert_eq!(
        tight.transcript_bytes,
        (16_000 - REQUEST_OVERHEAD_BYTES) * 7 / 10
    );
}

/// The largest request text a chunk adds to its rows and its known notes:
/// the fixed prompt, and two context rows at their cut with the longest
/// label.
fn request_overhead_bytes() -> usize {
    let prompt: usize = extractor::build_request(
        &extractor::PromptInput {
            today: "2026-09-24",
            known_notes: "",
            known_keys: "",
            conversation: "",
            max_items: 12,
        },
        2800,
        None,
    )
    .messages
    .iter()
    .map(|m| m.content.len())
    .sum();
    let request = "context-turn";
    let before = [
        openalpaca_storage::LearningRow {
            id: 1,
            role: "user".to_string(),
            content: "u".repeat(LONG_USER_ROW_BYTES + 1),
            source: Some("gui".to_string()),
            created_at: "2026-09-24 10:00:00".to_string(),
            request_id: Some(request.to_string()),
            workspace_root: None,
        },
        openalpaca_storage::LearningRow {
            id: 2,
            role: "assistant".to_string(),
            content: "a".repeat(LONG_USER_ROW_BYTES + 1),
            source: Some("gui".to_string()),
            created_at: "2026-09-24 10:00:01".to_string(),
            request_id: Some(request.to_string()),
            workspace_root: None,
        },
    ];
    let context = window::context_rows(&before);
    assert_eq!(context.len(), 2);
    let chunk = window::Chunk {
        context,
        rows: Vec::new(),
        through: 2,
        root: None,
    };
    prompt + chunk.transcript().0.len()
}

/// The TPM cap budgets the whole request, not only the chunk's rows: what a
/// request adds to them fits the overhead the cap subtracts.
#[test]
fn the_fixed_prompt_and_its_context_fit_the_tpm_caps_overhead() {
    let overhead = request_overhead_bytes();
    assert!(
        overhead <= REQUEST_OVERHEAD_BYTES,
        "the request adds {overhead} bytes, over REQUEST_OVERHEAD_BYTES"
    );
}

// ── Retry and backoff (§1.4, ruling LR20) ───────────────────────────────

/// Only a reachable model's bad output is counted: after three unusable
/// answers to one chunk the cursor moves past it, with one WARN naming the
/// session and the rows — one poisoned chunk cannot block a session forever.
#[tokio::test]
#[tracing_test::traced_test]
async fn three_invalid_outputs_skip_the_chunk_with_one_warn() {
    let bad = || Say::Text("I cannot answer in JSON today.".into());
    let scene = Scene::new(vec![bad(), bad(), bad()]);
    let s = scene.session();
    let (_, answer) = scene.turn(&s, PREF, "Noted.");

    for attempt in 1..=2 {
        let report = scene.queue.flush_session(&s).await;
        assert_eq!(report.stopped, Some(FlushStop::BadOutput), "{report:?}");
        assert_eq!(scene.attempts(&s), attempt);
        assert_eq!(scene.cursor(&s), 0, "the window holds");
    }
    let third = scene.queue.flush_session(&s).await;
    assert_eq!(third.skipped_unusable, 1, "{third:?}");
    assert_eq!(scene.cursor(&s), answer, "the cursor moved past the chunk");
    assert_eq!(scene.attempts(&s), 0, "and the count starts over");
    logs_assert(|lines: &[&str]| {
        let warns: Vec<&&str> = lines
            .iter()
            .filter(|l| l.contains("WARN") && l.contains("memory learning"))
            .collect();
        match warns.as_slice() {
            [one] if one.contains("skipped rows") && one.contains(&s) => Ok(()),
            other => Err(format!("expected one skip WARN, saw {other:?}")),
        }
    });
}

/// §7b P10 and ruling LR20: a model that cannot answer holds the window,
/// counts no attempt, backs off (60 s, doubling), and is logged once per
/// outage — then once more when learning resumes.
#[tokio::test]
#[tracing_test::traced_test]
async fn an_unavailable_model_holds_the_window_and_backs_off() {
    let offline = || Say::Fail(LlmError::Config("the model is offline".into()));
    let scene = Scene::new(vec![offline(), offline(), items(&[vim_item(&[1])])]);
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");

    let first = scene.queue.flush_session(&s).await;
    assert_eq!(first.stopped, Some(FlushStop::Unavailable));
    let wait = scene.queue.retry_in(&s).expect("a backoff");
    assert!(
        wait > std::time::Duration::from_secs(55) && wait <= std::time::Duration::from_secs(60),
        "{wait:?}"
    );
    scene.queue.flush_session(&s).await;
    let wait = scene.queue.retry_in(&s).expect("a longer backoff");
    assert!(
        wait > std::time::Duration::from_secs(115),
        "doubled: {wait:?}"
    );
    assert_eq!(scene.attempts(&s), 0, "an outage counts no attempt");
    assert_eq!(scene.cursor(&s), 0);

    let back = scene.queue.flush_session(&s).await;
    assert_eq!(back.stored, 1, "{back:?}");
    assert_eq!(
        scene.queue.retry_in(&s),
        None,
        "a success clears the backoff"
    );
    logs_assert(|lines: &[&str]| {
        let paused = lines
            .iter()
            .filter(|l| l.contains("WARN") && l.contains("memory learning paused"))
            .count();
        let resumed = lines
            .iter()
            .filter(|l| l.contains("INFO") && l.contains("memory learning resumed"))
            .count();
        match (paused, resumed) {
            (1, 1) => Ok(()),
            other => Err(format!("expected one WARN and one INFO, saw {other:?}")),
        }
    });
}

// ── The durable daily gates (§1.8, ruling LR8) ──────────────────────────

fn usage_row(
    date: chrono::NaiveDate,
    requests: i32,
    cost: f64,
) -> openalpaca_storage::LlmUsageDaily {
    openalpaca_storage::LlmUsageDaily {
        date: date.format("%Y-%m-%d").to_string(),
        agent_id: extractor::AGENT_LABEL.to_string(),
        model: "m".to_string(),
        total_requests: requests,
        total_input_tokens: 10,
        total_output_tokens: 10,
        total_cost_usd: cost,
    }
}

/// A binding cap leaves the window queued: no call, the cursor stays, the
/// session stays due, and it resumes when the cap no longer binds.
#[tokio::test]
async fn a_cap_leaves_the_window_queued() {
    let mut config = DaemonConfig::default();
    config.orchestrator.costs.extract_max_calls_per_day = 1;
    let scene = Scene::with(
        Scripted::new(vec![items(&[vim_item(&[1])])]),
        config.clone(),
    );
    openalpaca_storage::LlmUsageRepository::new(&scene.db)
        .upsert_daily_usage(&usage_row(chrono::Utc::now().date_naive(), 1, 0.0))
        .unwrap();
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");

    let capped = scene.queue.flush_session(&s).await;
    assert_eq!(capped.stopped, Some(FlushStop::Cap), "{capped:?}");
    assert_eq!(scene.model.calls(), 0);
    assert_eq!(scene.cursor(&s), 0);
    assert!(scene.queue.is_due(&s), "still queued");

    config.orchestrator.costs.extract_max_calls_per_day = 0; // no cap
    scene.config.store(Arc::new(config));
    let resumed = scene.queue.flush_session(&s).await;
    assert_eq!(resumed.stored, 1, "{resumed:?}");
}

/// §1.8's "one INFO per session per day": a capped session tried on every
/// sweep logs the cap once, and a flush that did nothing — no call, no chunk
/// — is not logged at INFO. A flush that learned still is.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_capped_session_logs_one_info_however_often_it_is_tried() {
    let mut config = DaemonConfig::default();
    config.orchestrator.costs.extract_max_calls_per_day = 1;
    let scene = Scene::with(
        Scripted::new(vec![items(&[vim_item(&[1])])]),
        config.clone(),
    );
    openalpaca_storage::LlmUsageRepository::new(&scene.db)
        .upsert_daily_usage(&usage_row(chrono::Utc::now().date_naive(), 1, 0.0))
        .unwrap();
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");
    for _ in 0..3 {
        let capped = scene.queue.flush_session(&s).await;
        assert_eq!(capped.stopped, Some(FlushStop::Cap), "{capped:?}");
    }
    config.orchestrator.costs.extract_max_calls_per_day = 0;
    scene.config.store(Arc::new(config));
    let learned = scene.queue.flush_session(&s).await;
    assert_eq!(learned.stored, 1, "{learned:?}");

    logs_assert(|lines: &[&str]| {
        let info = |needle: &str| {
            lines
                .iter()
                .filter(|l| l.contains(" INFO ") && l.contains(needle))
                .count()
        };
        match (
            info("memory learning finished"),
            info("daily cap is reached"),
        ) {
            (1, 1) => Ok(()),
            other => Err(format!(
                "expected one INFO for the flush that learned and one for the cap, saw {other:?}"
            )),
        }
    });
}

/// The daily-cap defect, fixed: the gate counts today's UTC date in
/// `llm_usage_daily`. Yesterday's calls and cost bind nothing — nor does
/// the in-memory cost tracker, which counts since boot and never resets —
/// and today's cost binds.
#[tokio::test]
async fn the_daily_cap_counts_the_utc_day_from_llm_usage_daily() {
    let scene = Scene::new(vec![items(&[vim_item(&[1])])]);
    let today = chrono::Utc::now().date_naive();
    let usage = openalpaca_storage::LlmUsageRepository::new(&scene.db);
    usage
        .upsert_daily_usage(&usage_row(today - chrono::Duration::days(1), 500, 9.0))
        .unwrap();
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");
    let yesterday_heavy = scene.queue.flush_session(&s).await;
    assert_eq!(yesterday_heavy.stored, 1, "{yesterday_heavy:?}");
    let (calls, _) = usage.usage_today(extractor::AGENT_LABEL).unwrap();
    assert_eq!(calls, 1, "the call itself is counted today");

    usage.upsert_daily_usage(&usage_row(today, 0, 1.0)).unwrap();
    scene.turn(&s, "And I keep my dotfiles in a git repository.", "Noted.");
    let capped = scene.queue.flush_session(&s).await;
    assert_eq!(
        capped.stopped,
        Some(FlushStop::Cap),
        "today's $1.00 > $0.25"
    );
}

// ── Sharing the model with chat (§1.7, ruling LR7) ──────────────────────

/// A model registered under `provider` with an 8K window, on a router whose
/// only provider is `model`.
fn router_for(model: Arc<Scripted>, provider: ProviderType, id: &str) -> Arc<LlmRouter> {
    let router = LlmRouter::single_provider(model, provider.clone(), id.to_string());
    router.model_registry().register(
        id.to_string(),
        openalpaca_llm::ModelInfo {
            provider,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 8192,
            discovered: true,
            supports_image: false,
            supports_audio: false,
            supports_document: false,
            supports_reasoning: false,
            supports_tools: true,
            declared: false,
        },
    );
    Arc::new(router)
}

/// Starts an automatic flush and returns once its first model call is made.
async fn flush_until_the_call(
    scene: &Scene,
    session: &str,
) -> tokio::task::JoinHandle<FlushReport> {
    let queue = scene.queue.clone();
    let session = session.to_string();
    let flush = tokio::spawn(async move { queue.flush_automatic(&session).await });
    while scene.model.calls() == 0 {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    flush
}

/// Asserts a yielded flush wrote nothing, counted nothing and stays due.
fn assert_yielded(scene: &Scene, session: &str, report: &FlushReport) {
    assert_eq!(report.stopped, Some(FlushStop::Yielded), "{report:?}");
    assert_eq!(scene.cursor(session), 0);
    assert_eq!(scene.attempts(session), 0);
    assert!(scene.active().is_empty());
    assert!(scene.queue.is_due(session));
}

/// On a local model — a provider that needs no key — a chat turn that
/// starts cancels the flush's call: the GPU serves the reply alone.
#[tokio::test]
async fn a_local_model_flush_yields_to_a_chat_turn() {
    let model = Arc::new(Scripted {
        keyless: true,
        ..Arc::into_inner(Scripted::new(vec![Say::Hang])).unwrap()
    });
    let router = router_for(model.clone(), ProviderType::Ollama, "local-model");
    let scene = Scene::with_router(model, router, DaemonConfig::default());
    scene.no_quiet_period();
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");

    let flush = flush_until_the_call(&scene, &s).await;
    let _chat = scene.queue.turn_started(uuid::Uuid::new_v4(), LANE, true);
    let report = tokio::time::timeout(std::time::Duration::from_secs(10), flush)
        .await
        .expect("the flush did not yield")
        .unwrap();
    assert_yielded(&scene, &s, &report);
}

/// The same for a keyed `openai` provider whose base URL is loopback — a
/// llama.cpp or LM Studio server — which would otherwise never yield.
#[tokio::test]
async fn an_openai_provider_on_localhost_yields() {
    let model = Scripted::new(vec![Say::Hang]);
    let router = router_for(model.clone(), ProviderType::OpenAI, "local-openai");
    let mut runtime = openalpaca_llm::LlmRuntimeConfig::default();
    runtime
        .provider_defaults
        .get_mut("openai")
        .expect("openai has defaults")
        .base_url = Some("http://127.0.0.1:8080/v1".to_string());
    router.reload_runtime_config(runtime);
    let scene = Scene::with_router(model, router, DaemonConfig::default());
    scene.no_quiet_period();
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");

    let flush = flush_until_the_call(&scene, &s).await;
    let _chat = scene.queue.turn_started(uuid::Uuid::new_v4(), LANE, true);
    let report = tokio::time::timeout(std::time::Duration::from_secs(10), flush)
        .await
        .expect("the flush did not yield")
        .unwrap();
    assert_yielded(&scene, &s, &report);
}

/// The quiet check and the call are one decision: a chat turn that starts
/// while the flush is between them — here, waiting on its window read —
/// makes a local model's flush yield **before** the call goes out. The call
/// is registered for cancellation under the lock that reads the in-flight
/// set, so no turn can slip between the check and the registration.
#[tokio::test]
async fn a_turn_before_the_call_is_registered_still_makes_it_yield() {
    let model = Arc::new(Scripted {
        keyless: true,
        ..Arc::into_inner(Scripted::new(vec![Say::Hang])).unwrap()
    });
    let router = router_for(model.clone(), ProviderType::Ollama, "local-model");
    let scene = Scene::with_router(model, router, DaemonConfig::default());
    scene.no_quiet_period();
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");

    // Hold the database, so the flush passes its quiet check and then waits
    // on its window read.
    let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let db = scene.db.clone();
    let holder = std::thread::spawn(move || {
        db.with_connection(|_| {
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        })
        .unwrap();
    });
    held_rx.recv().unwrap();
    let queue = scene.queue.clone();
    let session = s.clone();
    let flush = tokio::spawn(async move { queue.flush_automatic(&session).await });
    // The test runtime has one thread: the flush runs to its first await,
    // the blocked window read, before this task resumes.
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert!(!flush.is_finished(), "the flush waits on its window read");

    let _chat = scene.queue.turn_started(uuid::Uuid::new_v4(), LANE, true);
    release_tx.send(()).unwrap();
    let report = tokio::time::timeout(std::time::Duration::from_secs(5), flush)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "the extraction call went out while a chat turn was in flight (calls = {})",
                scene.model.calls()
            )
        })
        .unwrap();
    holder.join().unwrap();
    assert_eq!(scene.model.calls(), 0, "no call while a turn is in flight");
    assert_yielded(&scene, &s, &report);
}

/// A remote provider with one key whose TPM bucket holds `tpm`.
fn keyed_router(model: Arc<Scripted>, tpm: u32) -> Arc<LlmRouter> {
    let mut providers = std::collections::HashMap::new();
    providers.insert(
        ProviderType::Anthropic,
        openalpaca_llm::ProviderEntry {
            provider: model,
            key_pool: Arc::new(ArcSwap::from_pointee(openalpaca_llm::KeyPool::new(
                vec![openalpaca_llm::ApiKey::new(
                    "k1".to_string(),
                    ProviderType::Anthropic,
                    "sk-test".to_string(),
                )],
                openalpaca_llm::SelectionStrategy::RoundRobin,
            ))),
        },
    );
    Arc::new(LlmRouter::new_with_runtime(
        providers,
        openalpaca_llm::ModelRegistry::with_defaults(),
        std::collections::HashMap::new(),
        Arc::new(openalpaca_llm::CostTracker::new(
            openalpaca_llm::ModelRegistry::with_defaults(),
        )),
        RECORDED_MODEL.to_string(),
        openalpaca_llm::LlmRuntimeConfig::default(),
        openalpaca_llm::RateLimitConfig {
            default_tpm: tpm,
            ..Default::default()
        },
    ))
}

/// On a keyed remote model a chunk starts only when half of the key's RPM
/// and TPM would remain after it: the second chunk waits, and a chat round
/// made then still finds at least half the bucket.
#[tokio::test]
async fn a_keyed_flush_leaves_half_the_rate_budget() {
    let model = Scripted::new(vec![items(&[]), items(&[])]);
    const TPM: u32 = 10_000;
    let router = keyed_router(model.clone(), TPM);
    let scene = Scene::with_router(model, router.clone(), DaemonConfig::default());
    scene.no_quiet_period();
    let (_dir, root) = project();
    let s = scene.session();
    // Two turns in two roots: two chunks, each filling its transcript
    // budget — about 2,400 and 3,000 tokens with the prompt and context.
    let (_, first_answer) = scene.turn_in(
        &s,
        &format!("Home. {}", "a ".repeat(2500)),
        &"c ".repeat(2500),
        None,
    );
    scene.turn_in(
        &s,
        &format!("Project. {}", "b ".repeat(2500)),
        &"d ".repeat(2500),
        Some(&root.to_string_lossy()),
    );

    let report = scene.queue.flush_automatic(&s).await;
    assert_eq!(report.calls, 1, "{report:?}");
    assert_eq!(report.stopped, Some(FlushStop::RateHeadroom));
    assert_eq!(
        scene.cursor(&s),
        first_answer,
        "the first chunk was learned"
    );
    assert!(scene.queue.is_due(&s), "the second waits, still due");

    let left = router.rate_headroom(RECORDED_MODEL).await.unwrap();
    assert!(left.tpm_available >= f64::from(TPM) / 2.0, "{left:?}");
    let started = std::time::Instant::now();
    router
        .complete(openalpaca_llm::RouterRequest {
            model: None,
            messages: Arc::new(vec![openalpaca_llm::ChatMessage::user("a chat round")]),
            tools: Arc::new(vec![]),
            temperature: None,
            max_tokens: None,
            context: Default::default(),
            tool_choice: None,
            tools_token_estimate: None,
            enable_caching: false,
            thinking: None,
            context_management: None,
            fallback_models: Vec::new(),
            ephemeral_system_notice: None,
        })
        .await
        .unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "the chat round waited for the rate budget"
    );
}

/// §1.7's "a chunk can always start once the key is idle", on a key far
/// below the default TPM: the 40 % cap covers the whole request — the fixed
/// prompt and its context rows too, not only the chunk's own rows — so an
/// idle key's first chunk leaves half the bucket and goes out.
#[tokio::test]
async fn an_idle_key_far_below_the_default_tpm_still_learns() {
    let model = Scripted::new(vec![]);
    let router = keyed_router(model.clone(), 3_000);
    let scene = Scene::with_router(model, router, DaemonConfig::default());
    scene.no_quiet_period();
    let s = scene.session();
    // A learned turn long enough that the next chunk's context rows are
    // full, then new turns that fill the transcript budget.
    let (_, learned) = scene.turn(&s, &"u".repeat(10 * 1024), &"a".repeat(5 * 1024));
    scene.set_cursor(&s, learned);
    for i in 0..3 {
        scene.turn(&s, &format!("Fact {i}: {}", "z ".repeat(500)), "ok");
    }

    let report = scene.queue.flush_automatic(&s).await;
    assert!(report.calls >= 1, "the idle key never started: {report:?}");
    assert!(scene.cursor(&s) > learned, "no progress: {report:?}");
}

/// The quiet check runs before every chunk: a turn that starts while the
/// first chunk's call is out stops the flush at the next chunk boundary,
/// and the session stays due.
#[tokio::test]
async fn a_turn_mid_flush_stops_it_at_the_next_chunk() {
    let model = Scripted::new(vec![items(&[]), items(&[])]);
    let scene = Scene::with(model.clone(), DaemonConfig::default());
    scene.no_quiet_period();
    let (_dir, root) = project();
    let s = scene.session();
    let (_, first_answer) = scene.turn_in(&s, "I live in Porto, in the old town.", "ok", None);
    scene.turn_in(
        &s,
        "This project ships on Tuesdays.",
        "ok",
        Some(&root.to_string_lossy()),
    );
    let chat: Arc<Mutex<Vec<TurnGuard>>> = Arc::new(Mutex::new(Vec::new()));
    let queue = scene.queue.clone();
    let held = chat.clone();
    *model.on_call.lock().unwrap() = Some(Arc::new(move |_| {
        held.lock()
            .unwrap()
            .push(queue.turn_started(uuid::Uuid::new_v4(), LANE, true));
    }));

    let report = scene.queue.flush_automatic(&s).await;
    assert_eq!(report.calls, 1, "{report:?}");
    assert_eq!(report.stopped, Some(FlushStop::Busy));
    assert_eq!(scene.cursor(&s), first_answer);
    assert!(scene.queue.is_due(&s));
    chat.lock().unwrap().clear();
}

// ── The triggers (§1.5) ─────────────────────────────────────────────────

/// MD-10's "idle about 2 minutes": a session with an unlearned turn becomes
/// due once its last turn is `idle_secs` old, and never on idle when
/// `idle_secs = 0`.
#[tokio::test(start_paused = true)]
async fn idle_120s_makes_a_session_due() {
    let scene = Scene::new(vec![]);
    let s = scene.session();
    scene.finish_turn(&s, true);
    tokio::time::advance(std::time::Duration::from_secs(119)).await;
    scene.queue.sweep().await;
    assert!(!scene.queue.is_due(&s), "119 s is not idle yet");
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    scene.queue.sweep().await;
    assert!(scene.queue.is_due(&s), "121 s is");

    let mut config = DaemonConfig::default();
    config.orchestrator.learning.idle_secs = 0;
    scene.config.store(Arc::new(config));
    let other = scene.session_on("user1:cli");
    scene.finish_turn(&other, true);
    tokio::time::advance(std::time::Duration::from_secs(3600)).await;
    scene.queue.sweep().await;
    assert!(!scene.queue.is_due(&other), "idle_secs = 0 switches it off");
}

/// MD-10's "10 new turns": the tenth user-authored turn since the session
/// was learned makes it due at once; assistant-written turns do not count.
#[tokio::test]
async fn the_tenth_user_turn_makes_it_due() {
    let scene = Scene::new(vec![]);
    let s = scene.session();
    for _ in 0..9 {
        scene.finish_turn(&s, true);
    }
    scene.finish_turn(&s, false);
    assert!(
        !scene.queue.is_due(&s),
        "nine of the user's turns and a follow-up"
    );
    scene.finish_turn(&s, true);
    assert!(scene.queue.is_due(&s));
    assert_eq!(scene.queue.pending_turns(&s), 10);
}

/// "The user switches conversations": a session that is no longer its
/// lane's active one — "New chat" archived it — is due at the next sweep.
#[tokio::test]
async fn an_archived_session_with_pending_turns_is_due() {
    let scene = Scene::new(vec![]);
    let s = scene.session();
    scene.finish_turn(&s, true);
    scene.queue.sweep().await;
    assert!(!scene.queue.is_due(&s), "still the active conversation");
    scene.session(); // "New chat" archives `s`.
    scene.queue.sweep().await;
    assert!(scene.queue.is_due(&s));
}

/// The switch trigger's other half, in the gateway's hook: a lane's turn that
/// lands in another session — R48's project switch opens it right there —
/// makes the session it left due at once, before any sweep. Another turn in
/// the same session, or a turn on another lane, is no switch.
#[tokio::test]
async fn a_turn_in_another_session_of_its_lane_makes_the_one_it_left_due() {
    let scene = Scene::new(vec![]);
    let left = scene.session();
    scene.finish_turn(&left, true);
    scene.finish_turn(&left, true);
    assert!(!scene.queue.is_due(&left), "the same session twice");

    let elsewhere = scene.session_on("user1:cli");
    let other_lane = scene
        .queue
        .turn_started(uuid::Uuid::new_v4(), "user1:cli", true);
    other_lane.attach_session(&elsewhere);
    assert!(!scene.queue.is_due(&left), "another lane's turn");

    let next = scene.session();
    let turn = scene.queue.turn_started(uuid::Uuid::new_v4(), LANE, true);
    turn.attach_session(&next);
    assert!(scene.queue.is_due(&left), "the session it left is due");
    assert!(!scene.queue.is_due(&next));
}

/// LQ4: a deleted conversation's queue state is dropped at the next sweep
/// — its unlearned turns went with its messages.
#[tokio::test]
async fn a_deleted_session_is_forgotten() {
    let scene = Scene::new(vec![]);
    let s = scene.session();
    scene.finish_turn(&s, true);
    assert_eq!(scene.queue.pending_turns(&s), 1);
    scene.repo().delete_session(&s).unwrap();
    scene.queue.sweep().await;
    assert_eq!(scene.queue.pending_turns(&s), 0);
    assert!(!scene.queue.is_due(&s));
}

/// "Before the daemon stops": the drain flushes what is pending, and returns
/// within its budget even when the model never answers — the boot catch-up
/// learns what it did not finish.
#[tokio::test(start_paused = true)]
async fn drain_returns_within_its_budget() {
    let scene = Scene::new(vec![Say::Hang]);
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");
    scene.finish_turn(&s, true);

    let started = tokio::time::Instant::now();
    let finished = scene.queue.drain(SHUTDOWN_DRAIN).await;
    assert!(!finished, "the model never answered");
    assert!(started.elapsed() <= SHUTDOWN_DRAIN + std::time::Duration::from_millis(100));
    assert!(
        scene
            .queue
            .shutdown(std::time::Duration::from_secs(3))
            .await,
        "the stuck flush stops on the shutdown token"
    );
    assert_eq!(
        scene.cursor(&s),
        0,
        "nothing lost: the window is still there"
    );
}

/// The drain with a model that answers: it learns what is pending — ignoring
/// the quiet rule, since the server has already stopped (a turn ended just
/// now here) — and reports that it finished.
#[tokio::test]
async fn drain_learns_the_pending_window_and_reports_done() {
    let scene = Scene::new(vec![items(&[vim_item(&[1])])]);
    let s = scene.session();
    let (_, answer) = scene.turn(&s, PREF, "Noted.");
    scene.finish_turn(&s, true);

    assert!(
        scene.queue.drain(SHUTDOWN_DRAIN).await,
        "the drain finished"
    );
    assert_eq!(scene.cursor(&s), answer, "the window was learned");
    assert_eq!(scene.titles(), vec!["Editor preference".to_string()]);
    assert!(!scene.queue.is_due(&s));
}

/// The durable half of "before the daemon stops": at boot, every session
/// with a user-authored row past its cursor is due — and no other.
#[tokio::test]
async fn boot_catchup_finds_unlearned_sessions() {
    let scene = Scene::new(vec![]);
    let unlearned = scene.session();
    scene.turn(&unlearned, PREF, "Noted.");
    let learned = scene.session_on("user1:cli");
    let (_, answer) = scene.turn(&learned, "I use a standing desk.", "Nice.");
    scene.set_cursor(&learned, answer);
    let followup_only = scene.session_on("user1:telegram");
    let request = uuid::Uuid::new_v4().to_string();
    scene.row(
        &followup_only,
        "user",
        "a queued task",
        INTERNAL_SOURCE,
        Some(&request),
        None,
    );

    scene.queue.catch_up().await;
    assert!(scene.queue.is_due(&unlearned));
    assert!(!scene.queue.is_due(&learned));
    assert!(!scene.queue.is_due(&followup_only));
}

/// The daemon's path end to end: in `Automatic` mode the sweeper starts a
/// due session only once no turn is in flight, and learns it.
#[tokio::test]
async fn the_sweeper_starts_a_due_session_once_the_daemon_is_quiet() {
    let model = Scripted::new(vec![items(&[vim_item(&[1])])]);
    let router = Arc::new(LlmRouter::single_provider(
        model.clone(),
        ProviderType::Anthropic,
        RECORDED_MODEL.to_string(),
    ));
    let scene = Scene::build(model, router, DaemonConfig::default(), QueueMode::Automatic);
    scene.no_quiet_period();
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");
    for _ in 0..10 {
        scene.finish_turn(&s, true);
    }
    assert!(scene.queue.is_due(&s));

    let chat = scene.queue.turn_started(uuid::Uuid::new_v4(), LANE, true);
    scene.queue.sweep().await;
    scene.queue.wait_idle().await;
    assert_eq!(scene.model.calls(), 0, "a turn in flight: nothing starts");
    drop(chat);

    scene.queue.sweep().await;
    scene.queue.wait_idle().await;
    assert_eq!(scene.titles(), vec!["Editor preference".to_string()]);
    assert!(!scene.queue.is_due(&s), "learned");
}

/// A flush that stops without progress — a cap, the rate headroom, a busy
/// daemon — waits for the next sweep instead of restarting at once: a
/// capped session is tried once per tick, not in a loop.
#[tokio::test]
async fn a_flush_that_made_no_progress_waits_for_the_next_sweep() {
    let mut config = DaemonConfig::default();
    config.orchestrator.costs.extract_max_calls_per_day = 1;
    let model = Scripted::new(vec![]);
    let router = Arc::new(LlmRouter::single_provider(
        model.clone(),
        ProviderType::Anthropic,
        RECORDED_MODEL.to_string(),
    ));
    let scene = Scene::build(model, router, config, QueueMode::Automatic);
    scene.no_quiet_period();
    openalpaca_storage::LlmUsageRepository::new(&scene.db)
        .upsert_daily_usage(&usage_row(chrono::Utc::now().date_naive(), 1, 0.0))
        .unwrap();
    let s = scene.session();
    scene.turn(&s, PREF, "Noted.");
    scene.queue.catch_up().await;
    tokio::time::timeout(std::time::Duration::from_secs(5), scene.queue.wait_idle())
        .await
        .expect("a capped flush restarted itself in a loop");
    assert!(scene.queue.is_due(&s), "still queued for the next sweep");
    assert_eq!(scene.model.calls(), 0);
}

// ── Richer extraction (design 09 §2, slice L2) ──────────────────────────

/// Rule 8: a key that breaks the grammar is dropped and the item kept.
#[tokio::test]
async fn an_invalid_fact_key_drops_the_key_not_the_item() {
    let scene = Scene::new(vec![items(&[full_item(
        serde_json::json!({"fact_key": "Editor Preference!"}),
    )])]);
    let s = scene.session();
    scene.turn(&s, "I edit with Helix these days.", "Noted.");
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(
        (report.stored, report.dropped_invalid),
        (1, 0),
        "{report:?}"
    );
    let note = scene.read(&scene.user_ctx(), scene.active()[0].id);
    assert_eq!(note.frontmatter["fact_key"], serde_json::Value::Null);
}

/// Rule 6 of the prompt and §2.5: a key reused from `known_keys` is kept as
/// is, and — its winner being a learned note the prompt listed, whose value
/// changed — the new note replaces it automatically (MD-11, LR11).
#[tokio::test]
async fn a_reused_key_is_kept() {
    let scene = Scene::new(vec![items(&[full_item(serde_json::json!({
        "fact_key": "user.editor",
        "content": "Since June the user edits with Helix; before that, vim.",
    }))])]);
    let old = scene.learned(
        "Editor preference",
        "The user edits with vim.",
        Some("user.editor"),
    );
    let s = scene.session();
    scene.turn(
        &s,
        "I switched my editor from vim to Helix in June.",
        "Noted.",
    );
    let report = scene.queue.flush_session(&s).await;
    let prompt = &scene.model.prompts()[0];
    assert!(
        prompt.contains("<known_keys>user.editor</known_keys>"),
        "{prompt}"
    );
    assert!(
        prompt.contains("n1 key=user.editor origin=learned"),
        "{prompt}"
    );
    assert_eq!((report.stored, report.replaced), (1, 1), "{report:?}");

    let ctx = scene.user_ctx();
    let new = scene
        .active()
        .into_iter()
        .find(|i| i.id != old.id)
        .expect("the replacement");
    let note = scene.read(&ctx, new.id);
    assert_eq!(
        note.frontmatter["fact_key"],
        serde_json::json!("user.editor")
    );
    assert_eq!(note.supersedes, vec![old.id]);
    assert_eq!(
        scene.read(&ctx, old.id).effective_status,
        EffectiveStatus::Superseded
    );
}

/// Rule 9: a `replaces` that names no listed note is dropped and counted;
/// the item is kept, as a new note.
#[tokio::test]
async fn an_unknown_replaces_ref_is_ignored() {
    let scene = Scene::new(vec![items(&[full_item(
        serde_json::json!({"replaces": "n7", "replace_reason": "a guess"}),
    )])]);
    let s = scene.session();
    scene.turn(&s, "I edit with Helix these days.", "Noted.");
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(
        (report.stored, report.replaces_unknown, report.replaced),
        (1, 1, 0),
        "{report:?}"
    );
    let note = scene.read(&scene.user_ctx(), scene.active()[0].id);
    assert!(note.supersedes.is_empty());
    assert!(!note.body.contains("Replaces"), "{}", note.body);
}

/// §2.5: `observed_at` is the `created_at` of the item's newest cited row,
/// and the evidence line dates the note by it.
#[tokio::test]
async fn observed_at_is_the_newest_cited_rows_time() {
    let scene = Scene::new(vec![items(&[full_item(serde_json::json!({
        "title": "Commute",
        "content": "The user cycles to work, and has since September.",
        "kind": "fact",
        "rows": [3, 1],
    }))])]);
    let s = scene.session();
    let (first, _) = scene.turn(&s, "I cycle to work now.", "Nice.");
    let (second, _) = scene.turn(&s, "Since September, every day.", "Great.");
    scene.set_created_at(first, "2026-09-20 10:00:00");
    scene.set_created_at(second, "2026-09-21 11:30:00");
    scene.queue.flush_session(&s).await;
    let note = scene.read(&scene.user_ctx(), scene.active()[0].id);
    assert_eq!(
        note.frontmatter["observed_at"],
        serde_json::json!("2026-09-21T11:30:00Z")
    );
    assert_eq!(
        note.sources
            .iter()
            .map(|s| s.observed_at.map(|t| t.to_rfc3339()))
            .collect::<Vec<_>>(),
        vec![
            Some("2026-09-20T10:00:00+00:00".to_string()),
            Some("2026-09-21T11:30:00+00:00".to_string())
        ]
    );
    assert!(
        note.body.contains("From what you said on 2026-09-21."),
        "{}",
        note.body
    );
}

/// Rule 10 (§2.2): a date-only `valid_to` is the next day's midnight, so
/// the stated last day is inside the half-open window; an inverted pair
/// drops both bounds and keeps the item.
#[tokio::test]
async fn a_date_only_valid_to_includes_that_day() {
    let scene = Scene::new(vec![items(&[
        full_item(serde_json::json!({
            "title": "Berlin trip",
            "content": "The user is in Berlin from 1 to 5 October 2026.",
            "kind": "episode",
            "valid_from": "2026-10-01",
            "valid_to": "2026-10-05",
        })),
        full_item(serde_json::json!({
            "title": "Rome trip",
            "content": "The user goes to Rome in November 2026.",
            "kind": "episode",
            "valid_from": "2026-11-09",
            "valid_to": "2026-11-02",
        })),
    ])]);
    let s = scene.session();
    scene.turn(
        &s,
        "I'm in Berlin 1-5 October, then Rome in November.",
        "Noted.",
    );
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.stored, 2, "{report:?}");
    let ctx = scene.user_ctx();
    let by_title = |t: &str| {
        let id = scene
            .active()
            .into_iter()
            .find(|i| i.title == t)
            .unwrap()
            .id;
        scene.read(&ctx, id)
    };
    let berlin = by_title("Berlin trip");
    assert_eq!(
        berlin.frontmatter["valid_from"],
        serde_json::json!("2026-10-01T00:00:00Z")
    );
    assert_eq!(
        berlin.frontmatter["valid_to"],
        serde_json::json!("2026-10-06T00:00:00Z")
    );
    let rome = by_title("Rome trip");
    assert_eq!(rome.frontmatter["valid_from"], serde_json::Value::Null);
    assert_eq!(rome.frontmatter["valid_to"], serde_json::Value::Null);
}

/// Rule 11: two items with one key keep the one whose newest cited row is
/// latest.
#[tokio::test]
async fn two_items_with_one_key_keep_the_latest() {
    let scene = Scene::new(vec![items(&[
        full_item(serde_json::json!({
            "fact_key": "user.editor",
            "content": "The user edits with vim.",
            "rows": [3],
        })),
        full_item(serde_json::json!({
            "fact_key": "user.editor",
            "content": "The user edits with Helix.",
            "rows": [1],
        })),
    ])]);
    let s = scene.session();
    scene.turn(&s, "I used to edit with Helix.", "Noted.");
    scene.turn(&s, "Actually I edit with vim again.", "Noted.");
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(
        (report.stored, report.dropped_duplicate_key),
        (1, 1),
        "{report:?}"
    );
    let note = scene.read(&scene.user_ctx(), scene.active()[0].id);
    assert!(
        note.body.contains("The user edits with vim."),
        "{}",
        note.body
    );
}

/// Rule 11, second pass: one item names the listed note by `replaces`, the
/// other reaches the same note by its key; one survives — the later.
#[tokio::test]
async fn two_items_reaching_one_target_by_different_routes_keep_one() {
    let scene = Scene::new(vec![items(&[
        full_item(serde_json::json!({
            "title": "Editor",
            "content": "The user edits with Emacs.",
            "rows": [1],
            "replaces": "n1",
        })),
        full_item(serde_json::json!({
            "content": "The user edits with Helix.",
            "fact_key": "user.editor",
            "rows": [3],
        })),
    ])]);
    let old = scene.learned(
        "Editor preference",
        "The user edits with vim.",
        Some("user.editor"),
    );
    let s = scene.session();
    scene.turn(&s, "I tried Emacs instead of vim for my editor.", "Noted.");
    scene.turn(&s, "No, Helix is my editor now.", "Noted.");
    let report = scene.queue.flush_session(&s).await;
    assert!(
        scene.model.prompts()[0].contains("n1 key=user.editor"),
        "the learned note is listed as n1"
    );
    assert_eq!(
        (report.stored, report.replaced, report.dropped_duplicate_key),
        (1, 1, 1),
        "{report:?}"
    );
    let active = scene.active();
    assert_eq!(active.len(), 1, "{active:?}");
    let note = scene.read(&scene.user_ctx(), active[0].id);
    assert!(note.body.contains("Helix"), "{}", note.body);
    assert_eq!(note.supersedes, vec![old.id]);
}

/// §2.5, case 2 for a saved note: the prompt listed it, the model reused
/// its key without calling it replaced — a restatement, dropped as
/// `unchanged`, and no suggestion is raised.
#[tokio::test]
async fn a_restated_listed_saved_fact_is_unchanged() {
    let scene = Scene::new(vec![items(&[full_item(serde_json::json!({
        "title": "Laptop",
        "content": "The user works on a ThinkPad X1 laptop.",
        "kind": "fact",
        "fact_key": "user.laptop",
    }))])]);
    let saved = scene.save(
        "Work laptop",
        "The user's work laptop is a ThinkPad X1.",
        Some("user.laptop"),
    );
    let s = scene.session();
    scene.turn(&s, "My work laptop is a ThinkPad X1, by the way.", "Noted.");
    let report = scene.queue.flush_session(&s).await;
    assert!(
        scene.model.prompts()[0].contains("origin=saved-by-you"),
        "the saved note is listed"
    );
    assert_eq!(
        (report.stored, report.dropped_unchanged, report.suggestions),
        (0, 1, 0),
        "{report:?}"
    );
    let active = scene.active();
    assert_eq!(active.len(), 1);
    assert_eq!(
        (active[0].id, &active[0].revision),
        (saved.id, &saved.revision)
    );
}

/// §2.5, case 2 for a learned note: equal knowledge after normalisation —
/// case, spacing, trailing punctuation — is a restatement.
#[tokio::test]
async fn a_learned_restatement_with_equal_content_is_unchanged() {
    let scene = Scene::new(vec![items(&[full_item(serde_json::json!({
        "title": "Editor",
        "content": "the user  edits with VIM",
        "fact_key": "user.editor",
    }))])]);
    let learned = scene.learned(
        "Editor preference",
        "The user edits with vim.",
        Some("user.editor"),
    );
    let s = scene.session();
    scene.turn(&s, "I edit with vim, as always.", "Noted.");
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(
        (report.stored, report.dropped_unchanged),
        (0, 1),
        "{report:?}"
    );
    assert_eq!(
        scene.active().into_iter().map(|i| i.id).collect::<Vec<_>>(),
        vec![learned.id]
    );
}

/// §2.5, case 3: the note the key names was never shown to the model — a
/// change in new words is what the lexical lookup misses — so it is a
/// change, and against a note the user saved it is a suggestion (MD-11,
/// G2): the saved note stays exactly as it was. S1 and S2 (L3) write the
/// suggestion; until then it is decided and counted, never applied.
#[tokio::test]
async fn a_change_against_an_unlisted_saved_note_is_a_suggestion() {
    let scene = Scene::new(vec![items(&[full_item(serde_json::json!({
        "title": "Commute",
        "content": "Since March 2026 the user cycles to the office; before that, the bus.",
        "kind": "fact",
        "fact_key": "user.commute",
    }))])]);
    let saved = scene.save("Commute", "The user commutes by bus.", Some("user.commute"));
    let s = scene.session();
    scene.turn(&s, "Since March I cycle to the office.", "Noted.");
    let report = scene.queue.flush_session(&s).await;
    assert!(
        !scene.model.prompts()[0].contains("\"Commute\""),
        "the saved note was not listed: {}",
        scene.model.prompts()[0]
    );
    assert_eq!(
        (report.suggestions, report.stored, report.dropped_unchanged),
        (1, 0, 0),
        "{report:?}"
    );
    let active = scene.active();
    assert_eq!(active.len(), 1, "{active:?}");
    assert_eq!(
        (active[0].id, &active[0].revision),
        (saved.id, &saved.revision)
    );
    assert_eq!(active[0].origin, Origin::User);
    assert!(
        scene.candidates().is_empty(),
        "nothing is written before L3"
    );
}

/// Rulings LR10, LR11: a learned `user.*` fact restated with a new value
/// in a project chat replaces the **global** note — its key is looked up in
/// both of the chunk's partitions, and the replacement is written in its
/// target's — instead of piling up a second note in the project.
#[tokio::test]
async fn a_global_fact_restated_in_a_project_session_replaces_it() {
    let (_dir, root) = project();
    let root_str = root.to_string_lossy().to_string();
    let scene = Scene::new(vec![items(&[full_item(serde_json::json!({
        "title": "Work laptop",
        "content": "The user's work laptop is now a MacBook Pro; before, a ThinkPad X1.",
        "kind": "fact",
        "fact_key": "user.laptop",
        "replace_reason": "The user said they got a new work laptop.",
    }))])]);
    let old = scene.learned(
        "Work laptop",
        "The user's work laptop is a ThinkPad X1.",
        Some("user.laptop"),
    );
    let s = scene.session();
    scene.turn_in(
        &s,
        "I got a new work laptop: a MacBook Pro.",
        "Noted.",
        Some(&root_str),
    );
    let report = scene.queue.flush_session(&s).await;
    assert_eq!((report.stored, report.replaced), (1, 1), "{report:?}");

    let in_project = scene.ctx_in(Some(&root));
    let project_notes: Vec<ListItem> = scene
        .list(&in_project, None)
        .into_iter()
        .filter(|i| i.scope == NoteScope::Project)
        .collect();
    assert!(project_notes.is_empty(), "{project_notes:?}");
    let new = scene
        .active()
        .into_iter()
        .find(|i| i.id != old.id)
        .expect("the replacement");
    assert_eq!(new.scope, NoteScope::Global);
    let note = scene.read(&scene.user_ctx(), new.id);
    assert_eq!(note.supersedes, vec![old.id]);
    assert!(
        note.body.contains(
            "Replaces “Work laptop” (learned): The user said they got a new work laptop."
        ),
        "{}",
        note.body
    );
    assert_eq!(
        scene.read(&scene.user_ctx(), old.id).effective_status,
        EffectiveStatus::Superseded
    );
}

/// Ruling LR17: a learned note whose key's subject is `user` and that
/// replaces nothing is global whatever its kind — a `fact` learned in a
/// project chat is still the user's — while a project's own key stays in
/// the project.
#[tokio::test]
async fn a_user_fact_first_learned_in_a_project_is_global() {
    let (_dir, root) = project();
    let root_str = root.to_string_lossy().to_string();
    let scene = Scene::new(vec![items(&[
        full_item(serde_json::json!({
            "title": "Work laptop",
            "content": "The user's work laptop is a ThinkPad X1.",
            "kind": "fact",
            "fact_key": "user.laptop",
        })),
        full_item(serde_json::json!({
            "title": "Staging database",
            "content": "Lantern's staging database is Postgres 16.",
            "kind": "fact",
            "fact_key": "lantern.staging.db",
        })),
    ])]);
    let s = scene.session();
    scene.turn_in(
        &s,
        "My work laptop is a ThinkPad X1, and staging runs Postgres 16.",
        "Noted.",
        Some(&root_str),
    );
    let report = scene.queue.flush_session(&s).await;
    assert_eq!(report.stored, 2, "{report:?}");
    let notes = scene.list(&scene.ctx_in(Some(&root)), None);
    let scope_of = |t: &str| notes.iter().find(|i| i.title == t).unwrap().scope;
    assert_eq!(scope_of("Work laptop"), NoteScope::Global);
    assert_eq!(scope_of("Staging database"), NoteScope::Project);
}

/// Guard (b) needs equal knowledge, not only the key: a re-run whose item
/// keeps the key but changes the value corrects the earlier copy (guard
/// (c)) — one note stays recallable, and it is the new one.
#[tokio::test]
async fn already_learned_needs_equal_content_not_only_the_key() {
    let scene = Scene::new(vec![
        items(&[full_item(serde_json::json!({
            "fact_key": "user.editor",
            "content": "The user edits with vim.",
        }))]),
        items(&[full_item(serde_json::json!({
            "fact_key": "user.editor",
            "content": "The user edits with Helix.",
        }))]),
    ]);
    let s = scene.session();
    scene.turn(&s, "I edit with Helix, not vim.", "Noted.");
    scene.queue.set_store_hooks(StoreHooks {
        skip_advance: true,
        ..StoreHooks::default()
    });
    let first = scene.queue.flush_session(&s).await;
    assert_eq!(first.stored, 1, "{first:?}");
    let earlier = scene.active()[0].id;

    scene.queue.set_store_hooks(StoreHooks::default());
    let rerun = scene.queue.flush_session(&s).await;
    assert_eq!(
        (rerun.dropped_already_learned, rerun.stored, rerun.replaced),
        (0, 1, 1),
        "{rerun:?}"
    );
    let active = scene.active();
    assert_eq!(active.len(), 1, "{active:?}");
    assert_ne!(active[0].id, earlier);
    let note = scene.read(&scene.user_ctx(), active[0].id);
    assert_eq!(note.supersedes, vec![earlier]);
}

/// Guard (b) over candidates: exact dedup covers effectively active notes
/// only, so a re-run of a chunk whose item was inferred used to write a
/// second, identical candidate. The lookup finds this window's candidate
/// through `note_source`.
#[tokio::test]
async fn a_rerun_never_duplicates_an_inferred_candidate() {
    let inferred = || {
        items(&[item(
            "Probably uses Rust",
            "The user's project is probably written in Rust.",
            "fact",
            "assistant",
            &[2],
        )])
    };
    let scene = Scene::new(vec![inferred(), inferred()]);
    let s = scene.session();
    scene.turn(
        &s,
        "Can you look at my project and tell me what it does?",
        "It looks like a Rust workspace.",
    );
    scene.queue.set_store_hooks(StoreHooks {
        skip_advance: true,
        ..StoreHooks::default()
    });
    let first = scene.queue.flush_session(&s).await;
    assert_eq!(first.candidates, 1, "{first:?}");
    scene.queue.set_store_hooks(StoreHooks::default());
    let rerun = scene.queue.flush_session(&s).await;
    assert_eq!(
        (rerun.stored, rerun.dropped_already_learned),
        (0, 1),
        "{rerun:?}"
    );
    assert_eq!(scene.candidates().len(), 1);
}

/// §2.6: the body follows design 03 §4's sections. `## Knowledge` holds
/// exactly the item's content; `## Evidence and limits` says whose words
/// it was and when.
#[tokio::test]
async fn the_note_body_has_knowledge_and_evidence_sections() {
    let scene = Scene::new(vec![items(&[
        vim_item(&[1]),
        item(
            "Probably uses Rust",
            "The user's project is probably written in Rust.",
            "fact",
            "assistant",
            &[2],
        ),
    ])]);
    let s = scene.session();
    let (question, answer) = scene.turn(&s, PREF, "It looks like a Rust workspace.");
    scene.set_created_at(question, "2026-09-24 10:02:11");
    scene.set_created_at(answer, "2026-09-24 10:02:40");
    scene.queue.flush_session(&s).await;
    let ctx = scene.user_ctx();
    let learned = scene.read(&ctx, scene.active()[0].id);
    assert_eq!(
        learned.body,
        "# Editor preference\n\n## Knowledge\nThe user prefers vim for editing.\n\n## Evidence and limits\nFrom what you said on 2026-09-24.\n"
    );
    let inferred = scene.read(&ctx, scene.candidates()[0].id);
    assert_eq!(
        inferred.body,
        "# Probably uses Rust\n\n## Knowledge\nThe user's project is probably written in Rust.\n\n## Evidence and limits\nInferred from the conversation on 2026-09-24; not said by you.\n"
    );
}

/// §2.3 step 3 end to end: a note named only at the end of a long row is
/// still looked up — the row's last window is searched — and listed.
#[tokio::test]
async fn a_note_named_late_in_a_long_row_is_listed() {
    let scene = Scene::new(vec![]);
    scene.save(
        "Espresso machine",
        "The user owns a Gaggia espresso machine.",
        None,
    );
    let s = scene.session();
    let message = format!(
        "{} and finally the gaggia broke.",
        "Here is the whole story of my week at work. ".repeat(60)
    );
    assert!(message.chars().count() > 4 * 512);
    scene.turn(&s, &message, "Sorry to hear.");
    scene.queue.flush_session(&s).await;
    assert!(
        scene.model.prompts()[0].contains("\"Espresso machine\""),
        "{}",
        scene.model.prompts()[0]
    );
}

/// The prompt is the committed fixture (§2.1): one fixed chunk, with its
/// context rows, known notes and keys, renders byte for byte to the golden
/// file beside it.
#[test]
fn the_extractor_prompt_matches_its_fixture() {
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
    let before = [
        row(1, "user", "I edit with vim.", "a", "2026-09-24 10:00:03"),
        row(2, "assistant", "Noted: vim.", "a", "2026-09-24 10:00:20"),
    ];
    let rows = [
        row(
            3,
            "user",
            "I switched to Helix in June, and I commute by train now.",
            "b",
            "2026-09-24 10:02:11",
        ),
        row(
            4,
            "assistant",
            "Got it: Helix, and the train.",
            "b",
            "2026-09-24 10:02:40",
        ),
    ];
    let window::NextChunk::Chunk(chunk) = window::next_chunk(
        &rows,
        &before,
        &std::collections::HashSet::new(),
        24 * 1024,
        400,
    ) else {
        panic!("a chunk");
    };
    let known = |title: &str, knowledge: &str, key: Option<&str>, origin: Origin| {
        crate::memory::service::KnownNote {
            id: NoteId::new(),
            revision: "0".repeat(64),
            partition_key: "home-x".into(),
            scope: NoteScope::Global,
            title: title.into(),
            kind: NoteKind::Preference,
            origin,
            effective_status: EffectiveStatus::Active,
            fact_key: key.map(str::to_string),
            knowledge: knowledge.into(),
            supersedes: Vec::new(),
            from_this_window: false,
        }
    };
    let view = crate::memory::service::LearningView {
        notes: vec![
            known(
                "Editor preference",
                "The user edits with vim.",
                Some("user.editor"),
                Origin::ExtractedUser,
            ),
            known(
                "Commute",
                "The user commutes by bus.",
                Some("user.commute"),
                Origin::User,
            ),
        ],
        from_this_window: Vec::new(),
        keys: vec![
            "user.editor".into(),
            "user.commute".into(),
            "user.laptop".into(),
        ],
    };
    let lookup = lookup::render(view, 8 * 1024);
    let (transcript, _) = chunk.transcript();
    let today = chunk.today();
    let (system, user) = extractor::render_prompt(&extractor::PromptInput {
        today: &today,
        known_notes: &lookup.notes_text,
        known_keys: &lookup.keys_text,
        conversation: &transcript,
        max_items: 12,
    });
    let rendered = format!("{system}\n=== rendered user message ===\n{user}\n");
    assert!(
        !rendered.contains("{{"),
        "a placeholder was left: {rendered}"
    );
    assert_eq!(rendered, include_str!("extractor_prompt.golden.txt"));
}
