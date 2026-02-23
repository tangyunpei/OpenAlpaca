use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::events::SystemEvent;
use crate::gateway::persistence::GatewayPersistence;
use crate::lane::{LaneKey, LaneManager};
use crate::security::policy::{Principal, Scope};
use async_trait::async_trait;
use chrono::Utc;
use openalpaca_api::events::EventSource;
use openalpaca_storage::Database;
use std::sync::Arc;
use uuid::Uuid;

/// Rich result from message handling, carrying optional LLM metadata.
///
/// Non-LLM paths (task queries, commands, etc.) use `HandleResult::text()` which
/// sets all metadata fields to `None`.
#[derive(Debug)]
pub struct HandleResult {
    pub content: String,
    pub model: Option<String>,
    pub tokens_in: Option<u32>,
    pub tokens_out: Option<u32>,
}

impl HandleResult {
    /// Create a HandleResult for non-LLM responses (no metadata).
    pub fn text(content: String) -> Self {
        Self {
            content,
            model: None,
            tokens_in: None,
            tokens_out: None,
        }
    }
}

/// Trait for processing messages through the pipeline.
/// The daemon implements this by delegating to the Orchestrator.
#[async_trait]
pub trait MessageHandler: Send + Sync {
    async fn handle(
        &self,
        request_id: Uuid,
        source: String,
        content: String,
        principal: Principal,
        scope: Scope,
        lane_key: String,
        workspace_path: Option<String>,
    ) -> Result<HandleResult, String>;
}

/// Inbound request to the Gateway.
pub struct GatewayRequest {
    pub source: EventSource,
    pub content: String,
    pub principal: Principal,
    pub scope: Scope,
    /// Optional workspace path provided by the client (GUI project dir, CLI cwd).
    /// Used for memory scoping instead of the daemon's CWD.
    pub workspace_path: Option<String>,
}

/// Response from the gateway after handling a message.
#[derive(Debug)]
pub struct GatewayResponse {
    pub lane_key: LaneKey,
    pub content: String,
    /// Structured error flag — `true` when the handler returned `Err`.
    /// Replaces ad-hoc string prefix matching ("[error]", "Error:", etc.).
    pub is_error: bool,
    /// LLM model used (if any).
    pub model: Option<String>,
    /// Input tokens consumed (if LLM was called).
    pub tokens_in: Option<u32>,
    /// Output tokens generated (if LLM was called).
    pub tokens_out: Option<u32>,
}

/// The unified entry point for all inbound messages.
pub struct Gateway {
    pub shared_context: Arc<SharedContext>,
    pub lane_manager: Arc<LaneManager>,
    pub handler: Arc<dyn MessageHandler>,
    pub bus: EventBus,
    persistence: Option<GatewayPersistence>,
}

impl Gateway {
    pub fn new(
        shared_context: Arc<SharedContext>,
        lane_manager: Arc<LaneManager>,
        handler: Arc<dyn MessageHandler>,
        bus: EventBus,
        db: Option<Database>,
    ) -> Self {
        let persistence = db.map(GatewayPersistence::new);
        Self {
            shared_context,
            lane_manager,
            handler,
            bus,
            persistence,
        }
    }

    /// Handle an inbound event from any source.
    pub async fn handle_event(&self, req: GatewayRequest) -> GatewayResponse {
        let (mut user_id, source_name) = derive_user_and_source(&req.source);

        // Principal-aware: linked users use their global_id as lane user
        if let Principal::User { ref global_id } = req.principal {
            user_id = global_id.clone();
        }

        let key = LaneKey::new(&user_id, &source_name);
        let lane_key_str = key.to_string();
        let lane = self.lane_manager.get_or_create_conversation(key.clone());

        let request_id = Uuid::new_v4();

        // Emit UserRequest event
        self.bus.publish(SystemEvent::UserRequest {
            request_id,
            source: source_name.clone(),
            content: req.content.clone(),
            timestamp: Utc::now(),
        });

        // Record message on the lane
        lane.record_message();

        // Persist user message
        if let Some(ref p) = self.persistence
            && let Err(e) = p.persist_user_message(&lane_key_str, &req.content, &source_name)
        {
            tracing::warn!("Failed to persist user message: {e}");
        }

        let start = std::time::Instant::now();

        // Delegate to the handler
        match self
            .handler
            .handle(
                request_id,
                source_name.clone(),
                req.content,
                req.principal,
                req.scope,
                lane_key_str.clone(),
                req.workspace_path,
            )
            .await
        {
            Ok(result) => {
                let duration_ms = start.elapsed().as_millis() as i64;
                // Persist assistant message
                if let Some(ref p) = self.persistence
                    && let Err(e) = p.persist_assistant_message(
                        &lane_key_str,
                        &result.content,
                        Some(duration_ms),
                        &source_name,
                    )
                {
                    tracing::warn!("Failed to persist assistant message: {e}");
                }
                GatewayResponse {
                    lane_key: key,
                    content: result.content,
                    is_error: false,
                    model: result.model,
                    tokens_in: result.tokens_in,
                    tokens_out: result.tokens_out,
                }
            }
            Err(e) => GatewayResponse {
                lane_key: key,
                content: e,
                is_error: true,
                model: None,
                tokens_in: None,
                tokens_out: None,
            },
        }
    }

    /// Backward-compatible handle_message (delegates to handle_event with defaults).
    pub async fn handle_message(
        &self,
        _user_id: &str,
        _source: &str,
        content: &str,
    ) -> GatewayResponse {
        self.handle_event(GatewayRequest {
            source: EventSource::Api {
                request_id: Uuid::new_v4().to_string(),
            },
            content: content.to_string(),
            principal: Principal::System,
            scope: Scope::Global,
            workspace_path: None,
        })
        .await
    }

    /// Health check.
    pub fn is_healthy(&self) -> bool {
        true
    }
}

/// Derive user_id and source_name from EventSource.
fn derive_user_and_source(source: &EventSource) -> (String, String) {
    match source {
        EventSource::Telegram { user_id, .. } => (user_id.clone(), "telegram".to_string()),
        EventSource::Gui { connection_id } => (connection_id.clone(), "gui".to_string()),
        EventSource::Cli { session_id } => (session_id.clone(), "cli".to_string()),
        EventSource::Api { request_id } => (request_id.clone(), "api".to_string()),
        EventSource::Internal => ("system".to_string(), "internal".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stub handler that echoes the content.
    struct StubHandler;

    #[async_trait]
    impl MessageHandler for StubHandler {
        async fn handle(
            &self,
            _request_id: Uuid,
            _source: String,
            content: String,
            _principal: Principal,
            _scope: Scope,
            _lane_key: String,
            _workspace_path: Option<String>,
        ) -> Result<HandleResult, String> {
            Ok(HandleResult::text(format!("Echo: {content}")))
        }
    }

    /// A handler that always fails.
    struct FailHandler;

    #[async_trait]
    impl MessageHandler for FailHandler {
        async fn handle(
            &self,
            _request_id: Uuid,
            _source: String,
            _content: String,
            _principal: Principal,
            _scope: Scope,
            _lane_key: String,
            _workspace_path: Option<String>,
        ) -> Result<HandleResult, String> {
            Err("Access denied".to_string())
        }
    }

    fn make_gateway() -> Gateway {
        Gateway::new(
            Arc::new(SharedContext::new()),
            Arc::new(LaneManager::new()),
            Arc::new(StubHandler),
            EventBus::default(),
            None,
        )
    }

    fn make_failing_gateway() -> Gateway {
        Gateway::new(
            Arc::new(SharedContext::new()),
            Arc::new(LaneManager::new()),
            Arc::new(FailHandler),
            EventBus::default(),
            None,
        )
    }

    #[tokio::test]
    async fn test_gateway_creation() {
        let gw = make_gateway();
        assert!(gw.is_healthy());
    }

    #[tokio::test]
    async fn test_handle_event_echo() {
        let gw = make_gateway();
        let resp = gw
            .handle_event(GatewayRequest {
                source: EventSource::Cli {
                    session_id: "user1".to_string(),
                },
                content: "hello".to_string(),
                principal: Principal::System,
                scope: Scope::Global,
                workspace_path: None,
            })
            .await;
        assert_eq!(resp.lane_key.user_id, "user1");
        assert_eq!(resp.lane_key.source, "cli");
        assert_eq!(resp.content, "Echo: hello");
        assert!(!resp.is_error);
    }

    #[tokio::test]
    async fn test_handle_event_creates_lane() {
        let gw = make_gateway();
        assert_eq!(gw.lane_manager.conversation_count(), 0);

        gw.handle_event(GatewayRequest {
            source: EventSource::Telegram {
                chat_id: "chat1".to_string(),
                user_id: "user1".to_string(),
            },
            content: "hi".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
            workspace_path: None,
        })
        .await;
        assert_eq!(gw.lane_manager.conversation_count(), 1);

        // Same user+source should not create a new lane
        gw.handle_event(GatewayRequest {
            source: EventSource::Telegram {
                chat_id: "chat1".to_string(),
                user_id: "user1".to_string(),
            },
            content: "again".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
            workspace_path: None,
        })
        .await;
        assert_eq!(gw.lane_manager.conversation_count(), 1);
    }

    #[tokio::test]
    async fn test_handle_event_error_propagation() {
        let gw = make_failing_gateway();
        let resp = gw
            .handle_event(GatewayRequest {
                source: EventSource::Api {
                    request_id: "req1".to_string(),
                },
                content: "test".to_string(),
                principal: Principal::System,
                scope: Scope::Global,
                workspace_path: None,
            })
            .await;
        assert!(resp.is_error);
        assert_eq!(resp.content, "Access denied");
    }

    #[tokio::test]
    async fn test_handle_event_emits_user_request() {
        let gw = make_gateway();
        let mut rx = gw.bus.subscribe();

        gw.handle_event(GatewayRequest {
            source: EventSource::Api {
                request_id: "req1".to_string(),
            },
            content: "hello bus".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
            workspace_path: None,
        })
        .await;

        let event = rx.try_recv().unwrap();
        match event {
            SystemEvent::UserRequest {
                content, source, ..
            } => {
                assert_eq!(content, "hello bus");
                assert_eq!(source, "api");
            }
            _ => panic!("Expected UserRequest event"),
        }
    }

    #[tokio::test]
    async fn test_handle_event_records_message_on_lane() {
        let gw = make_gateway();

        gw.handle_event(GatewayRequest {
            source: EventSource::Telegram {
                chat_id: "c1".to_string(),
                user_id: "u1".to_string(),
            },
            content: "msg1".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
            workspace_path: None,
        })
        .await;
        gw.handle_event(GatewayRequest {
            source: EventSource::Telegram {
                chat_id: "c1".to_string(),
                user_id: "u1".to_string(),
            },
            content: "msg2".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
            workspace_path: None,
        })
        .await;

        let key = LaneKey::new("u1", "telegram");
        let lane = gw.lane_manager.get_or_create_conversation(key);
        assert_eq!(lane.message_count(), 2);
    }

    #[tokio::test]
    async fn test_principal_aware_lane_derivation() {
        let gw = make_gateway();

        // Linked user (Principal::User) should get lane keyed by global_id
        let resp = gw
            .handle_event(GatewayRequest {
                source: EventSource::Telegram {
                    chat_id: "c1".to_string(),
                    user_id: "tg_user_123".to_string(),
                },
                content: "hello".to_string(),
                principal: Principal::User {
                    global_id: "global1".to_string(),
                },
                scope: Scope::Global,
                workspace_path: None,
            })
            .await;
        assert_eq!(resp.lane_key.user_id, "global1");
        assert_eq!(resp.lane_key.source, "telegram");
        assert_eq!(resp.lane_key.to_string(), "global1:telegram");

        // Unlinked user (Principal::External) should keep provider user_id
        let resp2 = gw
            .handle_event(GatewayRequest {
                source: EventSource::Telegram {
                    chat_id: "c2".to_string(),
                    user_id: "tg_user_456".to_string(),
                },
                content: "hi".to_string(),
                principal: Principal::External {
                    provider: "telegram".to_string(),
                    id: "tg_user_456".to_string(),
                },
                scope: Scope::Global,
                workspace_path: None,
            })
            .await;
        assert_eq!(resp2.lane_key.user_id, "tg_user_456");
        assert_eq!(resp2.lane_key.source, "telegram");

        // System principal should also keep the source-derived user_id
        let resp3 = gw
            .handle_event(GatewayRequest {
                source: EventSource::Telegram {
                    chat_id: "c3".to_string(),
                    user_id: "tg_user_789".to_string(),
                },
                content: "yo".to_string(),
                principal: Principal::System,
                scope: Scope::Global,
                workspace_path: None,
            })
            .await;
        assert_eq!(resp3.lane_key.user_id, "tg_user_789");
    }

    #[tokio::test]
    async fn test_backward_compat_handle_message() {
        let gw = make_gateway();
        let resp = gw.handle_message("user1", "cli", "hello").await;
        // handle_message now delegates through the handler
        assert!(resp.content.starts_with("Echo:"));
        assert!(!resp.is_error);
    }

    #[tokio::test]
    async fn test_health_check() {
        let gw = make_gateway();
        assert!(gw.is_healthy());
    }

    #[tokio::test]
    async fn test_gateway_persists_messages() {
        let dir = tempfile::tempdir().unwrap();
        let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();

        let gw = Gateway::new(
            Arc::new(SharedContext::new()),
            Arc::new(LaneManager::new()),
            Arc::new(StubHandler),
            EventBus::default(),
            Some(db.clone()),
        );

        gw.handle_event(GatewayRequest {
            source: EventSource::Telegram {
                chat_id: "c1".to_string(),
                user_id: "alice".to_string(),
            },
            content: "hello from telegram".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
            workspace_path: None,
        })
        .await;

        // Verify messages persisted
        let repo = openalpaca_storage::ConversationRepository::new(&db);
        let messages = repo.list_by_lane("alice:telegram", 50, 0).unwrap();
        assert_eq!(messages.len(), 2); // user + assistant
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "hello from telegram");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].content, "Echo: hello from telegram");

        // Verify conversation master record
        let conv = repo
            .get_conversation_by_lane("alice:telegram")
            .unwrap()
            .unwrap();
        assert_eq!(conv.source, "telegram");
        assert_eq!(conv.message_count, 2);
    }

    #[tokio::test]
    async fn test_full_gateway_stack_integration() {
        let shared = Arc::new(SharedContext::new());
        let lanes = Arc::new(LaneManager::new());
        let gw = Gateway::new(
            shared.clone(),
            lanes.clone(),
            Arc::new(StubHandler),
            EventBus::default(),
            None,
        );

        // Register a task in shared context
        assert!(
            gw.shared_context
                .task_registry
                .register("task-1".into(), "integration test".into())
        );

        // Handle messages from multiple sources
        let r1 = gw
            .handle_event(GatewayRequest {
                source: EventSource::Telegram {
                    chat_id: "c1".to_string(),
                    user_id: "alice".to_string(),
                },
                content: "hello".to_string(),
                principal: Principal::System,
                scope: Scope::Global,
                workspace_path: None,
            })
            .await;
        let r2 = gw
            .handle_event(GatewayRequest {
                source: EventSource::Gui {
                    connection_id: "bob".to_string(),
                },
                content: "/status".to_string(),
                principal: Principal::System,
                scope: Scope::Global,
                workspace_path: None,
            })
            .await;
        let r3 = gw
            .handle_event(GatewayRequest {
                source: EventSource::Telegram {
                    chat_id: "c1".to_string(),
                    user_id: "alice".to_string(),
                },
                content: "follow-up".to_string(),
                principal: Principal::System,
                scope: Scope::Global,
                workspace_path: None,
            })
            .await;

        // Verify lanes
        assert_eq!(lanes.conversation_count(), 2); // alice+telegram, bob+gui
        assert_eq!(r1.lane_key.user_id, "alice");
        assert_eq!(r2.lane_key.source, "gui");
        assert_eq!(r3.content, "Echo: follow-up");

        // Create a task lane
        let _task_lane = lanes.create_task_lane("bg-task-1");
        assert_eq!(lanes.task_count(), 1);

        // Shared context task count
        assert_eq!(shared.task_registry.count(), 1);

        // Agent registry
        use crate::agent::subagent::{
            AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, SubAgent,
        };
        let agent = SubAgent {
            id: "a1".to_string(),
            template_id: "a1".to_string(),
            name: "Test Agent".to_string(),
            description: None,
            icon: None,
            status: AgentStatus::Idle,
            current_task: None,
            skills: vec![],
            preset: AgentPreset::default(),
            constraints: AgentConstraints::default(),
            llm_config: AgentLlmConfig::default(),
        };
        assert!(shared.agent_registry.register(agent));
        assert!(shared.agent_registry.get("a1").is_some());

        // Health
        assert!(gw.is_healthy());
    }
}
