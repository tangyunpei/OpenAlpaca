//! Orchestrator module: the central message handler replacing CoreCtx.
//!
//! Routes user messages through intent classification, skill matching,
//! and task dispatch pipelines.

pub mod dispatcher;
pub mod intent;
pub mod skill_matcher;

use crate::bus::EventBus;
use crate::context::{SharedContext, TaskEntry, TaskEntryStatus};
use crate::events::SystemEvent;
use crate::lane::{LaneManager, TaskLaneStatus};
use crate::middleware::guard::OutputGuard;
use crate::middleware::prompt::{AgentPersona, PromptAssembler, SystemPersona};
use crate::runner::{LoopConfig, run_agentic_loop};
use crate::security::policy::{Principal, Scope, TrustGate};
use crate::types::Capability;
use chrono::Utc;
use openalpaca_llm::{ChatMessage, LlmProvider};
use std::sync::Arc;
use uuid::Uuid;

use dispatcher::TaskDispatcher;
use intent::{Intent, IntentParser};

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
    pub llm_provider: Option<Arc<dyn LlmProvider>>,
    pub loop_config: LoopConfig,
    intent_parser: IntentParser,
    task_dispatcher: TaskDispatcher,
}

impl Orchestrator {
    pub fn new(
        shared_context: Arc<SharedContext>,
        lane_manager: Arc<LaneManager>,
        bus: EventBus,
        system_persona: SystemPersona,
        llm_provider: Option<Arc<dyn LlmProvider>>,
        loop_config: LoopConfig,
    ) -> Self {
        let task_dispatcher =
            TaskDispatcher::new(shared_context.clone(), lane_manager.clone(), bus.clone());
        Self {
            shared_context,
            lane_manager,
            bus,
            system_persona,
            llm_provider,
            loop_config,
            intent_parser: IntentParser,
            task_dispatcher,
        }
    }

    /// Handle a user message through the full pipeline:
    /// 1. TrustGate permission check
    /// 2. Intent classification
    /// 3. Emit IntentClassified event
    /// 4. Route to appropriate handler
    pub async fn handle_message(
        &self,
        request_id: Uuid,
        source: String,
        content: String,
        principal: Principal,
        scope: Scope,
    ) -> Result<String, String> {
        // 1. Permission check
        let capability = Capability {
            name: "chat.respond".to_string(),
        };
        TrustGate::check(&principal, &capability, &scope)?;

        // 2. Classify intent
        let intent = self.intent_parser.parse(&content);

        // 3. Emit IntentClassified event
        self.bus.publish(SystemEvent::IntentClassified {
            request_id,
            intent_type: intent.intent_type().to_string(),
            timestamp: Utc::now(),
        });

        // 4. Route by intent
        match intent {
            Intent::SimpleQuery { query } => {
                self.handle_simple_query(request_id, &source, &query).await
            }
            Intent::TaskQuery { task_id } => self.handle_task_query(task_id),
            Intent::ComplexTask {
                description,
                required_skills,
            } => self.task_dispatcher.dispatch(
                request_id,
                &source,
                &description,
                &required_skills,
                &principal_id(&principal),
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
    ) -> Result<String, String> {
        let agent_persona = AgentPersona {
            role: "Assistant".to_string(),
            tone: "Friendly".to_string(),
            domain_knowledge: vec![],
        };
        let full_prompt =
            PromptAssembler::assemble(&self.system_persona, &agent_persona, query);

        let response_content = if let Some(ref provider) = self.llm_provider {
            // Real LLM call via agentic loop
            let messages = vec![
                ChatMessage::system(&full_prompt),
                ChatMessage::user(query),
            ];
            let result = run_agentic_loop(
                provider.as_ref(),
                messages,
                vec![], // No tools for simple queries
                &self.loop_config,
            )
            .await;
            result.final_content
        } else {
            // Fallback: echo stub (backward compatible)
            format!(
                "{{\"status\": \"ok\", \"echo\": \"Received: {}\"}}",
                query.chars().take(50).collect::<String>()
            )
        };

        // Output guard
        let validated = OutputGuard::ensure_json(&response_content)?;

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
    use crate::agent::subagent::{AgentConstraints, AgentPreset, AgentStatus, Skill, SubAgent};

    fn make_orchestrator() -> Orchestrator {
        let ctx = Arc::new(SharedContext::new());
        let lanes = Arc::new(LaneManager::new());
        let bus = EventBus::default();
        Orchestrator::new(
            ctx,
            lanes,
            bus,
            SystemPersona::default(),
            None,
            LoopConfig::default(),
        )
    }

    fn make_orchestrator_with_agents(agents: Vec<SubAgent>) -> Orchestrator {
        let ctx = Arc::new(SharedContext::new());
        for a in agents {
            ctx.agent_registry.register(a);
        }
        let lanes = Arc::new(LaneManager::new());
        let bus = EventBus::default();
        Orchestrator::new(
            ctx,
            lanes,
            bus,
            SystemPersona::default(),
            None,
            LoopConfig::default(),
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
            )
            .await;
        assert!(result.is_ok());
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["status"], "queued");
        assert!(json["task_id"].as_str().is_some());
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
        use openalpaca_llm::{ChatRequest, ChatResponse, FinishReason, LlmError, Usage};

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
        let orch = Orchestrator::new(
            ctx,
            lanes,
            bus,
            SystemPersona::default(),
            Some(Arc::new(MockLlm)),
            LoopConfig::default(),
        );

        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "What is Rust?".to_string(),
                Principal::System,
                Scope::Global,
            )
            .await;
        assert!(result.is_ok());
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["answer"], "Mock LLM response");
    }
}
