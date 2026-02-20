use super::*;
use crate::agent::subagent::{
    AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Skill, SubAgent,
};
use crate::agent::template::{AgentTemplate, AgentTemplateFrontmatter};
use crate::events::SystemEvent;
use crate::security::policy::{Principal, Scope};
use crate::security::sandbox::SandboxManager;
use crate::tools::{RegistryToolExecutor, ToolRegistry};
use std::collections::HashMap;
use uuid::Uuid;

fn make_tool_registry() -> Arc<ToolRegistry> {
    Arc::new(ToolRegistry::new())
}

fn make_security_gate(bus: &EventBus) -> Arc<SecurityGate> {
    let registry = make_tool_registry();
    let executor = Arc::new(RegistryToolExecutor::new(registry));
    let sandbox = Arc::new(SandboxManager::with_defaults(executor, bus.clone()));
    Arc::new(SecurityGate::new(sandbox))
}

fn make_orchestrator() -> Orchestrator {
    let ctx = Arc::new(SharedContext::new());
    let lanes = Arc::new(LaneManager::new());
    let bus = EventBus::default();
    let gate = make_security_gate(&bus);
    let registry = make_tool_registry();
    Orchestrator::new(
        ctx,
        lanes,
        bus,
        SystemPersona::default(),
        None,
        LoopConfig::default(),
        gate,
        registry,
        None,
        None,
        Arc::new(skill_catalog::SkillCatalog::new()),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
    )
}

fn make_orchestrator_with_agents(agents: Vec<SubAgent>) -> Orchestrator {
    let ctx = Arc::new(SharedContext::new());
    for a in &agents {
        ctx.agent_registry.register_template(template_from_agent(a));
        ctx.agent_registry.register(a.clone());
    }
    let lanes = Arc::new(LaneManager::new());
    let bus = EventBus::default();
    let gate = make_security_gate(&bus);
    let registry = make_tool_registry();
    Orchestrator::new(
        ctx,
        lanes,
        bus,
        SystemPersona::default(),
        None,
        LoopConfig::default(),
        gate,
        registry,
        None,
        None,
        Arc::new(skill_catalog::SkillCatalog::new()),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
    )
}

#[test]
fn test_update_system_persona_updates_active_snapshot() {
    let orch = make_orchestrator();
    let mut replacement = SystemPersona::default();
    replacement.name = "Soul Reloaded".to_string();

    orch.update_system_persona(replacement.clone());

    let active = orch
        .system_persona
        .read()
        .expect("system_persona lock should be readable")
        .clone();
    assert_eq!(active.name, replacement.name);
}

fn make_agent(id: &str, skills: Vec<&str>) -> SubAgent {
    SubAgent {
        id: id.to_string(),
        template_id: id.to_string(),
        name: format!("Agent {}", id),
        description: Some(format!("{} agent", id)),
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        skills: skills
            .into_iter()
            .map(|s| Skill {
                name: s.to_string(),
                category: "test".to_string(),
                proficiency: 1.0,
            })
            .collect(),
        preset: AgentPreset::default(),
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    }
}

/// Create a minimal AgentTemplate from a SubAgent (for test setup).
fn template_from_agent(agent: &SubAgent) -> AgentTemplate {
    let is_lead = agent.skills.iter().any(|s| s.name == "lead_orchestration");
    AgentTemplate {
        frontmatter: AgentTemplateFrontmatter {
            id: agent.template_id.clone(),
            name: agent.name.clone(),
            description: agent.description.clone().unwrap_or_default(),
            icon: agent.icon.clone(),
            singleton: is_lead,
            skills: agent.skills.iter().map(|s| s.name.clone()).collect(),
            denied_skills: vec![],
            temperature: agent.preset.temperature,
            verbosity: agent.preset.verbosity.clone(),
            model: agent.llm_config.model.clone(),
            fallback_models: agent.llm_config.fallback_models.clone(),
            max_tool_calls: agent.constraints.max_tool_calls,
            timeout_seconds: agent.constraints.timeout_seconds,
            max_cost_per_task: agent.constraints.max_cost_per_task,
            require_confirmation_for: agent.constraints.require_confirmation_for.clone(),
        },
        body: String::new(),
        sections: HashMap::new(),
    }
}

#[tokio::test]
async fn test_simple_query_echo() {
    let orch = make_orchestrator();
    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "hello world".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        )
        .await;
    assert!(result.is_ok());
    let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(json["status"], "ok");
    assert!(json["echo"].as_str().unwrap().contains("hello world"));
}

#[tokio::test]
async fn test_task_query_empty() {
    let orch = make_orchestrator();
    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "/status".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        )
        .await;
    assert!(result.is_ok());
    let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(json["count"], 0);
}

#[tokio::test]
async fn test_complex_task_dispatch() {
    let orch = make_orchestrator_with_agents(vec![
        make_agent("a1", vec!["web_search"]),
        make_agent("a2", vec!["text_generate"]),
    ]);
    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "please research and write about Rust".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        )
        .await;
    assert!(result.is_ok());
    let text = result.unwrap();
    // Response is now human-readable, not JSON
    assert!(text.contains("assigned"));
}

#[tokio::test]
async fn test_task_control_cancel() {
    let orch = make_orchestrator();
    // Register a task first
    orch.shared_context
        .task_registry
        .register("t1".to_string(), "test task".to_string());

    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "/cancel t1".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        )
        .await;
    assert!(result.is_ok());
    let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(json["new_status"], "cancelled");
}

#[tokio::test]
async fn test_permission_denied_external() {
    let orch = make_orchestrator();
    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "telegram".to_string(),
            "hello".to_string(),
            Principal::External {
                provider: "telegram".to_string(),
                id: "unknown".to_string(),
            },
            Scope::Global,
            "unknown:telegram".to_string(),
        )
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Permission Denied"));
}

#[tokio::test]
async fn test_full_lifecycle_events() {
    let orch = make_orchestrator_with_agents(vec![make_agent("a1", vec!["web_search"])]);
    let mut rx = orch.bus.subscribe();

    // Send a complex task
    let _result = orch
        .handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "can you search for Rust tutorials".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        )
        .await;

    // Collect events
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    // Should have: IntentClassified, AgentStatusChanged, TaskCreated
    let event_types: Vec<String> = events
        .iter()
        .map(|e| match e {
            SystemEvent::IntentClassified { .. } => "intent_classified".to_string(),
            SystemEvent::AgentStatusChanged { .. } => "agent_status_changed".to_string(),
            SystemEvent::TaskCreated { .. } => "task_created".to_string(),
            other => format!("{:?}", other),
        })
        .collect();

    assert!(
        event_types.contains(&"intent_classified".to_string()),
        "Missing IntentClassified event. Got: {:?}",
        event_types
    );
    assert!(
        event_types.contains(&"task_created".to_string()),
        "Missing TaskCreated event. Got: {:?}",
        event_types
    );
}

#[tokio::test]
async fn test_simple_query_with_mock_llm() {
    use async_trait::async_trait;
    use openalpaca_llm::{
        ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, LlmRouter, ProviderType,
        Usage,
    };

    struct MockLlm;

    #[async_trait]
    impl LlmProvider for MockLlm {
        fn name(&self) -> &str {
            "mock"
        }
        fn supports_tools(&self) -> bool {
            false
        }
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
            Ok(ChatResponse {
                content: r#"{"status": "ok", "answer": "Mock LLM response"}"#.to_string(),
                tool_calls: vec![],
                model: "mock-model".to_string(),
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 20,
                    ..Default::default()
                },
                finish_reason: FinishReason::Stop,
            })
        }
    }

    let ctx = Arc::new(SharedContext::new());
    let lanes = Arc::new(LaneManager::new());
    let bus = EventBus::default();
    let gate = make_security_gate(&bus);
    let registry = make_tool_registry();
    let router = LlmRouter::single_provider(
        Arc::new(MockLlm),
        ProviderType::Anthropic,
        "claude-sonnet-4-5-20250929".to_string(),
    );
    let orch = Orchestrator::new(
        ctx,
        lanes,
        bus,
        SystemPersona::default(),
        Some(Arc::new(router)),
        LoopConfig::default(),
        gate,
        registry,
        None,
        None,
        Arc::new(skill_catalog::SkillCatalog::new()),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
    );

    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "What is Rust?".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        )
        .await;
    assert!(result.is_ok());
    let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["answer"], "Mock LLM response");
}

#[tokio::test]
async fn test_input_sanitization_blocks_null_bytes() {
    let orch = make_orchestrator();
    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "hello\0world".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        )
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("null bytes"));
}

#[tokio::test]
async fn test_security_gate_replaces_trust_gate() {
    // Verify that SecurityGate (wrapping TrustGate) still blocks external users
    let orch = make_orchestrator();
    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "telegram".to_string(),
            "hello".to_string(),
            Principal::External {
                provider: "telegram".to_string(),
                id: "unknown".to_string(),
            },
            Scope::Global,
            "unknown:telegram".to_string(),
        )
        .await;
    assert!(result.is_err());
    // SecurityGate wraps TrustGate error as "Access denied: Permission Denied: ..."
    assert!(result.unwrap_err().contains("denied"));
}

// --- LLM Task Planning integration tests ---

/// Helper: create a mock LLM that returns a fixed response string.
fn make_planning_mock_llm(response: &str) -> Arc<LlmRouter> {
    use async_trait::async_trait;
    use openalpaca_llm::{ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, Usage};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct PlanningMockLlm {
        response: String,
        call_count: AtomicUsize,
    }

    #[async_trait]
    impl LlmProvider for PlanningMockLlm {
        fn name(&self) -> &str {
            "planning-mock"
        }
        fn supports_tools(&self) -> bool {
            false
        }
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(ChatResponse {
                content: self.response.clone(),
                tool_calls: vec![],
                model: "mock-model".to_string(),
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 20,
                    ..Default::default()
                },
                finish_reason: FinishReason::Stop,
            })
        }
    }

    let mock = PlanningMockLlm {
        response: response.to_string(),
        call_count: AtomicUsize::new(0),
    };
    let router = openalpaca_llm::LlmRouter::single_provider(
        Arc::new(mock),
        openalpaca_llm::ProviderType::Anthropic,
        "claude-sonnet-4-5-20250929".to_string(),
    );
    Arc::new(router)
}

fn make_orchestrator_with_llm_and_agents(
    router: Arc<LlmRouter>,
    agents: Vec<SubAgent>,
) -> Orchestrator {
    let ctx = Arc::new(SharedContext::new());
    for a in &agents {
        ctx.agent_registry.register_template(template_from_agent(a));
        ctx.agent_registry.register(a.clone());
    }
    let lanes = Arc::new(LaneManager::new());
    let bus = EventBus::default();
    let gate = make_security_gate(&bus);
    let registry = make_tool_registry();
    Orchestrator::new(
        ctx,
        lanes,
        bus,
        SystemPersona::default(),
        Some(router),
        LoopConfig::default(),
        gate,
        registry,
        None,
        None,
        Arc::new(skill_catalog::SkillCatalog::new()),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
    )
}

#[tokio::test]
async fn test_llm_planning_complex_task() {
    let plan_json = r#"{"classification": "complex_task", "title": "Research Rust patterns", "assignments": [{"agent_id": "a1", "agent_name": "Agent a1", "role_description": "Research agent", "matched_skills": ["web_search"]}], "reasoning": "User wants research"}"#;
    let router = make_planning_mock_llm(plan_json);
    let orch =
        make_orchestrator_with_llm_and_agents(router, vec![make_agent("a1", vec!["web_search"])]);

    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "research Rust async patterns".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        )
        .await;

    assert!(result.is_ok());
    let text = result.unwrap();
    assert!(
        text.contains("assigned"),
        "Expected 'assigned' in: {}",
        text
    );

    // Verify task is registered
    assert_eq!(orch.shared_context.task_registry.count(), 1);
}

#[tokio::test]
async fn test_llm_planning_simple_query() {
    let plan_json = r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "This is a greeting"}"#;
    let router = make_planning_mock_llm(plan_json);
    let orch = make_orchestrator_with_llm_and_agents(router, vec![]);

    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "hello".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        )
        .await;

    assert!(result.is_ok());
    // Should NOT dispatch a task
    assert_eq!(orch.shared_context.task_registry.count(), 0);
}

#[tokio::test]
async fn test_llm_planning_fallback_on_malformed() {
    // LLM returns garbage — should fall back to keyword heuristic
    let router = make_planning_mock_llm("this is not valid json at all");
    let orch =
        make_orchestrator_with_llm_and_agents(router, vec![make_agent("a1", vec!["web_search"])]);

    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "can you search for Rust tutorials".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        )
        .await;

    // Should still work via heuristic fallback
    assert!(result.is_ok());
    let text = result.unwrap();
    assert!(
        text.contains("assigned"),
        "Expected heuristic fallback to dispatch. Got: {}",
        text
    );
}

#[tokio::test]
async fn test_slash_commands_bypass_llm() {
    use async_trait::async_trait;
    use openalpaca_llm::{ChatRequest, ChatResponse, LlmError, LlmProvider};

    // Mock LLM that panics if called — slash commands must bypass it
    struct PanickingLlm;

    #[async_trait]
    impl LlmProvider for PanickingLlm {
        fn name(&self) -> &str {
            "panicking"
        }
        fn supports_tools(&self) -> bool {
            false
        }
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
            panic!("LLM should not be called for slash commands");
        }
    }

    let router = openalpaca_llm::LlmRouter::single_provider(
        Arc::new(PanickingLlm),
        openalpaca_llm::ProviderType::Anthropic,
        "claude-sonnet-4-5-20250929".to_string(),
    );
    let orch = make_orchestrator_with_llm_and_agents(Arc::new(router), vec![]);

    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "/status".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        )
        .await;

    assert!(result.is_ok());
    let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(json["count"], 0);
}

// --- Tool-capable simple_query + dispatch fallback tests ---

use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};

fn make_security_gate_with_registry(
    bus: &EventBus,
    registry: Arc<ToolRegistry>,
) -> Arc<SecurityGate> {
    let executor = Arc::new(RegistryToolExecutor::new(registry));
    let sandbox = Arc::new(SandboxManager::with_defaults(executor, bus.clone()));
    Arc::new(SecurityGate::new(sandbox))
}

struct MockBuiltInTool;

#[async_trait::async_trait]
impl BuiltInTool for MockBuiltInTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Ok("mock tool result".to_string())
    }
}

fn make_mock_tool(name: &str) -> RegisteredTool {
    RegisteredTool {
        definition: openalpaca_llm::ToolDefinition {
            name: name.to_string(),
            description: format!("{} tool", name),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltInTool)),
    }
}

fn make_orchestrator_with_tools_and_llm(
    router: Arc<LlmRouter>,
    tool_names: &[&str],
) -> Orchestrator {
    let mut registry = ToolRegistry::new();
    for name in tool_names {
        registry.register(make_mock_tool(name));
    }
    let registry = Arc::new(registry);
    let ctx = Arc::new(SharedContext::new());
    let lanes = Arc::new(LaneManager::new());
    let bus = EventBus::default();
    let gate = make_security_gate_with_registry(&bus, registry.clone());
    Orchestrator::new(
        ctx,
        lanes,
        bus,
        SystemPersona::default(),
        Some(router),
        LoopConfig::default(),
        gate,
        registry,
        None,
        None,
        Arc::new(skill_catalog::SkillCatalog::new()),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
    )
}

#[tokio::test]
async fn test_tool_intent_detected_and_executes() {
    use openalpaca_llm::{
        ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, ToolCall as LlmToolCall,
        Usage,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ToolMockLlm {
        call_count: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmProvider for ToolMockLlm {
        fn name(&self) -> &str {
            "tool-mock"
        }
        fn supports_tools(&self) -> bool {
            true
        }
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
            let n = self.call_count.fetch_add(1, Ordering::SeqCst);
            match n {
                // Call 0: planner call — return simple_query classification
                0 => Ok(ChatResponse {
                    content: r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "User wants to fetch a URL"}"#.to_string(),
                    tool_calls: vec![],
                    model: "mock-model".to_string(),
                    usage: Usage { input_tokens: 10, output_tokens: 20, ..Default::default() },
                    finish_reason: FinishReason::Stop,
                }),
                // Call 1: agentic loop — return tool use
                1 => Ok(ChatResponse {
                    content: String::new(),
                    tool_calls: vec![LlmToolCall {
                        id: "tc_1".to_string(),
                        name: "web_fetch".to_string(),
                        arguments: serde_json::json!({"url": "https://example.com"}),
                    }],
                    model: "mock-model".to_string(),
                    usage: Usage { input_tokens: 10, output_tokens: 20, ..Default::default() },
                    finish_reason: FinishReason::ToolUse,
                }),
                // Call 2+: return final answer with Stop
                _ => Ok(ChatResponse {
                    content: "Here is the fetched content from example.com.".to_string(),
                    tool_calls: vec![],
                    model: "mock-model".to_string(),
                    usage: Usage { input_tokens: 10, output_tokens: 20, ..Default::default() },
                    finish_reason: FinishReason::Stop,
                }),
            }
        }
    }

    let mock = ToolMockLlm {
        call_count: AtomicUsize::new(0),
    };
    let router = openalpaca_llm::LlmRouter::single_provider(
        Arc::new(mock),
        openalpaca_llm::ProviderType::Anthropic,
        "claude-sonnet-4-5-20250929".to_string(),
    );
    let orch = make_orchestrator_with_tools_and_llm(Arc::new(router), &["web_fetch"]);

    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "fetch https://example.com".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        )
        .await;

    assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
    let content = result.unwrap();
    assert!(!content.is_empty(), "Expected non-empty response");
}

#[tokio::test]
async fn test_tool_max_rounds_enforcement() {
    use openalpaca_llm::{
        ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, ToolCall as LlmToolCall,
        Usage,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct AlwaysToolUseLlm {
        call_count: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmProvider for AlwaysToolUseLlm {
        fn name(&self) -> &str {
            "always-tool"
        }
        fn supports_tools(&self) -> bool {
            true
        }
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
            let n = self.call_count.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                // Planner call
                return Ok(ChatResponse {
                    content: r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "simple"}"#.to_string(),
                    tool_calls: vec![],
                    model: "mock-model".to_string(),
                    usage: Usage { input_tokens: 10, output_tokens: 20, ..Default::default() },
                    finish_reason: FinishReason::Stop,
                });
            }
            // Always return ToolUse
            Ok(ChatResponse {
                content: String::new(),
                tool_calls: vec![LlmToolCall {
                    id: format!("tc_{}", n),
                    name: "web_fetch".to_string(),
                    arguments: serde_json::json!({"url": "https://example.com"}),
                }],
                model: "mock-model".to_string(),
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 20,
                    ..Default::default()
                },
                finish_reason: FinishReason::ToolUse,
            })
        }
    }

    let mock = AlwaysToolUseLlm {
        call_count: AtomicUsize::new(0),
    };
    let router = openalpaca_llm::LlmRouter::single_provider(
        Arc::new(mock),
        openalpaca_llm::ProviderType::Anthropic,
        "claude-sonnet-4-5-20250929".to_string(),
    );
    let orch = make_orchestrator_with_tools_and_llm(Arc::new(router), &["web_fetch"]);

    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "fetch https://example.com".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        )
        .await;

    // Should complete without hanging (max_rounds=4 cap kicks in)
    assert!(
        result.is_ok(),
        "Expected Ok (max_rounds should cap), got: {:?}",
        result
    );
}

#[tokio::test]
async fn test_tool_intent_but_not_in_registry() {
    // Query triggers web_fetch suggestion but registry is empty — graceful degradation
    let plan_json = r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "simple"}"#;
    let router = make_planning_mock_llm(plan_json);
    // Build orchestrator with NO tools in registry
    let orch = make_orchestrator_with_llm_and_agents(router, vec![]);

    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "fetch https://example.com".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        )
        .await;

    // Should succeed without error — just proceeds tool-less
    assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
}

#[tokio::test]
async fn test_dispatch_error_falls_back_to_simple_query() {
    // Planner returns complex_task with nonexistent agent → dispatch fails → fallback to simple_query
    let plan_json = r#"{"classification": "complex_task", "title": "Do something", "assignments": [{"agent_id": "nonexistent_agent", "agent_name": "Ghost", "role_description": "Ghost role", "matched_skills": ["web_search"]}], "reasoning": "complex"}"#;
    let router = make_planning_mock_llm(plan_json);
    // No agents registered → dispatch_planned will fail
    let orch = make_orchestrator_with_llm_and_agents(router, vec![]);

    let result = orch
        .handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "do something complex".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        )
        .await;

    // Should succeed via fallback to simple_query (echo stub since mock LLM returns plan JSON)
    assert!(
        result.is_ok(),
        "Expected Ok via fallback, got: {:?}",
        result
    );
    // No tasks should be registered (dispatch failed)
    assert_eq!(orch.shared_context.task_registry.count(), 0);
}
