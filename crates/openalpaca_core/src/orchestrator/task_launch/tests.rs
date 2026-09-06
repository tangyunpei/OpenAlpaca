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
