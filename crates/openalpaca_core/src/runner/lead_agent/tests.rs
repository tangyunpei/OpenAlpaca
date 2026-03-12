use super::*;
use crate::agent::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, Skill};

fn make_agent(id: &str, name: &str, skills: &[&str]) -> SubAgent {
    SubAgent {
        id: id.to_string(),
        template_id: id.to_string(),
        name: name.to_string(),
        description: Some(format!("{} agent", name)),
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        skills: skills
            .iter()
            .map(|s| Skill {
                name: s.to_string(),
                category: "test".to_string(),
                proficiency: 0.9,
            })
            .collect(),
        preset: AgentPreset::default(),
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    }
}

#[test]
fn test_lead_agent_tool_executor_routes_correctly() {
    // Test that LeadAgentToolExecutor lists spawn_subagent + contextual tools.
    // We test registered_tools() which only requires the struct, not actual execution.
    use crate::tools::registry::{RegisteredTool, ToolBackend};

    struct NoopTool;

    #[async_trait]
    impl BuiltInTool for NoopTool {
        async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Ok("noop".to_string())
        }
    }

    let mut registry = ToolRegistry::new();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "web_search".to_string(),
            description: "test".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(NoopTool)),
        provides_capabilities: vec![],
    });
    let registry = Arc::new(registry);

    // Build a ContextualToolExecutor with task_id so workspace tools are listed
    let ctx_exec = ToolExecutionContext {
        owner_id: None,
        task_id: Some("task-1".to_string()),
        agent_id: None,
        db: None,
        workspace_id: None,
    };
    let contextual = Arc::new(ContextualToolExecutor::new(registry.clone(), ctx_exec));

    // Build a minimal SpawnSubagentTool — we won't call execute(), just need
    // it for the LeadAgentToolExecutor construction
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
        registry,
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
    ));
    let check_status_tool = Arc::new(CheckSubagentStatusTool {
        tracker: tracker.clone(),
    });
    let wait_tool = Arc::new(WaitForSubagentsTool { tracker });

    let executor =
        LeadAgentToolExecutor::new(spawn_tool, None, check_status_tool, wait_tool, contextual);

    let tools = executor.registered_tools();
    assert!(tools.contains(&"spawn_subagent".to_string()));
    assert!(!tools.contains(&"spawn_subagents_batch".to_string()));
    assert!(tools.contains(&"check_subagent_status".to_string()));
    assert!(tools.contains(&"wait_for_subagents".to_string()));
    assert!(tools.contains(&"web_search".to_string()));
    // workspace tools should be listed since task_id is set
    assert!(tools.contains(&"workspace_read".to_string()));
    assert!(tools.contains(&"workspace_write".to_string()));
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
    use crate::agent::template::AgentTemplateFrontmatter;

    let templates = vec![AgentTemplate {
        frontmatter: AgentTemplateFrontmatter {
            id: "researcher".to_string(),
            name: "Researcher".to_string(),
            description: "Research agent".to_string(),
            icon: None,
            singleton: false,
            skills: vec!["web_search".to_string()],
            denied_skills: vec![],
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
    }];

    let def = spawn_subagents_batch_tool_definition(&templates);
    assert_eq!(def.name, "spawn_subagents_batch");
    assert!(def.description.contains("researcher"));
    assert!(def.description.contains("Researcher"));
}

#[test]
fn test_batch_spawn_tool_hidden_when_disabled() {
    // When batch_spawn_enabled is false, registered_tools should NOT list it
    use crate::tools::registry::{RegisteredTool, ToolBackend};

    struct NoopTool;
    #[async_trait]
    impl BuiltInTool for NoopTool {
        async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Ok("noop".to_string())
        }
    }

    let mut registry = ToolRegistry::new();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "web_search".to_string(),
            description: "test".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(NoopTool)),
        provides_capabilities: vec![],
    });
    let registry = Arc::new(registry);

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
        registry.clone(),
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
        None, // workspace_id
        None, // confirmation_broker
    ));
    let check_tool = Arc::new(CheckSubagentStatusTool {
        tracker: tracker.clone(),
    });
    let wait_tool = Arc::new(WaitForSubagentsTool { tracker });
    let ctx_exec = ToolExecutionContext {
        owner_id: None,
        task_id: Some("task-1".to_string()),
        agent_id: None,
        db: None,
        workspace_id: None,
    };
    let contextual = Arc::new(ContextualToolExecutor::new(registry, ctx_exec));

    // batch_spawn_tool = None -> not in registered_tools
    let executor = LeadAgentToolExecutor::new(spawn_tool, None, check_tool, wait_tool, contextual);
    let tools = executor.registered_tools();
    assert!(!tools.contains(&"spawn_subagents_batch".to_string()));
    assert!(tools.contains(&"spawn_subagent".to_string()));
}

#[test]
fn test_batch_spawn_tool_present_when_enabled() {
    use crate::tools::registry::{RegisteredTool, ToolBackend};

    struct NoopTool;
    #[async_trait]
    impl BuiltInTool for NoopTool {
        async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Ok("noop".to_string())
        }
    }

    let mut registry = ToolRegistry::new();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "web_search".to_string(),
            description: "test".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(NoopTool)),
        provides_capabilities: vec![],
    });
    let registry = Arc::new(registry);

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
        registry.clone(),
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
        None, // workspace_id
        None, // confirmation_broker
    ));
    let batch_tool = Some(Arc::new(SpawnSubagentsBatchTool::new(spawn_tool.clone())));
    let check_tool = Arc::new(CheckSubagentStatusTool {
        tracker: tracker.clone(),
    });
    let wait_tool = Arc::new(WaitForSubagentsTool { tracker });
    let ctx_exec = ToolExecutionContext {
        owner_id: None,
        task_id: Some("task-1".to_string()),
        agent_id: None,
        db: None,
        workspace_id: None,
    };
    let contextual = Arc::new(ContextualToolExecutor::new(registry, ctx_exec));

    // batch_spawn_tool = Some -> IS in registered_tools
    let executor =
        LeadAgentToolExecutor::new(spawn_tool, batch_tool, check_tool, wait_tool, contextual);
    let tools = executor.registered_tools();
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
        Arc::new(ToolRegistry::new()),
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
        Arc::new(ToolRegistry::new()),
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
fn test_lead_agent_executor_delegates_shell_like_tools() {
    // Verify that LeadAgentToolExecutor.shell_like_tools() delegates to the
    // contextual executor instead of returning an empty Vec (the default).
    use crate::tools::registry::{RegisteredTool, ToolBackend};

    struct NoopTool;
    #[async_trait]
    impl BuiltInTool for NoopTool {
        async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Ok("noop".to_string())
        }
    }

    let mut registry = ToolRegistry::new();
    // Register a command-backend tool so command_backend_tool_names() is non-empty
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
    });
    let registry = Arc::new(registry);

    let ctx_exec = ToolExecutionContext {
        owner_id: None,
        task_id: Some("task-1".to_string()),
        agent_id: None,
        db: None,
        workspace_id: None,
    };
    let contextual = Arc::new(ContextualToolExecutor::new(registry.clone(), ctx_exec));

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
        registry,
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
        None, // confirmation_broker
    ));
    let check_tool = Arc::new(CheckSubagentStatusTool {
        tracker: tracker.clone(),
    });
    let wait_tool = Arc::new(WaitForSubagentsTool { tracker });

    let executor = LeadAgentToolExecutor::new(spawn_tool, None, check_tool, wait_tool, contextual);

    // shell_like_tools() should include "my_cmd_tool" from the registry
    let shell_tools = executor.shell_like_tools();
    assert!(
        shell_tools.contains(&"my_cmd_tool".to_string()),
        "LeadAgentToolExecutor.shell_like_tools() should delegate to contextual executor \
         and include command-backend tools. Got: {:?}",
        shell_tools
    );
}

// ── Phase P4: LA-2 batch spawn prompt test ──

#[test]
fn test_lead_agent_prompt_includes_batch_spawn_instruction() {
    let prompt = build_lead_agent_prompt_from_templates("Test persona", &[]);
    assert!(
        prompt.contains("spawn_subagents_batch"),
        "Lead agent prompt should mention spawn_subagents_batch tool"
    );
    assert!(
        prompt.contains("3+ independent subagents"),
        "Lead agent prompt should instruct using batch for 3+ subagents"
    );
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
