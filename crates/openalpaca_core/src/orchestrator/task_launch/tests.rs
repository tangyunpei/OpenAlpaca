//! GAP-06 — `rerun` (a new id) and `start` (D5: the same id).

use super::*;
use crate::agent::subagent::SubAgent;
use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::daemon_config::DaemonConfig;
use crate::lane::LaneManager;
use crate::middleware::prompt::SystemPersona;
use crate::runner::LoopConfig;
use crate::security::gate::SecurityGate;
use crate::security::sandbox::SandboxManager;
use crate::test_util::{make_agent, template_from_agent};
use crate::tools::ToolRegistry;
use arc_swap::ArcSwap;
use async_trait::async_trait;
use chrono::Utc;
use openalpaca_llm::{
    ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, LlmRouter, ProviderType, Usage,
};
use openalpaca_storage::Database;
use std::sync::Arc;

/// A provider that answers anything with one short line. The lead agent never
/// gets far enough to matter here — what these tests need is a *present*
/// router, so the dispatch takes the same path production does.
struct StubLlm;

#[async_trait]
impl LlmProvider for StubLlm {
    fn name(&self) -> &str {
        "stub"
    }
    fn supports_tools(&self) -> bool {
        false
    }
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Ok(ChatResponse {
            content: "done".to_string(),
            tool_calls: vec![],
            model: "stub-model".to_string(),
            usage: Usage::default(),
            finish_reason: FinishReason::Stop,
            thinking: None,
            parts: None,
        })
    }
}

/// An orchestrator over a temp database, with a lead-agent template unless
/// `agents` says otherwise, and a router unless `with_router` is false.
fn make_orchestrator(
    db: &Database,
    agents: Vec<SubAgent>,
    with_router: bool,
) -> (Orchestrator, Arc<SharedContext>) {
    let ctx = Arc::new(SharedContext::new());
    for agent in &agents {
        ctx.agent_registry
            .register_template(template_from_agent(agent));
        ctx.agent_registry.register(agent.clone());
    }
    let bus = EventBus::default();
    let registry = Arc::new(ToolRegistry::default());
    let gate = Arc::new(SecurityGate::new(Arc::new(SandboxManager::with_defaults(
        registry.clone(),
        bus.clone(),
    ))));
    let router = with_router.then(|| {
        Arc::new(LlmRouter::single_provider(
            Arc::new(StubLlm),
            ProviderType::Anthropic,
            "stub-model".to_string(),
        ))
    });
    let orchestrator = Orchestrator::new(
        ctx.clone(),
        Arc::new(LaneManager::new()),
        bus,
        SystemPersona::default(),
        router,
        LoopConfig::default(),
        gate,
        registry,
        Some(db.clone()),
        None,
        Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new()),
        Arc::new(crate::orchestrator::skill_router::SkillRouter::new(
            0.65, 0.45,
        )),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
    );
    (orchestrator, ctx)
}

/// The usual shape: one lead-agent template, one router, a temp database.
fn ready(db: &Database) -> (Orchestrator, Arc<SharedContext>) {
    make_orchestrator(
        db,
        vec![make_agent("lead_agent", vec!["orchestration"])],
        true,
    )
}

fn temp_db() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Database::open(&dir.path().join("test.db")).expect("open db");
    (dir, db)
}

/// A stored run: `status`, a goal unless `description` says otherwise, on lane
/// `user-1:gui`.
fn store_task(db: &Database, id: &str, status: TaskStatus, description: Option<&str>) -> Task {
    let task = store_task_row(id, status, description, Utc::now());
    TaskRepository::new(db).create(&task).expect("create task");
    task
}

/// A run that has already finished and has something to lose: a summary, a
/// completion time and a state version that a re-launch under the same id
/// would wipe (`upsert_queued`'s conflict tail).
fn store_finished_task(db: &Database, id: &str, status: TaskStatus) -> Task {
    let now = Utc::now();
    let task = Task {
        result_summary: Some("the first run's answer".to_string()),
        state_json: Some("{\"steps\":[]}".to_string()),
        state_version: 7,
        artifact_count: 3,
        ..store_task_row(id, status, Some("write the changelog"), now)
    };
    TaskRepository::new(db).create(&task).expect("create task");
    task
}

/// The row [`store_task`] writes, unwritten — so a variant can change a field
/// or two without repeating twenty.
fn store_task_row(
    id: &str,
    status: TaskStatus,
    description: Option<&str>,
    now: chrono::DateTime<Utc>,
) -> Task {
    Task {
        id: id.to_string(),
        title: "Ship the release".to_string(),
        description: description.map(str::to_string),
        status,
        priority: 0,
        progress_current: None,
        progress_total: None,
        result_summary: None,
        created_by: "user-1".to_string(),
        source_lane: "user-1:gui".to_string(),
        created_at: now,
        updated_at: now,
        completed_at: status.is_terminal().then_some(now),
        state_json: None,
        state_version: 0,
        outcome_json: None,
        outcome_kind: None,
        artifact_count: 0,
        workspace_id: None,
        source_task_id: None,
        session_id: None,
    }
}

// ── rerun: a new run, linked back to the old one ──────────────────────

/// The whole point of the asymmetry with `start`: a re-run is a **different**
/// run. The original row keeps its result — it is what the user is comparing
/// against — and the copy records where it came from, in the column rather than
/// only in the response, so the link survives a restart.
#[tokio::test]
async fn rerun_dispatches_a_new_run_that_records_the_one_it_came_from() {
    let (_dir, db) = temp_db();
    store_task(
        &db,
        "t1",
        TaskStatus::Completed,
        Some("write the changelog"),
    );
    let (orchestrator, _ctx) = ready(&db);

    let outcome = orchestrator.rerun_task("t1").expect("re-run dispatched");

    assert_ne!(outcome.task_id, "t1", "a re-run is a new run");
    assert_eq!(outcome.source_task_id, "t1");
    assert_eq!(outcome.title, "Ship the release");

    let repo = TaskRepository::new(&db);
    let copy = repo.get(&outcome.task_id).unwrap().expect("the new row");
    assert_eq!(copy.source_task_id.as_deref(), Some("t1"));
    assert_eq!(copy.description.as_deref(), Some("write the changelog"));
    assert_eq!(copy.title, "Ship the release");
    // The lane, the creator and the project travel with the goal, so the new
    // run reports back where the old one did.
    assert_eq!(copy.created_by, "user-1");
    assert_eq!(copy.source_lane, "user-1:gui");

    // The original is untouched: still finished, still nobody's copy.
    let original = repo.get("t1").unwrap().unwrap();
    assert_eq!(original.status, TaskStatus::Completed);
    assert_eq!(original.source_task_id, None);
}

/// Re-running work that is still in flight would put a second lead agent on the
/// same goal. The refusal names the state so the client can say which.
#[tokio::test]
async fn rerun_refuses_a_run_that_has_not_finished() {
    let (_dir, db) = temp_db();
    for (id, status, word) in [
        ("t-queued", TaskStatus::Queued, "queued"),
        ("t-running", TaskStatus::Running, "running"),
        ("t-paused", TaskStatus::Paused, "paused"),
    ] {
        store_task(&db, id, status, Some("do the thing"));
        let (orchestrator, _ctx) = ready(&db);
        assert_eq!(
            orchestrator.rerun_task(id),
            Err(TaskLaunchError::NotTerminal { current: word })
        );
    }
    // Nothing was dispatched: still three rows.
    assert_eq!(TaskRepository::new(&db).list_recent(10).unwrap().len(), 3);
}

/// A `POST /v1/tasks` row may be title-only. There is no goal to hand a lead
/// agent, and a run seeded from a title alone would be a different task.
#[tokio::test]
async fn rerun_refuses_a_row_with_nothing_to_re_dispatch() {
    let (_dir, db) = temp_db();
    store_task(&db, "t-none", TaskStatus::Completed, None);
    store_task(&db, "t-blank", TaskStatus::Completed, Some("   \n\t "));
    let (orchestrator, _ctx) = ready(&db);

    assert_eq!(
        orchestrator.rerun_task("t-none"),
        Err(TaskLaunchError::NoDescription)
    );
    assert_eq!(
        orchestrator.rerun_task("t-blank"),
        Err(TaskLaunchError::NoDescription)
    );
    assert_eq!(TaskRepository::new(&db).list_recent(10).unwrap().len(), 2);
}

#[tokio::test]
async fn rerun_of_an_unknown_run_is_not_found() {
    let (_dir, db) = temp_db();
    let (orchestrator, _ctx) = ready(&db);
    assert_eq!(
        orchestrator.rerun_task("no-such-run"),
        Err(TaskLaunchError::NotFound)
    );
}

// ── start: the same id (D5) ───────────────────────────────────────────

/// D5 — the client keeps the id it already has. There is one row before and
/// one row after, and it is the same row.
#[tokio::test]
async fn start_runs_the_stored_row_under_its_own_id() {
    let (_dir, db) = temp_db();
    store_task(&db, "t1", TaskStatus::Queued, Some("write the changelog"));
    let (orchestrator, _ctx) = ready(&db);

    let outcome = orchestrator.start_task("t1").expect("dispatched");
    assert_eq!(outcome.task_id, "t1", "D5: the id does not change");
    assert_eq!(outcome.title, "Ship the release");
    assert!(
        matches!(outcome.status.as_str(), "queued" | "running"),
        "a dispatched run is queued or already running, got {}",
        outcome.status,
    );

    let repo = TaskRepository::new(&db);
    assert_eq!(repo.list_recent(10).unwrap().len(), 1, "no second row");
    let row = repo.get("t1").unwrap().unwrap();
    // `start` links nothing: there is only one run, so there is nothing for the
    // provenance column to point at.
    assert_eq!(row.source_task_id, None);
    assert_eq!(row.description.as_deref(), Some("write the changelog"));
}

/// The refusal that keeps two lead agents off one task id. The claim is taken
/// before the dispatch, so the second call loses even if it arrives while the
/// first is still setting up.
#[tokio::test]
async fn starting_the_same_run_twice_refuses_the_second() {
    let (_dir, db) = temp_db();
    store_task(&db, "t1", TaskStatus::Queued, Some("write the changelog"));
    let (orchestrator, _ctx) = ready(&db);

    assert!(orchestrator.start_task("t1").is_ok());
    assert_eq!(
        orchestrator.start_task("t1"),
        Err(TaskLaunchError::AlreadyRunning),
        "the id is already running",
    );
    assert_eq!(TaskRepository::new(&db).list_recent(10).unwrap().len(), 1);
}

/// A run started by some other path — the model's `start_workflow`, a chat turn
/// — holds the same slot, so `start` on it is the same refusal.
#[tokio::test]
async fn start_refuses_a_run_that_is_already_live() {
    let (_dir, db) = temp_db();
    store_task(&db, "t1", TaskStatus::Running, Some("write the changelog"));
    let (orchestrator, ctx) = ready(&db);
    ctx.register_cancellation_token("t1", tokio_util::sync::CancellationToken::new());

    assert_eq!(
        orchestrator.start_task("t1"),
        Err(TaskLaunchError::AlreadyRunning)
    );
}

/// R43 — a finished run is not startable. `start` re-launches a row **under
/// its own id**, and `upsert_queued`'s conflict tail clears the summary, the
/// outcome, the artifact count and the state version; doing that to a run that
/// already produced an answer destroys the answer with no record that it ever
/// existed. `rerun` is the verb for "run this goal again", and it keeps the
/// original row.
#[tokio::test]
async fn start_refuses_a_run_that_has_already_finished() {
    let (_dir, db) = temp_db();
    for (id, status, word) in [
        ("t-completed", TaskStatus::Completed, "completed"),
        ("t-failed", TaskStatus::Failed, "failed"),
        ("t-cancelled", TaskStatus::Cancelled, "cancelled"),
    ] {
        let before = store_finished_task(&db, id, status);
        let (orchestrator, ctx) = ready(&db);

        assert_eq!(
            orchestrator.start_task(id),
            Err(TaskLaunchError::NotStartable { current: word }),
            "a {word} run must not be re-launched in place",
        );

        // The refusal is total: the row still describes the run that finished.
        let after = TaskRepository::new(&db).get(id).unwrap().expect("the row");
        assert_eq!(after.status, status);
        assert_eq!(after.result_summary, before.result_summary);
        assert_eq!(after.completed_at.is_some(), before.completed_at.is_some());
        assert_eq!(after.state_version, before.state_version);
        assert_eq!(after.artifact_count, before.artifact_count);
        // …and nothing claimed the id on the way out.
        assert!(ctx.claim_run_slot(id), "a refusal must not hold the slot");
    }
    // No copy was dispatched either — `rerun` is what makes those.
    assert_eq!(TaskRepository::new(&db).list_recent(10).unwrap().len(), 3);
}

/// A paused run keeps its cancellation token — `pause` is a status change, not
/// a cancel — so it is still live and `start` answers the running refusal, not
/// R43's. Asserted because it holds by construction, and construction changes.
#[tokio::test]
async fn start_on_a_paused_run_is_still_the_already_running_refusal() {
    let (_dir, db) = temp_db();
    store_task(&db, "t1", TaskStatus::Paused, Some("write the changelog"));
    let (orchestrator, ctx) = ready(&db);
    ctx.register_cancellation_token("t1", tokio_util::sync::CancellationToken::new());

    assert_eq!(
        orchestrator.start_task("t1"),
        Err(TaskLaunchError::AlreadyRunning)
    );
}

/// R45's window, from the caller's side. Between `run_lead_agent` returning
/// and `finalize_task_with_outcome` the row still says `running` and the id
/// still holds its token, so a `start` arriving there is refused as already
/// running — it never reaches `upsert_queued` to re-queue the row under a
/// second lead agent. Once the tail finalises, the row is terminal and carries
/// *that* run's result, and `start` is refused again, now by R43. There is no
/// instant between the two refusals.
///
/// (What proves the ordering in production is
/// `dispatcher::tests::the_run_slot_is_held_until_the_row_is_terminal`; this
/// states what the ordering buys the caller.)
#[tokio::test]
async fn a_start_during_the_old_runs_finalisation_cannot_take_the_row() {
    let (_dir, db) = temp_db();
    store_task(&db, "t1", TaskStatus::Running, Some("write the changelog"));
    let (orchestrator, ctx) = ready(&db);
    ctx.register_cancellation_token("t1", tokio_util::sync::CancellationToken::new());

    assert_eq!(
        orchestrator.start_task("t1"),
        Err(TaskLaunchError::AlreadyRunning),
        "the tail is still writing to this row",
    );

    // …and now the tail finalises: the terminal status and the run's result.
    let repo = TaskRepository::new(&db);
    repo.set_result("t1", "the first run's answer").unwrap();
    repo.update_status("t1", TaskStatus::Completed).unwrap();
    ctx.remove_cancellation_token("t1");

    assert_eq!(
        orchestrator.start_task("t1"),
        Err(TaskLaunchError::NotStartable {
            current: "completed"
        }),
        "the slot is free, so R43 is what refuses now",
    );
    let row = repo.get("t1").unwrap().unwrap();
    assert_eq!(row.result_summary.as_deref(), Some("the first run's answer"));
    assert_eq!(repo.list_recent(10).unwrap().len(), 1);
}

#[tokio::test]
async fn start_of_an_unknown_run_is_not_found() {
    let (_dir, db) = temp_db();
    let (orchestrator, _ctx) = ready(&db);
    assert_eq!(
        orchestrator.start_task("no-such-run"),
        Err(TaskLaunchError::NotFound)
    );
}

#[tokio::test]
async fn start_refuses_a_row_with_nothing_to_dispatch() {
    let (_dir, db) = temp_db();
    store_task(&db, "t1", TaskStatus::Queued, None);
    let (orchestrator, _ctx) = ready(&db);
    assert_eq!(
        orchestrator.start_task("t1"),
        Err(TaskLaunchError::NoDescription)
    );
}

/// A claim that is not followed by a run has to be given back, or the id
/// answers "already running" for the rest of the daemon's life. Here the
/// dispatch fails because no template can lead the run.
#[tokio::test]
async fn a_start_that_could_not_dispatch_releases_the_run_slot() {
    let (_dir, db) = temp_db();
    store_task(&db, "t1", TaskStatus::Queued, Some("write the changelog"));
    let (orchestrator, ctx) = make_orchestrator(&db, vec![], true);

    assert!(matches!(
        orchestrator.start_task("t1"),
        Err(TaskLaunchError::Dispatch(_))
    ));
    assert!(
        ctx.claim_run_slot("t1"),
        "the slot must be free again after a dispatch that never started"
    );
}

/// …and so does a daemon with no LLM router at all: nothing will run under the
/// id, so nothing may keep holding it.
#[tokio::test]
async fn a_start_with_no_router_releases_the_run_slot() {
    let (_dir, db) = temp_db();
    store_task(&db, "t1", TaskStatus::Queued, Some("write the changelog"));
    let (orchestrator, ctx) = make_orchestrator(
        &db,
        vec![make_agent("lead_agent", vec!["orchestration"])],
        false,
    );

    // The dispatch itself succeeds — the run is registered and persisted — and
    // then dies for want of a router.
    assert!(orchestrator.start_task("t1").is_ok());
    assert!(
        ctx.claim_run_slot("t1"),
        "a run that could never start must not hold the id"
    );
}

// ── resume: the same id, re-primed from the log (§5.6c, S2) ───────────

/// A run with a session, and a session log holding one complete round of it.
fn store_interrupted_with_log(
    db: &Database,
    ctx: &Arc<SharedContext>,
    root: &std::path::Path,
    id: &str,
    session_id: &str,
) -> Task {
    let now = Utc::now();
    let task = Task {
        session_id: Some(session_id.to_string()),
        completed_at: Some(now),
        result_summary: Some("interrupted — the daemon restarted".to_string()),
        ..store_task_row(id, TaskStatus::Interrupted, Some("write the changelog"), now)
    };
    TaskRepository::new(db).create(&task).expect("create task");
    seed_log(root, session_id, id);
    ctx.set_session_log(Arc::new(crate::session_log::SessionLogService::new(
        root.to_path_buf(),
        None,
        crate::session_log::SessionLogLimits::default(),
        "test".to_string(),
    )));
    task
}

/// One `round` and its `tool_result`, written the way the writer writes them.
fn seed_log(root: &std::path::Path, session_id: &str, task_id: &str) {
    let dir = root.join(session_id);
    std::fs::create_dir_all(&dir).unwrap();
    let records = [
        serde_json::json!({
            "v": 1, "seq": 1, "ts": "2026-09-06T10:00:00.000Z", "type": "round",
            "task_id": task_id,
            "data": {
                "round": 1, "text": "reading the file",
                "tool_use": [{"id": "tu-1", "name": "file_read", "input": {"path": "a.rs"}}],
            },
        }),
        serde_json::json!({
            "v": 1, "seq": 2, "ts": "2026-09-06T10:00:01.000Z", "type": "tool_result",
            "task_id": task_id,
            "data": {"tool_use_id": "tu-1", "name": "file_read", "ok": true, "result": "fn main"},
        }),
    ];
    let body: String = records
        .iter()
        .map(|r| format!("{}\n", serde_json::to_string(r).unwrap()))
        .collect();
    std::fs::write(dir.join(crate::session_log::LIVE_SEGMENT), body).unwrap();
}

/// The flag is the whole point: S2 is the plan's one speculative piece and
/// ships **off**, so the verb refuses until an owner turns it on.
#[tokio::test]
async fn resume_is_refused_while_the_flag_is_off() {
    let (_dir, db) = temp_db();
    let logs = tempfile::tempdir().unwrap();
    let (orchestrator, ctx) = ready(&db);
    store_interrupted_with_log(&db, &ctx, logs.path(), "t1", "s1");

    assert!(!DaemonConfig::default().orchestrator.routing.resume_enabled);
    assert_eq!(
        orchestrator.resume_task("t1").await,
        Err(TaskLaunchError::ResumeDisabled)
    );
    // And nothing was launched behind the refusal.
    let row = TaskRepository::new(&db).get("t1").unwrap().unwrap();
    assert_eq!(row.status, TaskStatus::Interrupted);
}

/// `resume` is for an `interrupted` run and nothing else — a finished one is
/// `rerun`'s, a live one is nobody's.
#[tokio::test]
async fn resume_is_refused_on_a_run_that_was_not_interrupted() {
    let (_dir, db) = temp_db();
    let logs = tempfile::tempdir().unwrap();
    let (orchestrator, ctx) = ready(&db);
    enable_resume(&orchestrator);
    seed_log(logs.path(), "s1", "t1");
    ctx.set_session_log(Arc::new(crate::session_log::SessionLogService::new(
        logs.path().to_path_buf(),
        None,
        crate::session_log::SessionLogLimits::default(),
        "test".to_string(),
    )));
    for status in [
        TaskStatus::Completed,
        TaskStatus::Running,
        TaskStatus::Queued,
        TaskStatus::Cancelled,
    ] {
        let id = format!("t-{}", status.as_str());
        store_task(&db, &id, status, Some("write the changelog"));
        assert_eq!(
            orchestrator.resume_task(&id).await,
            Err(TaskLaunchError::NotResumable {
                current: status.as_str()
            }),
            "{status:?}"
        );
    }
    assert_eq!(
        orchestrator.resume_task("no-such-run").await,
        Err(TaskLaunchError::NotFound)
    );
}

/// §5.6c: "a gutted log is a clean 409 pointing at `rerun`". A run whose
/// session the byte-cap sweep has emptied has nothing to re-prime from, and
/// the verb must say that rather than quietly starting from scratch under an
/// id whose row already carries a result.
#[tokio::test]
async fn resume_is_refused_when_the_log_is_gone() {
    let (_dir, db) = temp_db();
    let logs = tempfile::tempdir().unwrap();
    let (orchestrator, ctx) = ready(&db);
    let _task = store_interrupted_with_log(&db, &ctx, logs.path(), "t1", "s1");
    enable_resume(&orchestrator);

    // The sweep took the session's live segment (R54).
    std::fs::remove_file(logs.path().join("s1").join(crate::session_log::LIVE_SEGMENT)).unwrap();
    assert_eq!(
        orchestrator.resume_task("t1").await,
        Err(TaskLaunchError::ResumeLogMissing)
    );

    // A run that never had a session at all is the same answer.
    store_task(&db, "t2", TaskStatus::Interrupted, Some("write the changelog"));
    assert_eq!(
        orchestrator.resume_task("t2").await,
        Err(TaskLaunchError::ResumeLogMissing)
    );
}

/// D5's philosophy, applied to the third verb: a resume keeps the id the
/// client is already holding, and re-enters the row rather than copying it.
#[tokio::test]
async fn resume_relaunches_the_run_under_its_own_id() {
    let (_dir, db) = temp_db();
    let logs = tempfile::tempdir().unwrap();
    let (orchestrator, ctx) = ready(&db);
    store_interrupted_with_log(&db, &ctx, logs.path(), "t1", "s1");
    enable_resume(&orchestrator);

    let outcome = orchestrator.resume_task("t1").await.expect("resumed");

    assert_eq!(outcome.task_id, "t1", "the same id (D5)");
    assert_eq!(outcome.title, "Ship the release");
    assert_eq!(outcome.rounds_replayed, 1);
    assert_eq!(outcome.from_seq, Some(1));
    assert_eq!(outcome.to_seq, Some(2));

    let row = TaskRepository::new(&db).get("t1").unwrap().expect("the row");
    assert!(
        matches!(row.status, TaskStatus::Queued | TaskStatus::Running),
        "interrupted → live again, not a new row: {:?}",
        row.status
    );
    assert_eq!(row.source_task_id, None, "a resume is not a copy");
    assert_eq!(
        row.session_id.as_deref(),
        Some("s1"),
        "the run keeps the conversation it belonged to"
    );
    // The slot is claimed for the duration, exactly as `start` claims it.
    assert!(!ctx.claim_run_slot("t1"));
}

/// Turn S2 on for one orchestrator, the way a hand-edited `daemon.toml`
/// would.
fn enable_resume(orchestrator: &Orchestrator) {
    let mut config = DaemonConfig::clone(&orchestrator.daemon_config.load());
    config.orchestrator.routing.resume_enabled = true;
    orchestrator.daemon_config.store(Arc::new(config));
}
