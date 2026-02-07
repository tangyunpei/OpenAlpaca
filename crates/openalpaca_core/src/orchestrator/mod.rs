//! Orchestrator module: the central message handler replacing CoreCtx.
//!
//! Routes user messages through intent classification, skill matching,
//! and task dispatch pipelines.

pub mod dispatcher;
pub mod intent;
pub mod skill_matcher;
pub mod task_planner;

use crate::bus::EventBus;
use crate::context::{SharedContext, TaskEntry, TaskEntryStatus};
use crate::events::SystemEvent;
use crate::lane::{LaneManager, TaskLaneStatus};
use crate::middleware::guard::OutputGuard;
use crate::middleware::prompt::{AgentPersona, PromptAssembler, SystemPersona};
use crate::runner::{LoopConfig, run_agentic_loop_routed};
use crate::security::gate::SecurityGate;
use crate::security::policy::{Principal, Scope};
use crate::tools::ToolRegistry;
use crate::types::Capability;
use chrono::Utc;
use openalpaca_llm::{ChatMessage, LlmRouter};
use openalpaca_storage::{ConversationRepository, Database};
use std::sync::Arc;
use uuid::Uuid;

use dispatcher::TaskDispatcher;
use intent::{Intent, IntentParser};
use task_planner::TaskPlanner;

/// The Orchestrator: unified message handler for all user interactions.
///
/// Replaces CoreCtx with intent-based routing:
/// - SimpleQuery → LLM call (or echo stub if no LLM configured)
/// - TaskQuery → query task registry
/// - ComplexTask → dispatch to agents via TaskDispatcher
/// - TaskControl → manage task lifecycle
pub struct Orchestrator {
    pub shared_context: Arc<SharedContext>,
    pub lane_manager: Arc<LaneManager>,
    pub bus: EventBus,
    pub system_persona: SystemPersona,
    pub llm_router: Option<Arc<LlmRouter>>,
    pub loop_config: LoopConfig,
    pub security_gate: Arc<SecurityGate>,
    pub tool_registry: Arc<ToolRegistry>,
    intent_parser: IntentParser,
    task_dispatcher: TaskDispatcher,
    db: Option<Database>,
}

const MAX_HISTORY_MESSAGES: i64 = 40;

impl Orchestrator {
    pub fn new(
        shared_context: Arc<SharedContext>,
        lane_manager: Arc<LaneManager>,
        bus: EventBus,
        system_persona: SystemPersona,
        llm_router: Option<Arc<LlmRouter>>,
        loop_config: LoopConfig,
        security_gate: Arc<SecurityGate>,
        tool_registry: Arc<ToolRegistry>,
        db: Option<Database>,
    ) -> Self {
        let task_dispatcher = TaskDispatcher::new(
            shared_context.clone(),
            lane_manager.clone(),
            bus.clone(),
            llm_router.clone(),
            security_gate.clone(),
            tool_registry.clone(),
            db.clone(),
        );
        Self {
            shared_context,
            lane_manager,
            bus,
            system_persona,
            llm_router,
            loop_config,
            security_gate,
            tool_registry,
            intent_parser: IntentParser,
            task_dispatcher,
            db,
        }
    }

    fn load_history(&self, lane_key: &str) -> Vec<ChatMessage> {
        let db = match &self.db {
            Some(db) => db,
            None => return Vec::new(),
        };
        let repo = ConversationRepository::new(db);
        match repo.list_recent_by_lane(lane_key, MAX_HISTORY_MESSAGES) {
            Ok(messages) => messages
                .into_iter()
                .filter_map(|msg| match msg.role.as_str() {
                    "user" => Some(ChatMessage::user(&msg.content)),
                    "assistant" => Some(ChatMessage::assistant(&msg.content)),
                    _ => None,
                })
                .collect(),
            Err(e) => {
                tracing::warn!("Failed to load conversation history: {e}");
                Vec::new()
            }
        }
    }

    /// Handle a user message through the full pipeline:
    /// 1. SecurityGate permission check (wraps TrustGate)
    /// 2. Input sanitization
    /// 3. Try slash commands / task queries (cheap, no LLM)
    /// 4. If LLM router configured: try LLM-based planning
    /// 5. Fallback: keyword heuristic routing
    pub async fn handle_message(
        &self,
        request_id: Uuid,
        source: String,
        content: String,
        principal: Principal,
        scope: Scope,
        lane_key: String,
    ) -> Result<String, String> {
        // 1. Permission check via SecurityGate (wraps TrustGate)
        let capability = Capability {
            name: "chat.respond".to_string(),
        };
        SecurityGate::check_access(&principal, &capability, &scope)?;

        // 2. Input sanitization
        let content = SecurityGate::sanitize_input(&content)?;

        // 3. Try slash commands and task queries first (cheap, always correct)
        let intent = self.intent_parser.parse(&content);
        match &intent {
            Intent::TaskQuery { .. } | Intent::TaskControl { .. } => {
                // Emit IntentClassified event
                self.bus.publish(SystemEvent::IntentClassified {
                    request_id,
                    intent_type: intent.intent_type().to_string(),
                    timestamp: Utc::now(),
                });
                return match intent {
                    Intent::TaskQuery { task_id } => self.handle_task_query(task_id),
                    Intent::TaskControl { task_id, action } => {
                        self.handle_task_control(&task_id, &action)
                    }
                    _ => unreachable!(),
                };
            }
            _ => {}
        }

        // 4. If LLM router is configured, try LLM-based planning
        if let Some(ref router) = self.llm_router {
            let idle_agents = self.shared_context.agent_registry.list_idle();
            let history = self.load_history(&lane_key);
            match TaskPlanner::plan(router, &content, &idle_agents, &history).await {
                Ok(plan) => {
                    match plan.classification.as_str() {
                        "simple_query" => {
                            self.bus.publish(SystemEvent::IntentClassified {
                                request_id,
                                intent_type: "simple_query".to_string(),
                                timestamp: Utc::now(),
                            });
                            return self
                                .handle_simple_query(request_id, &source, &content, &lane_key)
                                .await;
                        }
                        "complex_task" => {
                            self.bus.publish(SystemEvent::IntentClassified {
                                request_id,
                                intent_type: "complex_task".to_string(),
                                timestamp: Utc::now(),
                            });
                            return self.task_dispatcher.dispatch_planned(
                                &content,
                                plan,
                                &principal_id(&principal),
                                &lane_key,
                            );
                        }
                        other => {
                            tracing::warn!(
                                "LLM planner returned unknown classification '{}', falling back to heuristic",
                                other
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("LLM planning failed: {}, falling back to heuristic", e);
                }
            }
        }

        // 5. Fallback: keyword heuristic routing
        self.dispatch_with_heuristic(request_id, &source, &content, &principal, &lane_key)
            .await
    }

    /// Fallback dispatch using keyword-based intent classification and greedy skill matching.
    async fn dispatch_with_heuristic(
        &self,
        request_id: Uuid,
        source: &str,
        content: &str,
        principal: &Principal,
        lane_key: &str,
    ) -> Result<String, String> {
        let intent = self.intent_parser.parse(content);

        self.bus.publish(SystemEvent::IntentClassified {
            request_id,
            intent_type: intent.intent_type().to_string(),
            timestamp: Utc::now(),
        });

        match intent {
            Intent::SimpleQuery { query } => {
                self.handle_simple_query(request_id, source, &query, lane_key).await
            }
            Intent::TaskQuery { task_id } => self.handle_task_query(task_id),
            Intent::ComplexTask {
                description,
                required_skills,
            } => self.task_dispatcher.dispatch(
                request_id,
                source,
                &description,
                &required_skills,
                &principal_id(principal),
                lane_key,
            ),
            Intent::TaskControl { task_id, action } => {
                self.handle_task_control(&task_id, &action)
            }
        }
    }

    async fn handle_simple_query(
        &self,
        request_id: Uuid,
        _source: &str,
        query: &str,
        lane_key: &str,
    ) -> Result<String, String> {
        let agent_persona = AgentPersona {
            role: "Assistant".to_string(),
            tone: "Friendly".to_string(),
            domain_knowledge: vec![],
        };
        let full_prompt =
            PromptAssembler::assemble(&self.system_persona, &agent_persona, query);

        let (response_content, is_structured) = if let Some(ref router) = self.llm_router {
            // Real LLM call via routed agentic loop
            let history = self.load_history(lane_key);
            let mut messages = Vec::with_capacity(2 + history.len());
            messages.push(ChatMessage::system(&full_prompt));
            messages.extend(history);
            messages.push(ChatMessage::user(query));
            let result = run_agentic_loop_routed(
                router.as_ref(),
                messages,
                vec![], // No tools for simple queries
                &self.loop_config,
                Some(self.security_gate.sandbox()),
                "orchestrator",
                None, // No policy for simple queries (no tools)
                None, // No task_id for simple queries
            )
            .await;
            // LLM chat responses are free-form text, not structured JSON
            (result.final_content, false)
        } else {
            // Fallback: echo stub (backward compatible) — produces JSON
            (format!(
                "{{\"status\": \"ok\", \"echo\": \"Received: {}\"}}",
                query.chars().take(50).collect::<String>()
            ), true)
        };

        // Output guard: only enforce JSON for structured (non-LLM) responses
        let validated = if is_structured {
            OutputGuard::ensure_json(&response_content)?
        } else {
            response_content
        };

        // Emit AgentResponse event
        self.bus.publish(SystemEvent::AgentResponse {
            request_id,
            agent_id: "orchestrator".to_string(),
            content: validated.clone(),
            timestamp: Utc::now(),
        });

        Ok(validated)
    }

    fn handle_task_query(&self, task_id: Option<String>) -> Result<String, String> {
        match task_id {
            Some(id) => {
                match self.shared_context.task_registry.get(&id) {
                    Some(entry) => Ok(task_entry_to_json(&entry)),
                    None => Ok(serde_json::json!({
                        "error": "not_found",
                        "message": format!("Task '{}' not found", id)
                    })
                    .to_string()),
                }
            }
            None => {
                let active = self.shared_context.task_registry.list_active();
                let tasks: Vec<serde_json::Value> =
                    active.iter().map(|e| serde_json::json!({
                        "task_id": e.task_id,
                        "title": e.title,
                        "status": e.status.as_str(),
                    })).collect();
                Ok(serde_json::json!({
                    "tasks": tasks,
                    "count": tasks.len(),
                })
                .to_string())
            }
        }
    }

    fn handle_task_control(&self, task_id: &str, action: &str) -> Result<String, String> {
        // Fetch current state
        let entry = self
            .shared_context
            .task_registry
            .get(task_id)
            .ok_or_else(|| format!("Task '{}' not found", task_id))?;

        // Validate state transition
        let new_status = match action {
            "cancel" => {
                if entry.status.is_terminal() {
                    return Err(format!(
                        "Cannot cancel task in '{}' state",
                        entry.status.as_str()
                    ));
                }
                TaskEntryStatus::Cancelled
            }
            "pause" => {
                if entry.status != TaskEntryStatus::Running {
                    return Err(format!(
                        "Can only pause a running task, current: '{}'",
                        entry.status.as_str()
                    ));
                }
                TaskEntryStatus::Paused
            }
            "resume" => {
                if entry.status != TaskEntryStatus::Paused {
                    return Err(format!(
                        "Can only resume a paused task, current: '{}'",
                        entry.status.as_str()
                    ));
                }
                TaskEntryStatus::Running
            }
            _ => return Err(format!("Unknown action: '{}'", action)),
        };

        // Update task registry
        self.shared_context
            .task_registry
            .update_status(task_id, new_status);

        // Update task lane if present
        if let Some(lane) = self.lane_manager.get_task_lane(task_id) {
            let lane_status = match new_status {
                TaskEntryStatus::Queued => TaskLaneStatus::Queued,
                TaskEntryStatus::Running => TaskLaneStatus::Running,
                TaskEntryStatus::Completed => TaskLaneStatus::Completed,
                TaskEntryStatus::Failed => TaskLaneStatus::Failed,
                TaskEntryStatus::Cancelled => TaskLaneStatus::Cancelled,
                TaskEntryStatus::Paused => TaskLaneStatus::Paused,
            };
            lane.set_status(lane_status);
        }

        // Emit TaskUpdated event
        self.bus.publish(SystemEvent::TaskUpdated {
            task_id: task_id.to_string(),
            status: new_status.as_str().to_string(),
            progress_current: None,
            progress_total: None,
            timestamp: Utc::now(),
        });

        Ok(serde_json::json!({
            "task_id": task_id,
            "action": action,
            "new_status": new_status.as_str(),
        })
        .to_string())
    }
}

fn principal_id(principal: &Principal) -> String {
    match principal {
        Principal::System => "system".to_string(),
        Principal::User { global_id } => global_id.clone(),
        Principal::External { provider, id } => format!("{}:{}", provider, id),
    }
}

fn task_entry_to_json(entry: &TaskEntry) -> String {
    serde_json::json!({
        "task_id": entry.task_id,
        "title": entry.title,
        "status": entry.status.as_str(),
        "created_at": entry.created_at.to_rfc3339(),
        "updated_at": entry.updated_at.to_rfc3339(),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Skill, SubAgent};
    use crate::security::sandbox::SandboxManager;
    use crate::tools::{RegistryToolExecutor, ToolRegistry};

    fn make_tool_registry() -> Arc<ToolRegistry> {
        Arc::new(ToolRegistry::new())
    }

    fn make_security_gate(bus: &EventBus) -> Arc<SecurityGate> {
        let registry = make_tool_registry();
        let executor = Arc::new(RegistryToolExecutor::new(registry));
        let sandbox = Arc::new(SandboxManager::new(executor, bus.clone()));
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
        )
    }

    fn make_orchestrator_with_agents(agents: Vec<SubAgent>) -> Orchestrator {
        let ctx = Arc::new(SharedContext::new());
        for a in agents {
            ctx.agent_registry.register(a);
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
        )
    }

    fn make_agent(id: &str, skills: Vec<&str>) -> SubAgent {
        SubAgent {
            id: id.to_string(),
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
        use openalpaca_llm::{ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, LlmRouter, ProviderType, Usage};

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
        for a in agents {
            ctx.agent_registry.register(a);
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
        )
    }

    #[tokio::test]
    async fn test_llm_planning_complex_task() {
        let plan_json = r#"{"classification": "complex_task", "title": "Research Rust patterns", "assignments": [{"agent_id": "a1", "agent_name": "Agent a1", "role_description": "Research agent", "matched_skills": ["web_search"]}], "reasoning": "User wants research"}"#;
        let router = make_planning_mock_llm(plan_json);
        let orch = make_orchestrator_with_llm_and_agents(
            router,
            vec![make_agent("a1", vec!["web_search"])],
        );

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
        assert!(text.contains("assigned"), "Expected 'assigned' in: {}", text);

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
        let orch = make_orchestrator_with_llm_and_agents(
            router,
            vec![make_agent("a1", vec!["web_search"])],
        );

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
}
