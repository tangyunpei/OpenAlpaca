use super::*;

#[test]
fn test_lead_agent_registry_contains_coordination_tools() {
    // Verify that register_coordination_tools populates the registry with
    // spawn_subagent, check_subagent_status, and wait_for_subagents.
    // batch_spawn is absent when None is passed.
    use crate::tools::registry::{RegisteredTool, ToolBackend};

    struct NoopTool;

    #[async_trait]
    impl BuiltInTool for NoopTool {
        async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Ok("noop".to_string())
        }
    }

    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "web_search".to_string(),
            description: "test".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(NoopTool)),
        provides_capabilities: vec!["web_search".to_string()],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    let tracker = Arc::new(SubagentTracker::new());
    let spawn_tool = Arc::new(SpawnSubagentTool::new(
        Arc::new(openalpaca_llm::LlmRouter::new(
            std::collections::HashMap::new(),
            openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            std::collections::HashMap::new(),
            Arc::new(openalpaca_llm::CostTracker::new(
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            )),
            "test-model".to_string(),
        )),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        "task-1".to_string(),
        "user-1".to_string(),
        "test-lead".to_string(),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        None,
        tracker.clone(),
        0,
        DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        None,
        None,
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::prompt_ctx::section::ContextBundle::empty()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
    ));
    let check_status_tool = Arc::new(CheckSubagentStatusTool {
        tracker: tracker.clone(),
    });
    let wait_tool = Arc::new(WaitForSubagentsTool { tracker, steering: None });

    register_coordination_tools(
        &registry,
        spawn_tool,
        None, // no batch spawn
        check_status_tool,
        wait_tool,
        spawn_subagent_tool_definition_from_templates(&[]),
        None,
        check_subagent_status_tool_definition(),
        wait_for_subagents_tool_definition(),
    );

    let tools = registry.registered_tool_names();
    assert!(tools.contains(&"spawn_subagent".to_string()));
    assert!(!tools.contains(&"spawn_subagents_batch".to_string()));
    assert!(tools.contains(&"check_subagent_status".to_string()));
    assert!(tools.contains(&"wait_for_subagents".to_string()));
    assert!(tools.contains(&"web_search".to_string()));
}

// ── SubagentTracker tests ────────────────────────────────────────

#[test]
fn test_tracker_register_and_status() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    assert!(matches!(tracker.get("run-1"), Some(SubagentStatus::Queued)));
    assert!(!tracker.all_done());
}

#[test]
fn test_tracker_complete() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    tracker.complete("run-1", "Result text".to_string(), true);

    match tracker.get("run-1") {
        Some(SubagentStatus::Completed { content, success }) => {
            assert_eq!(content, "Result text");
            assert!(success);
        }
        other => panic!("Expected Completed, got {:?}", other),
    }
    assert!(tracker.all_done());
}

#[test]
fn test_tracker_fail() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    tracker.fail("run-1", "Some error".to_string());

    match tracker.get("run-1") {
        Some(SubagentStatus::Failed { error }) => {
            assert_eq!(error, "Some error");
        }
        other => panic!("Expected Failed, got {:?}", other),
    }
    assert!(tracker.all_done());
}

#[test]
fn test_tracker_all_done_mixed() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    tracker.register("run-2");
    tracker.register("run-3");

    // Partially complete
    tracker.complete("run-1", "done".to_string(), true);
    assert!(!tracker.all_done());

    tracker.fail("run-2", "err".to_string());
    assert!(!tracker.all_done());

    tracker.complete("run-3", "done too".to_string(), true);
    assert!(tracker.all_done());
}

#[test]
fn test_tracker_summary() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    tracker.complete("run-1", "Research result".to_string(), true);
    tracker.register("run-2");
    tracker.fail("run-2", "Timeout".to_string());

    let summary = tracker.summary();
    assert!(summary.contains("run-1"));
    assert!(summary.contains("completed"));
    assert!(summary.contains("Research result"));
    assert!(summary.contains("run-2"));
    assert!(summary.contains("failed"));
    assert!(summary.contains("Timeout"));
}

#[test]
fn test_tracker_empty_summary() {
    let tracker = SubagentTracker::new();
    assert!(tracker.summary().contains("No subagents"));
}

#[test]
fn test_tracker_set_status() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    assert!(matches!(tracker.get("run-1"), Some(SubagentStatus::Queued)));

    tracker.set_status("run-1", SubagentStatus::Running);
    assert!(matches!(
        tracker.get("run-1"),
        Some(SubagentStatus::Running)
    ));
    assert!(!tracker.all_done());

    tracker.set_status(
        "run-1",
        SubagentStatus::Completed {
            content: "done".to_string(),
            success: true,
        },
    );
    assert!(tracker.all_done());
}

#[test]
fn test_tracker_all_done_with_queued() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    tracker.register("run-2");

    // Both queued — not done
    assert!(!tracker.all_done());

    // One running, one queued — not done
    tracker.set_status("run-1", SubagentStatus::Running);
    assert!(!tracker.all_done());

    // One completed, one queued — not done
    tracker.complete("run-1", "done".to_string(), true);
    assert!(!tracker.all_done());

    // Both completed — done
    tracker.complete("run-2", "done too".to_string(), true);
    assert!(tracker.all_done());
}

#[test]
fn test_tracker_status_counts() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1"); // queued
    tracker.register("run-2"); // queued
    tracker.register("run-3"); // queued
    tracker.register("run-4"); // queued

    let (queued, running, completed, failed) = tracker.status_counts();
    assert_eq!((queued, running, completed, failed), (4, 0, 0, 0));

    tracker.set_status("run-1", SubagentStatus::Running);
    tracker.complete("run-2", "done".to_string(), true);
    tracker.fail("run-3", "err".to_string());

    let (queued, running, completed, failed) = tracker.status_counts();
    assert_eq!((queued, running, completed, failed), (1, 1, 1, 1));
}

#[test]
fn test_tracker_summary_with_queued() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    let summary = tracker.summary();
    assert!(summary.contains("run-1"));
    assert!(summary.contains("queued"));
    assert!(summary.contains("waiting for execution slot"));
}

#[tokio::test]
async fn test_check_subagent_status_tool() {
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-abc");
    tracker.complete("run-abc", "Done!".to_string(), true);

    let tool = CheckSubagentStatusTool {
        tracker: tracker.clone(),
    };

    // Completed
    let result = tool
        .execute(&serde_json::json!({"subagent_run_id": "run-abc"}))
        .await;
    assert!(result.is_ok());
    assert!(result.unwrap().contains("completed successfully"));

    // Unknown
    let result = tool
        .execute(&serde_json::json!({"subagent_run_id": "no-such"}))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_check_subagent_status_tool_queued() {
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-queued");

    let tool = CheckSubagentStatusTool {
        tracker: tracker.clone(),
    };

    // Queued
    let result = tool
        .execute(&serde_json::json!({"subagent_run_id": "run-queued"}))
        .await;
    assert!(result.is_ok());
    let msg = result.unwrap();
    assert!(msg.contains("queued"));
    assert!(msg.contains("execution slot"));

    // Transition to Running
    tracker.set_status("run-queued", SubagentStatus::Running);
    let result = tool
        .execute(&serde_json::json!({"subagent_run_id": "run-queued"}))
        .await;
    assert!(result.is_ok());
    assert!(result.unwrap().contains("still running"));
}

// ── Batch spawn tool tests ────────────────────────────────────────

#[test]
fn test_batch_spawn_tool_definition_includes_agents() {
    use crate::agent::template::{AgentSource, AgentTemplateFrontmatter};

    let templates = vec![AgentTemplate {
        frontmatter: AgentTemplateFrontmatter {
            id: "researcher".to_string(),
            name: "Researcher".to_string(),
            description: "Research agent".to_string(),
            icon: None,
            singleton: false,
            capabilities: vec!["web_search".to_string()],
            denied_capabilities: vec![],
            temperature: 0.5,
            verbosity: "normal".to_string(),
            model: None,
            fallback_models: vec![],
            max_tool_calls: None,
            timeout_seconds: None,
            max_cost_per_task: None,
            max_rounds: None,
            require_confirmation_for: vec![],
        },
        body: String::new(),
        sections: HashMap::new(),
        source: AgentSource::default(),
    }];

    let def = spawn_subagents_batch_tool_definition(&templates);
    assert_eq!(def.name, "spawn_subagents_batch");
    assert!(def.description.contains("researcher"));
    assert!(def.description.contains("Researcher"));
}

#[test]
fn test_batch_spawn_tool_hidden_when_disabled() {
    // When batch_spawn is disabled (None passed), spawn_subagents_batch should NOT
    // appear in the registry.
    let tracker = Arc::new(SubagentTracker::new());
    let spawn_tool = Arc::new(SpawnSubagentTool::new(
        Arc::new(openalpaca_llm::LlmRouter::new(
            std::collections::HashMap::new(),
            openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            std::collections::HashMap::new(),
            Arc::new(openalpaca_llm::CostTracker::new(
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            )),
            "test-model".to_string(),
        )),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        "task-1".to_string(),
        "user-1".to_string(),
        "test-lead".to_string(),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        None,
        tracker.clone(),
        0,
        DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        None,
        None,
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::prompt_ctx::section::ContextBundle::empty()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
    ));
    let check_tool = Arc::new(CheckSubagentStatusTool {
        tracker: tracker.clone(),
    });
    let wait_tool = Arc::new(WaitForSubagentsTool { tracker, steering: None });

    let registry = ToolRegistry::default();
    register_coordination_tools(
        &registry,
        spawn_tool,
        None, // batch disabled
        check_tool,
        wait_tool,
        spawn_subagent_tool_definition_from_templates(&[]),
        None,
        check_subagent_status_tool_definition(),
        wait_for_subagents_tool_definition(),
    );

    let tools = registry.registered_tool_names();
    assert!(!tools.contains(&"spawn_subagents_batch".to_string()));
    assert!(tools.contains(&"spawn_subagent".to_string()));
}

#[test]
fn test_batch_spawn_tool_present_when_enabled() {
    // When batch_spawn is enabled (Some passed), spawn_subagents_batch should
    // appear in the registry.
    let tracker = Arc::new(SubagentTracker::new());
    let spawn_tool = Arc::new(SpawnSubagentTool::new(
        Arc::new(openalpaca_llm::LlmRouter::new(
            std::collections::HashMap::new(),
            openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            std::collections::HashMap::new(),
            Arc::new(openalpaca_llm::CostTracker::new(
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            )),
            "test-model".to_string(),
        )),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        "task-1".to_string(),
        "user-1".to_string(),
        "test-lead".to_string(),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        None,
        tracker.clone(),
        0,
        DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        None,
        None,
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::prompt_ctx::section::ContextBundle::empty()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
    ));
    let batch_tool = Some(Arc::new(SpawnSubagentsBatchTool::new(spawn_tool.clone())));
    let check_tool = Arc::new(CheckSubagentStatusTool {
        tracker: tracker.clone(),
    });
    let wait_tool = Arc::new(WaitForSubagentsTool { tracker, steering: None });

    let registry = ToolRegistry::default();
    register_coordination_tools(
        &registry,
        spawn_tool,
        batch_tool, // batch enabled
        check_tool,
        wait_tool,
        spawn_subagent_tool_definition_from_templates(&[]),
        Some(spawn_subagents_batch_tool_definition(&[])),
        check_subagent_status_tool_definition(),
        wait_for_subagents_tool_definition(),
    );

    let tools = registry.registered_tool_names();
    assert!(tools.contains(&"spawn_subagents_batch".to_string()));
    assert!(tools.contains(&"spawn_subagent".to_string()));
}

#[tokio::test]
async fn test_batch_spawn_empty_array_error() {
    let tracker = Arc::new(SubagentTracker::new());
    let spawn_tool = Arc::new(SpawnSubagentTool::new(
        Arc::new(openalpaca_llm::LlmRouter::new(
            std::collections::HashMap::new(),
            openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            std::collections::HashMap::new(),
            Arc::new(openalpaca_llm::CostTracker::new(
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            )),
            "test-model".to_string(),
        )),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        "task-1".to_string(),
        "user-1".to_string(),
        "test-lead".to_string(),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        None,
        tracker,
        0,
        DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        None, // workspace_id
        None, // confirmation_broker
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::prompt_ctx::section::ContextBundle::empty()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
    ));
    let batch_tool = SpawnSubagentsBatchTool::new(spawn_tool);

    let result = batch_tool
        .execute(&serde_json::json!({"subagents": []}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("at least 1"));
}

#[tokio::test]
async fn test_batch_spawn_exceeds_max_error() {
    let tracker = Arc::new(SubagentTracker::new());
    let spawn_tool = Arc::new(SpawnSubagentTool::new(
        Arc::new(openalpaca_llm::LlmRouter::new(
            std::collections::HashMap::new(),
            openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            std::collections::HashMap::new(),
            Arc::new(openalpaca_llm::CostTracker::new(
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            )),
            "test-model".to_string(),
        )),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        "task-1".to_string(),
        "user-1".to_string(),
        "test-lead".to_string(),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        None,
        tracker,
        0,
        DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        None, // workspace_id
        None, // confirmation_broker
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::prompt_ctx::section::ContextBundle::empty()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
    ));
    let batch_tool = SpawnSubagentsBatchTool::new(spawn_tool);

    // 9 items should fail (max 8)
    let items: Vec<serde_json::Value> = (0..9)
        .map(|i| {
            serde_json::json!({
                "agent_id": format!("agent-{}", i),
                "objective": format!("task-{}", i)
            })
        })
        .collect();
    let result = batch_tool
        .execute(&serde_json::json!({"subagents": items}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("max 8"));
}

#[test]
fn test_lead_agent_registry_exposes_command_backend_tools() {
    // Verify that command-backend tools registered in the ToolRegistry are
    // reported by command_backend_tool_names() so SandboxManager can treat
    // them as shell-like tools.
    use crate::tools::registry::{RegisteredTool, ToolBackend};

    let registry = ToolRegistry::default();
    // Register a command-backend tool
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "my_cmd_tool".to_string(),
            description: "test command tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::Command {
            command: "echo".to_string(),
            args_template: Some("hello".to_string()),
            timeout_secs: 10,
        },
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    let shell_tools = registry.command_backend_tool_names();
    assert!(
        shell_tools.contains(&"my_cmd_tool".to_string()),
        "command_backend_tool_names() should include command-backend tools. Got: {:?}",
        shell_tools
    );
}

// ── Phase P4: LA-2 batch spawn prompt test ──

#[test]
fn test_lead_agent_prompt_includes_batch_spawn_instruction() {
    let engine = crate::compose::ComposeEngine::new(16);
    let prompt = build_lead_agent_prompt_from_templates(&engine, "Test persona", &[]);
    assert!(
        prompt.contains("spawn_subagents_batch"),
        "Lead agent prompt should mention spawn_subagents_batch tool"
    );
    assert!(
        prompt.contains("3+ independent subagents"),
        "Lead agent prompt should instruct using batch for 3+ subagents"
    );
}

#[test]
fn test_workflow_contract_suffix_gates_interjection_protocol() {
    // Steering attached: both sections, interjection protocol first.
    let with_steering = super::prompt::workflow_contract_suffix(true);
    assert!(with_steering.contains("<interjection_protocol>"));
    assert!(with_steering.contains("<user_interjection>"));
    assert!(with_steering.contains("post_update"));
    assert!(with_steering.contains("queue_followup"));
    assert!(with_steering.contains("<completion_report>"));

    // No steering inbox: completion-report contract only (spec §2b is
    // unconditional; the interjection protocol rides with the rail).
    let without_steering = super::prompt::workflow_contract_suffix(false);
    assert!(!without_steering.contains("<interjection_protocol>"));
    assert!(without_steering.contains("<completion_report>"));
    assert!(without_steering.contains("user-facing completion report"));
}

// ── Phase P4: LA-3 prompt template caching test ──

#[test]
fn test_spawn_subagent_prompt_template_substitution() {
    let template = "\
        <identity>\n{PERSONA}\n</identity>\n\n\
        <scope>\nYou are a subagent.\n</scope>\n\n\
        <constraints>\nIndependent.\n</constraints>{TOOL_GUIDANCE}";

    let result = template
        .replace("{PERSONA}", "I am a researcher")
        .replace("{TOOL_GUIDANCE}", "\n\nTools: search, browse");

    assert!(result.contains("<identity>\nI am a researcher\n</identity>"));
    assert!(result.contains("Tools: search, browse"));
    assert!(!result.contains("{PERSONA}"));
    assert!(!result.contains("{TOOL_GUIDANCE}"));
}

// ── Routing V2: workflow tools (post_update / queue_followup) ──

/// Drain matching events from a broadcast receiver without blocking.
fn recv_all(
    rx: &mut tokio::sync::broadcast::Receiver<crate::events::SystemEvent>,
) -> Vec<crate::events::SystemEvent> {
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    events
}

#[tokio::test]
async fn test_post_update_persists_conversation_and_publishes_event() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let bus = EventBus::default();
    let mut rx = bus.subscribe();

    let tool = PostUpdateTool::new(
        Some(db.clone()),
        bus,
        "task-1".to_string(),
        "junpei:cli".to_string(),
        "cli".to_string(),
    );
    let result = tool
        .execute(&serde_json::json!({"message": "Halfway there"}))
        .await
        .unwrap();
    assert!(result.contains("posted"));

    // Persisted as an assistant message on the lane conversation.
    let conv_repo = openalpaca_storage::ConversationRepository::new(&db);
    let messages = conv_repo.list_recent_by_lane("junpei:cli", 10).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "assistant");
    assert_eq!(messages[0].content, "Halfway there");
    assert_eq!(messages[0].source.as_deref(), Some("cli"));

    // WorkflowProgress published with lane + task identity.
    let events = recv_all(&mut rx);
    assert!(events.iter().any(|ev| matches!(
        ev,
        crate::events::SystemEvent::WorkflowProgress { task_id, lane_key, message, .. }
            if task_id == "task-1" && lane_key == "junpei:cli" && message == "Halfway there"
    )));
}

#[tokio::test]
async fn test_post_update_rejects_empty_message() {
    let tool = PostUpdateTool::new(
        None,
        EventBus::default(),
        "task-1".to_string(),
        "junpei:cli".to_string(),
        "cli".to_string(),
    );
    assert!(tool.execute(&serde_json::json!({})).await.is_err());
    assert!(tool.execute(&serde_json::json!({"message": "  "})).await.is_err());
}

#[tokio::test]
async fn test_queue_followup_inserts_row_and_publishes_event() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let bus = EventBus::default();
    let mut rx = bus.subscribe();

    let tool = QueueFollowupTool::new(
        Some(db.clone()),
        bus,
        "task-1".to_string(),
        "junpei:cli".to_string(),
        "junpei".to_string(),
    );
    let result = tool
        .execute(&serde_json::json!({"description": "Also update the docs"}))
        .await
        .unwrap();
    assert!(result.contains("Follow-up queued"));

    // Row inserted with the reconstructed principal and source task.
    let repo = openalpaca_storage::repository::FollowupRepository::new(&db);
    let rows = repo.list_queued_by_lane("junpei:cli").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, "followup");
    assert_eq!(rows[0].content, "Also update the docs");
    assert_eq!(rows[0].source_task_id.as_deref(), Some("task-1"));
    let principal: crate::security::policy::Principal =
        serde_json::from_str(&rows[0].principal_json).unwrap();
    assert_eq!(
        principal,
        crate::security::policy::Principal::User {
            global_id: "junpei".to_string()
        }
    );

    // FollowupQueued published with the row id.
    let events = recv_all(&mut rx);
    assert!(events.iter().any(|ev| matches!(
        ev,
        crate::events::SystemEvent::FollowupQueued { lane_key, followup_id, kind, .. }
            if lane_key == "junpei:cli" && *followup_id == rows[0].id && kind == "followup"
    )));
}

#[tokio::test]
async fn test_queue_followup_system_owner_maps_to_system_principal() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let tool = QueueFollowupTool::new(
        Some(db.clone()),
        EventBus::default(),
        "task-1".to_string(),
        "system:internal".to_string(),
        "system".to_string(),
    );
    tool.execute(&serde_json::json!({"description": "scheduled follow-up"}))
        .await
        .unwrap();

    let repo = openalpaca_storage::repository::FollowupRepository::new(&db);
    let rows = repo.list_queued_by_lane("system:internal").unwrap();
    let principal: crate::security::policy::Principal =
        serde_json::from_str(&rows[0].principal_json).unwrap();
    assert_eq!(principal, crate::security::policy::Principal::System);
}

#[tokio::test]
async fn test_queue_followup_main_loop_uses_ctx_identity_and_workspace_path() {
    // Main-loop variant (Routing V2 Phase 2): identity + workspace path come
    // from the invocation's ToolContext, no source task recorded.
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let tool = QueueFollowupTool::for_main_loop(
        Some(db.clone()),
        EventBus::default(),
        "junpei:cli".to_string(),
        "fallback-owner".to_string(),
    );
    let ctx = crate::tools::registry::ToolContext {
        owner_id: Some("junpei".to_string()),
        principal: Some(crate::security::policy::Principal::User {
            global_id: "junpei".to_string(),
        }),
        workspace_path: Some("/ws/project".to_string()),
        ..Default::default()
    };
    tool.execute_with_context(&serde_json::json!({"description": "Do it later"}), &ctx)
        .await
        .unwrap();

    let repo = openalpaca_storage::repository::FollowupRepository::new(&db);
    let rows = repo.list_queued_by_lane("junpei:cli").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source_task_id, None);
    assert_eq!(rows[0].workspace_path.as_deref(), Some("/ws/project"));
    let principal: crate::security::policy::Principal =
        serde_json::from_str(&rows[0].principal_json).unwrap();
    assert_eq!(
        principal,
        crate::security::policy::Principal::User {
            global_id: "junpei".to_string()
        }
    );
}

#[tokio::test]
async fn test_queue_followup_ctx_without_principal_falls_back_to_created_by() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let tool = QueueFollowupTool::for_main_loop(
        Some(db.clone()),
        EventBus::default(),
        "junpei:cli".to_string(),
        "junpei".to_string(),
    );
    tool.execute_with_context(
        &serde_json::json!({"description": "later"}),
        &crate::tools::registry::ToolContext::default(),
    )
    .await
    .unwrap();

    let repo = openalpaca_storage::repository::FollowupRepository::new(&db);
    let rows = repo.list_queued_by_lane("junpei:cli").unwrap();
    let principal: crate::security::policy::Principal =
        serde_json::from_str(&rows[0].principal_json).unwrap();
    assert_eq!(
        principal,
        crate::security::policy::Principal::User {
            global_id: "junpei".to_string()
        }
    );
    assert_eq!(rows[0].workspace_path, None);
}

// ── Routing V2: wait_for_subagents steering interrupt ──────────────

fn wait_steering_msg(text: &str) -> crate::runner::steering::SteeringMsg {
    crate::runner::steering::SteeringMsg {
        text: text.to_string(),
        request_id: uuid::Uuid::new_v4(),
        principal: crate::security::policy::Principal::System,
        scope: crate::security::policy::Scope::Global,
        workspace_path: None,
        received_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn test_wait_for_subagents_interrupted_by_steering_push() {
    // A push while the tool is blocked in its select must break the wait.
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-1"); // never completes — wait would block 600s
    let inbox = Arc::new(crate::runner::steering::SteeringInbox::default());
    let tool = Arc::new(WaitForSubagentsTool {
        tracker,
        steering: Some(inbox.clone()),
    });

    let waiter = {
        let tool = tool.clone();
        tokio::spawn(async move { tool.execute(&serde_json::json!({})).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    inbox.push(wait_steering_msg("adjust course")).unwrap();

    let result = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
        .await
        .expect("steering push must interrupt the wait")
        .unwrap()
        .unwrap();
    assert!(result.contains("Wait interrupted"), "got: {result}");
    assert!(result.contains("1 queued"), "got: {result}");
}

#[tokio::test]
async fn test_wait_for_subagents_pre_queued_steering_returns_immediately() {
    // No lost wakeup: a message pushed BEFORE the wait starts must still
    // interrupt within the first iteration (the pre-check catches it).
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-1"); // never completes
    let inbox = Arc::new(crate::runner::steering::SteeringInbox::default());
    inbox.push(wait_steering_msg("queued before wait")).unwrap();
    let tool = WaitForSubagentsTool {
        tracker,
        steering: Some(inbox),
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        tool.execute(&serde_json::json!({})),
    )
    .await
    .expect("pre-queued interjection must interrupt without waiting")
    .unwrap();
    assert!(result.contains("Wait interrupted"), "got: {result}");
}

#[tokio::test]
async fn test_wait_for_subagents_without_steering_completes_normally() {
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-1");
    tracker.complete("run-1", "done".to_string(), true);
    let tool = WaitForSubagentsTool {
        tracker,
        steering: None,
    };
    let result = tool.execute(&serde_json::json!({})).await.unwrap();
    assert!(result.contains("All subagents finished"));
}

#[tokio::test]
async fn test_wait_for_subagents_closed_empty_inbox_does_not_interrupt() {
    // A closed, empty inbox is treated as absent — the wait completes on
    // subagent completion instead of spinning or interrupting.
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-1");
    tracker.complete("run-1", "done".to_string(), true);
    let inbox = Arc::new(crate::runner::steering::SteeringInbox::default());
    inbox.close_and_drain();
    let tool = WaitForSubagentsTool {
        tracker,
        steering: Some(inbox),
    };
    let result = tool.execute(&serde_json::json!({})).await.unwrap();
    assert!(result.contains("All subagents finished"));
}

#[test]
fn test_register_workflow_tools_registers_both() {
    let registry = ToolRegistry::default();
    register_workflow_tools(
        &registry,
        Arc::new(PostUpdateTool::new(
            None,
            EventBus::default(),
            "task-1".to_string(),
            "junpei:cli".to_string(),
            "cli".to_string(),
        )),
        Arc::new(QueueFollowupTool::new(
            None,
            EventBus::default(),
            "task-1".to_string(),
            "junpei:cli".to_string(),
            "junpei".to_string(),
        )),
    );
    let tools = registry.registered_tool_names();
    assert!(tools.contains(&"post_update".to_string()));
    assert!(tools.contains(&"queue_followup".to_string()));
}
