use super::*;
use crate::agent::subagent::SubAgent;
use crate::test_util::{make_agent, template_from_agent};

fn setup(agents: Vec<SubAgent>) -> TaskDispatcher {
    setup_with_config(agents, DaemonConfig::default())
}

fn setup_with_config(agents: Vec<SubAgent>, config: DaemonConfig) -> TaskDispatcher {
    setup_with_config_and_db(agents, config, None)
}

fn setup_with_config_and_db(
    agents: Vec<SubAgent>,
    config: DaemonConfig,
    db: Option<Database>,
) -> TaskDispatcher {
    let ctx = Arc::new(SharedContext::new());
    for a in &agents {
        // Register both template (for spawn_instance) and instance (for backward compat)
        ctx.agent_registry.register_template(template_from_agent(a));
        ctx.agent_registry.register(a.clone());
    }
    let lane_mgr = Arc::new(LaneManager::new());
    let bus = EventBus::default();
    let tool_registry = Arc::new(crate::tools::ToolRegistry::default());
    let sandbox = Arc::new(crate::security::sandbox::SandboxManager::with_defaults(
        tool_registry.clone(),
        bus.clone(),
    ));
    let gate = Arc::new(crate::security::gate::SecurityGate::new(sandbox));
    let daemon_config = Arc::new(ArcSwap::from_pointee(config));
    TaskDispatcher::new(
        ctx,
        lane_mgr,
        bus,
        None,
        gate,
        tool_registry,
        db,
        None,
        daemon_config,
        Arc::new(std::sync::RwLock::new(None)),
        Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new()),
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
    )
}

/// Setup with a (provider-less) router and a real DB so the spawned lead
/// agent execution actually runs — required by the steering lifecycle tests.
fn setup_with_router_and_db(
    agents: Vec<SubAgent>,
    config: DaemonConfig,
    db: Database,
) -> TaskDispatcher {
    // Provider-less router: the spawned loop's first LLM call fails, so the
    // execution runs and finalizes with LoopFinishReason::Error.
    let router = Arc::new(openalpaca_llm::LlmRouter::new(
        std::collections::HashMap::new(),
        openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
        std::collections::HashMap::new(),
        Arc::new(openalpaca_llm::CostTracker::new(
            openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
        )),
        "test-model".to_string(),
    ));
    setup_with_specific_router_and_db(agents, config, db, router)
}

fn setup_with_specific_router_and_db(
    agents: Vec<SubAgent>,
    config: DaemonConfig,
    db: Database,
    router: Arc<openalpaca_llm::LlmRouter>,
) -> TaskDispatcher {
    let ctx = Arc::new(SharedContext::new());
    for a in &agents {
        ctx.agent_registry.register_template(template_from_agent(a));
        ctx.agent_registry.register(a.clone());
    }
    let lane_mgr = Arc::new(LaneManager::new());
    let bus = EventBus::default();
    let tool_registry = Arc::new(crate::tools::ToolRegistry::default());
    let sandbox = Arc::new(crate::security::sandbox::SandboxManager::with_defaults(
        tool_registry.clone(),
        bus.clone(),
    ));
    let gate = Arc::new(crate::security::gate::SecurityGate::new(sandbox));
    TaskDispatcher::new(
        ctx,
        lane_mgr,
        bus,
        Some(router),
        gate,
        tool_registry,
        Some(db),
        None,
        Arc::new(ArcSwap::from_pointee(config)),
        Arc::new(std::sync::RwLock::new(None)),
        Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new()),
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
    )
}

#[test]
fn test_title_generation_short() {
    // Test the generate_title function directly
    assert_eq!(generate_title("Short task"), "Short task");
}

#[test]
fn test_title_generation_strips_filler() {
    assert_eq!(
        generate_title("can you search for Rust docs"),
        "Search for rust docs"
    );
    assert_eq!(
        generate_title("please help me find the answer"),
        "Find the answer"
    );
}

#[test]
fn test_title_generation_truncates_long() {
    let long_desc = "search for the latest version of rust and all related documentation";
    let title = generate_title(long_desc);
    assert!(title.len() <= 53); // 50 + "..."
    assert!(title.ends_with("..."));
}

#[test]
fn test_dispatch_lead_agent_marks_agent_busy() {
    let dispatcher = setup(vec![
        make_agent("lead-01", vec!["orchestration"]),
        make_agent("worker-01", vec!["web_search"]),
    ]);

    let result = dispatcher.dispatch_lead_agent(
        "Complex research task",
        "Test Lead Agent Task".to_string(),
        "user1",
        "user1:cli",
        "cli",
        None,
    );

    assert!(result.is_ok());

    // The lead agent should be marked Busy
    let lead = dispatcher
        .shared_context
        .agent_registry
        .get("lead-01")
        .unwrap();
    assert_eq!(lead.status.as_str(), "busy");

    // The worker should still be Idle
    let worker = dispatcher
        .shared_context
        .agent_registry
        .get("worker-01")
        .unwrap();
    assert!(worker.status.is_available());
}

#[test]
fn test_dispatch_lead_agent_prefers_orchestration_capability() {
    // When an agent with "orchestration" capability exists, it should be preferred
    let dispatcher = setup(vec![
        make_agent("worker-01", vec!["web_search"]),
        make_agent("lead-01", vec!["orchestration"]),
    ]);

    let result = dispatcher.dispatch_lead_agent(
        "Complex task",
        "Test".to_string(),
        "user1",
        "user1:cli",
        "cli",
        None,
    );

    assert!(result.is_ok());

    // lead-01 should be busy (it has orchestration capability)
    let lead = dispatcher
        .shared_context
        .agent_registry
        .get("lead-01")
        .unwrap();
    assert_eq!(lead.status.as_str(), "busy");

    // worker-01 should still be idle
    let worker = dispatcher
        .shared_context
        .agent_registry
        .get("worker-01")
        .unwrap();
    assert!(worker.status.is_available());
}

#[test]
fn test_dispatch_lead_agent_fallback_to_any_idle_agent() {
    // When no agent has "orchestration" capability, any idle agent template is used.
    // The worker-01 template is non-singleton, so spawn_instance creates a new instance
    // with a UUID suffix. We verify that:
    // 1. dispatch succeeds
    // 2. a new instance exists for the worker-01 template
    let dispatcher = setup(vec![make_agent("worker-01", vec!["web_search"])]);

    let result = dispatcher.dispatch_lead_agent(
        "Complex task",
        "Test".to_string(),
        "user1",
        "user1:cli",
        "cli",
        None,
    );

    assert!(result.is_ok());

    // A new instance spawned from worker-01 template should exist and be busy
    assert!(
        dispatcher
            .shared_context
            .agent_registry
            .count_instances_of("worker-01")
            >= 1
    );
    let instances = dispatcher.shared_context.agent_registry.list_instances();
    let busy_worker = instances
        .iter()
        .find(|a| a.template_id == "worker-01" && !a.status.is_available());
    assert!(
        busy_worker.is_some(),
        "Expected a busy instance of worker-01"
    );
}

#[test]
fn test_dispatch_lead_agent_fails_no_agents() {
    // When no agents are available at all, should fail
    let dispatcher = setup(vec![]);

    let result = dispatcher.dispatch_lead_agent(
        "Complex task",
        "Test".to_string(),
        "user1",
        "user1:cli",
        "cli",
        None,
    );

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("No agents available"));
}

#[test]
fn test_build_task_outcome_no_db_success() {
    use super::build_task_outcome;
    use openalpaca_storage::OutcomeKind;

    let outcome = build_task_outcome(None, "t1", "Research completed successfully", true);
    assert_eq!(outcome.outcome_kind, OutcomeKind::TextOnly);
    assert_eq!(outcome.summary, "Research completed successfully");
    assert!(outcome.artifacts.is_empty());
    assert!(outcome.no_artifact_reason.is_some());
}

#[test]
fn test_build_task_outcome_no_db_failure() {
    use super::build_task_outcome;
    use openalpaca_storage::OutcomeKind;

    let outcome = build_task_outcome(None, "t1", "Network timeout", false);
    assert_eq!(outcome.outcome_kind, OutcomeKind::Failed);
    assert_eq!(outcome.summary, "Network timeout");
    assert!(outcome.artifacts.is_empty());
    assert!(outcome.no_artifact_reason.is_none());
}

#[test]
fn test_build_task_outcome_empty_content_fallback() {
    use super::build_task_outcome;
    use openalpaca_storage::OutcomeKind;

    let outcome = build_task_outcome(None, "t1", "", true);
    assert_eq!(outcome.outcome_kind, OutcomeKind::TextOnly);
    assert_eq!(outcome.summary, "Task completed.");
}

#[test]
fn test_build_task_outcome_empty_content_failure() {
    use super::build_task_outcome;
    use openalpaca_storage::OutcomeKind;

    let outcome = build_task_outcome(None, "t1", "", false);
    assert_eq!(outcome.outcome_kind, OutcomeKind::Failed);
    assert_eq!(outcome.summary, "Task failed.");
}

// ── Pipeline end-to-end: non-singleton agent + workspace artifact ────

/// Mock LLM provider for e2e pipeline tests.
/// Returns pre-scripted responses in order; repeats the last one on overflow.
mod e2e_mock {
    use async_trait::async_trait;
    use openalpaca_llm::{ChatRequest, ChatResponse, LlmError, LlmProvider};
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(super) struct MockProvider {
        responses: Vec<ChatResponse>,
        call_count: AtomicUsize,
    }

    impl MockProvider {
        pub fn new(responses: Vec<ChatResponse>) -> Self {
            Self {
                responses,
                call_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }
        fn supports_tools(&self) -> bool {
            true
        }
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            let resp = if idx < self.responses.len() {
                &self.responses[idx]
            } else {
                self.responses.last().unwrap()
            };
            Ok(resp.clone())
        }
    }
}

#[test]
fn test_finalize_task_emits_outcome_fields() {
    use super::finalize_task;
    use openalpaca_storage::OutcomeKind;

    let ctx = std::sync::Arc::new(crate::context::SharedContext::new());
    ctx.task_registry
        .register("t1".to_string(), "Test task".to_string());
    let bus = crate::bus::EventBus::default();
    let mut rx = bus.subscribe();

    finalize_task(
        &ctx,
        &bus,
        None,
        "t1",
        "All done",
        true,
        Some(OutcomeKind::Mixed),
        Some(2),
        Some("Generated report and chart"),
    );

    // Drain events and find TaskCompleted
    while let Ok(event) = rx.try_recv() {
        if let SystemEvent::TaskCompleted {
            task_id,
            result_summary,
            outcome_kind,
            artifact_count,
            outcome_summary,
            ..
        } = event
        {
            assert_eq!(task_id, "t1");
            assert_eq!(result_summary, Some("All done".to_string()));
            assert_eq!(outcome_kind, Some("mixed".to_string()));
            assert_eq!(artifact_count, Some(2));
            assert_eq!(
                outcome_summary,
                Some("Generated report and chart".to_string())
            );
            return;
        }
    }
    panic!("Expected TaskCompleted event with outcome fields");
}

#[test]
fn test_finalize_task_failed_emits_outcome_kind() {
    use super::finalize_task;
    use openalpaca_storage::OutcomeKind;

    let ctx = std::sync::Arc::new(crate::context::SharedContext::new());
    ctx.task_registry
        .register("t2".to_string(), "Failing task".to_string());
    let bus = crate::bus::EventBus::default();
    let mut rx = bus.subscribe();

    finalize_task(
        &ctx,
        &bus,
        None,
        "t2",
        "Network timeout",
        false,
        Some(OutcomeKind::Failed),
        None,
        None,
    );

    while let Ok(event) = rx.try_recv() {
        if let SystemEvent::TaskFailed {
            task_id,
            error,
            outcome_kind,
            ..
        } = event
        {
            assert_eq!(task_id, "t2");
            assert_eq!(error, "Network timeout");
            assert_eq!(outcome_kind, Some("failed".to_string()));
            return;
        }
    }
    panic!("Expected TaskFailed event with outcome_kind");
}

#[test]
fn test_finalize_task_none_outcome_fields() {
    use super::finalize_task;

    let ctx = std::sync::Arc::new(crate::context::SharedContext::new());
    ctx.task_registry
        .register("t3".to_string(), "Legacy task".to_string());
    let bus = crate::bus::EventBus::default();
    let mut rx = bus.subscribe();

    // Pass None for all outcome params (backward compat path)
    finalize_task(&ctx, &bus, None, "t3", "Done", true, None, None, None);

    while let Ok(event) = rx.try_recv() {
        if let SystemEvent::TaskCompleted {
            task_id,
            outcome_kind,
            artifact_count,
            outcome_summary,
            ..
        } = event
        {
            assert_eq!(task_id, "t3");
            assert_eq!(outcome_kind, None);
            assert_eq!(artifact_count, None); // None preserved (unknown)
            assert_eq!(outcome_summary, None);
            return;
        }
    }
    panic!("Expected TaskCompleted event");
}

// ── Phase 12-A: True-gap regression tests ─────────────────────────

#[test]
fn test_build_task_outcome_with_db_and_state_json() {
    use super::build_task_outcome;
    use crate::orchestrator::task_state::{TaskConstraints, TaskState, TaskWorkspace};
    use openalpaca_storage::OutcomeKind;

    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let repo = openalpaca_storage::repository::TaskRepository::new(&db);

    // Create task in DB
    let now = chrono::Utc::now();
    let task = openalpaca_storage::Task {
        id: "t-outcome-db".to_string(),
        title: "DB outcome test".to_string(),
        description: None,
        status: openalpaca_storage::TaskStatus::Running,
        priority: 0,
        progress_current: None,
        progress_total: None,
        result_summary: None,
        created_by: "user1".to_string(),
        source_lane: "cli".to_string(),
        created_at: now,
        updated_at: now,
        completed_at: None,
        state_json: None,
        state_version: 0,
        outcome_json: None,
        outcome_kind: None,
        artifact_count: 0,
    };
    repo.create(&task).unwrap();

    // Build a TaskState with completed steps containing summaries
    let state = TaskState {
        objective: "Test objective".to_string(),
        steps: vec![
            crate::orchestrator::task_state::StepState {
                step_order: 0,
                agent_id: "agent-a".to_string(),
                agent_name: "Agent A".to_string(),
                role: "researcher".to_string(),
                status: "completed".to_string(),
                result_summary: Some("Found 3 relevant papers on the topic.".to_string()),
                artifact_pointers: Vec::new(),
                started_at: Some(now),
                completed_at: Some(now),
            },
            crate::orchestrator::task_state::StepState {
                step_order: 1,
                agent_id: "agent-b".to_string(),
                agent_name: "Agent B".to_string(),
                role: "writer".to_string(),
                status: "completed".to_string(),
                result_summary: Some("Synthesized findings into a report.".to_string()),
                artifact_pointers: Vec::new(),
                started_at: Some(now),
                completed_at: Some(now),
            },
        ],
        constraints: TaskConstraints {
            pipeline_sequential: true,
        },
        workspace: TaskWorkspace::default(),
        created_at: now,
        updated_at: now,
    };

    // Persist state_json to DB
    let state_json = state.to_json();
    assert!(repo.update_state("t-outcome-db", &state_json, 0).unwrap());

    // Call build_task_outcome with real DB — should read step summaries from state_json
    let outcome = build_task_outcome(Some(&db), "t-outcome-db", "fallback content", true);

    // Outcome should use step summaries (joined), NOT the fallback
    assert_eq!(outcome.outcome_kind, OutcomeKind::TextOnly);
    assert!(
        outcome.summary.contains("Found 3 relevant papers"),
        "Expected step summary in outcome, got: {}",
        outcome.summary
    );
    assert!(
        outcome.summary.contains("Synthesized findings"),
        "Expected second step summary in outcome, got: {}",
        outcome.summary
    );
    // Fallback should NOT be used when step summaries exist
    assert!(
        !outcome.summary.contains("fallback content"),
        "Fallback should not appear when step summaries exist"
    );
    assert!(outcome.artifacts.is_empty());
}

#[test]
fn test_finalize_task_with_outcome_persists_and_reads_back() {
    use super::finalize_task_with_outcome;
    use openalpaca_storage::OutcomeKind;

    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let repo = openalpaca_storage::repository::TaskRepository::new(&db);

    // Create task in DB
    let now = chrono::Utc::now();
    let task = openalpaca_storage::Task {
        id: "t-finalize".to_string(),
        title: "Finalize test".to_string(),
        description: None,
        status: openalpaca_storage::TaskStatus::Running,
        priority: 0,
        progress_current: None,
        progress_total: None,
        result_summary: None,
        created_by: "user1".to_string(),
        source_lane: "cli".to_string(),
        created_at: now,
        updated_at: now,
        completed_at: None,
        state_json: None,
        state_version: 0,
        outcome_json: None,
        outcome_kind: None,
        artifact_count: 0,
    };
    repo.create(&task).unwrap();

    // Register task in the in-memory registry
    let ctx = std::sync::Arc::new(crate::context::SharedContext::new());
    ctx.task_registry
        .register("t-finalize".to_string(), "Finalize test".to_string());
    let bus = crate::bus::EventBus::default();

    // Call finalize_task_with_outcome — no state_json, so fallback path
    let outcome = finalize_task_with_outcome(&ctx, &bus, Some(&db), "t-finalize", "Analysis complete", true);

    // Verify returned outcome
    assert_eq!(outcome.outcome_kind, OutcomeKind::TextOnly);
    assert_eq!(outcome.summary, "Analysis complete");

    // Read back from DB and verify persistence
    let persisted = repo.get("t-finalize").unwrap().unwrap();
    assert_eq!(
        persisted.outcome_kind,
        Some(OutcomeKind::TextOnly),
        "outcome_kind should be persisted"
    );
    assert_eq!(persisted.artifact_count, 0, "artifact_count should be 0");
    assert!(
        persisted.outcome_json.is_some(),
        "outcome_json should be persisted"
    );

    // Parse the persisted outcome_json and verify it matches
    let parsed: crate::orchestrator::task_state::TaskOutcome =
        serde_json::from_str(persisted.outcome_json.as_ref().unwrap()).unwrap();
    assert_eq!(parsed.outcome_kind, OutcomeKind::TextOnly);
    assert_eq!(parsed.summary, "Analysis complete");
    assert!(parsed.artifacts.is_empty());
    assert!(parsed.no_artifact_reason.is_some());

    // Verify result_summary was also set
    assert_eq!(
        persisted.result_summary.as_deref(),
        Some("Analysis complete")
    );
    // Verify status was updated to Completed
    assert_eq!(persisted.status, openalpaca_storage::TaskStatus::Completed);
}

fn lifecycle_steering_msg(text: &str) -> crate::runner::steering::SteeringMsg {
    crate::runner::steering::SteeringMsg {
        text: text.to_string(),
        request_id: uuid::Uuid::new_v4(),
        principal: crate::security::policy::Principal::User {
            global_id: "user1".to_string(),
        },
        scope: crate::security::policy::Scope::Global,
        workspace_path: None,
        received_at: Utc::now(),
    }
}

#[tokio::test]
async fn test_lead_agent_steering_attach_detach_and_leftover_conversion() {
    let mut config = DaemonConfig::default();
    config.orchestrator.routing.steering_enabled = true;
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    let dispatcher = setup_with_router_and_db(
        vec![make_agent("lead-01", vec!["orchestration"])],
        config,
        db.clone(),
    );

    let outcome = dispatcher
        .dispatch_lead_agent(
            "Long running task",
            "Steering lifecycle".to_string(),
            "user1",
            "user1:cli",
            "cli",
            None,
        )
        .unwrap();
    let task_id = outcome.task_id;

    // Attach: inbox + lane registration happen synchronously at dispatch
    // (before the spawned execution runs on the current-thread test runtime).
    let inbox = dispatcher
        .shared_context
        .steering_inbox(&task_id)
        .expect("steering inbox must be registered at dispatch");
    assert_eq!(
        dispatcher.shared_context.workflows_for_lane("user1:cli"),
        vec![task_id.clone()]
    );
    // A task lane is created at dispatch…
    assert!(dispatcher.lane_manager.get_task_lane(&task_id).is_some());

    // Queue a message the loop will never consume, then cancel the task.
    inbox
        .push(lifecycle_steering_msg("switch to staging"))
        .unwrap();
    assert!(dispatcher.shared_context.cancel_task(&task_id));

    // Detach: the spawned execution closes and deregisters the inbox.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while dispatcher.shared_context.steering_inbox(&task_id).is_some() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "steering detach timed out"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(inbox.is_closed());
    assert!(
        dispatcher
            .shared_context
            .workflows_for_lane("user1:cli")
            .is_empty()
    );
    // …and removed again by the execution cleanup (no task-lane leak).
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while dispatcher.lane_manager.get_task_lane(&task_id).is_some() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "task lane removal timed out"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // The undelivered message became an unprocessed_steering follow-up row…
    let repo = openalpaca_storage::repository::FollowupRepository::new(&db);
    let rows = repo.list_queued_by_lane("user1:cli").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, "unprocessed_steering");
    assert_eq!(rows[0].content, "switch to staging");
    assert_eq!(rows[0].source_task_id.as_deref(), Some(task_id.as_str()));
    let principal: crate::security::policy::Principal =
        serde_json::from_str(&rows[0].principal_json).unwrap();
    assert_eq!(
        principal,
        crate::security::policy::Principal::User {
            global_id: "user1".to_string()
        }
    );
    // …which the finalize autostart hook must never claim.
    assert!(repo.claim_next("user1:cli").unwrap().is_none());
}

#[tokio::test]
async fn test_lead_agent_lane_attachment_independent_of_steering_flag() {
    // steering_enabled=false: no inbox registers, but the lane attachment
    // (which backs the StartWorkflowTool cap and the workflow-context
    // block) must still register at dispatch and deregister at cleanup.
    let mut config = DaemonConfig::default();
    config.orchestrator.routing.steering_enabled = false;
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    let dispatcher = setup_with_router_and_db(
        vec![make_agent("lead-01", vec!["orchestration"])],
        config,
        db,
    );

    let outcome = dispatcher
        .dispatch_lead_agent(
            "Some task",
            "No steering".to_string(),
            "user1",
            "user1:cli",
            "cli",
            None,
        )
        .unwrap();

    // Lane attachment registered, no steering inbox.
    assert_eq!(
        dispatcher.shared_context.workflows_for_lane("user1:cli"),
        vec![outcome.task_id.clone()]
    );
    assert!(
        dispatcher
            .shared_context
            .steering_inbox(&outcome.task_id)
            .is_none()
    );

    // Cancel the background execution; the lane attachment must
    // deregister even though no inbox was ever registered.
    dispatcher.shared_context.cancel_task(&outcome.task_id);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !dispatcher
        .shared_context
        .workflows_for_lane("user1:cli")
        .is_empty()
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "lane deregistration timed out"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

// ── Routing V2 §2b: model-authored completion report ─────────────────

/// Poll the lane's conversation until an assistant message lands (the spawned
/// lead execution persists it right before publishing TaskCompleted).
async fn wait_for_conversation_message(
    db: &Database,
    lane_key: &str,
) -> openalpaca_storage::ConversationMessage {
    let repo = openalpaca_storage::ConversationRepository::new(db);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let messages = repo.list_by_lane(lane_key, 10, 0).unwrap_or_default();
        if let Some(msg) = messages.into_iter().next() {
            return msg;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for the completion conversation message"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn test_lead_agent_completion_report_persists_final_content_verbatim() {
    use openalpaca_llm::{ChatResponse, FinishReason, ProviderType, Usage};

    let report = "Report: refactored the parser, added 12 tests, all green.\n\
                  Problems hit: none. Queued for later: nothing.";
    let mock_provider = Arc::new(e2e_mock::MockProvider::new(vec![ChatResponse {
        content: report.to_string(),
        tool_calls: vec![],
        model: "claude-sonnet-4-5-20250929".to_string(),
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        },
        finish_reason: FinishReason::Stop,
        thinking: None,
        parts: None,
    }]));
    let router = Arc::new(openalpaca_llm::LlmRouter::single_provider(
        mock_provider,
        ProviderType::Anthropic,
        "claude-sonnet-4-5-20250929".to_string(),
    ));

    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    let dispatcher = setup_with_specific_router_and_db(
        vec![make_agent("lead-01", vec!["orchestration"])],
        DaemonConfig::default(),
        db.clone(),
        router,
    );

    dispatcher
        .dispatch_lead_agent(
            "Refactor the parser",
            "Parser refactor".to_string(),
            "user1",
            "user1:cli",
            "cli",
            None,
        )
        .unwrap();

    // The lead agent's final message IS the completion message — verbatim,
    // no "**Task completed: ...**" template wrapper, no status prefix.
    let msg = wait_for_conversation_message(&db, "user1:cli").await;
    assert_eq!(msg.role, "assistant");
    assert_eq!(msg.content, report);
}

#[tokio::test]
async fn test_lead_agent_completion_report_empty_falls_back_to_template_with_status() {
    // Provider-less router: the loop exits with LoopFinishReason::Error and
    // empty final content → legacy template fallback + one-line status prefix.
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    let dispatcher = setup_with_router_and_db(
        vec![make_agent("lead-01", vec!["orchestration"])],
        DaemonConfig::default(),
        db.clone(),
    );

    dispatcher
        .dispatch_lead_agent(
            "Doomed task",
            "Doomed task".to_string(),
            "user1",
            "user1:cli",
            "cli",
            None,
        )
        .unwrap();

    let msg = wait_for_conversation_message(&db, "user1:cli").await;
    assert!(
        msg.content.starts_with("Task ended with an error:\n\n"),
        "missing status prefix: {}",
        msg.content
    );
    assert!(
        msg.content.contains("**Task failed: Doomed task**"),
        "missing template fallback: {}",
        msg.content
    );
}
