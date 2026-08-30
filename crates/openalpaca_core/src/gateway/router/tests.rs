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
        _stream_id: Option<String>,
    ) -> Result<HandleResult, String> {
        Ok(HandleResult::text(format!("Echo: {content}")))
    }
}

/// A handler that reports a delegated task.
struct DelegatingHandler;

#[async_trait]
impl MessageHandler for DelegatingHandler {
    async fn handle(
        &self,
        _request_id: Uuid,
        _source: String,
        _content: String,
        _principal: Principal,
        _scope: Scope,
        _lane_key: String,
        _workspace_path: Option<String>,
        _stream_id: Option<String>,
    ) -> Result<HandleResult, String> {
        let mut result = HandleResult::text("ack".to_string());
        result.delegation = Some(DelegationInfo {
            task_id: "task-42".to_string(),
            title: "Research Rust".to_string(),
        });
        Ok(result)
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
        _stream_id: Option<String>,
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
            attachments: Vec::new(),
            workspace_path: None,
            stream_id: None,
        })
        .await;
    assert_eq!(resp.lane_key.user_id, "user1");
    assert_eq!(resp.lane_key.source, "cli");
    assert_eq!(resp.content, "Echo: hello");
    assert!(!resp.is_error);
    assert!(resp.delegation.is_none());
}

#[tokio::test]
async fn test_handle_event_propagates_delegation() {
    let gw = Gateway::new(
        Arc::new(SharedContext::new()),
        Arc::new(LaneManager::new()),
        Arc::new(DelegatingHandler),
        EventBus::default(),
        None,
    );
    let resp = gw
        .handle_event(GatewayRequest {
            source: EventSource::Cli {
                session_id: "user1".to_string(),
            },
            content: "do a big task".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
            attachments: Vec::new(),
            workspace_path: None,
            stream_id: None,
        })
        .await;
    assert!(!resp.is_error);
    let delegation = resp.delegation.expect("delegation should propagate");
    assert_eq!(delegation.task_id, "task-42");
    assert_eq!(delegation.title, "Research Rust");
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
        attachments: Vec::new(),
        workspace_path: None,
        stream_id: None,
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
        attachments: Vec::new(),
        workspace_path: None,
        stream_id: None,
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
            attachments: Vec::new(),
            workspace_path: None,
            stream_id: None,
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
        attachments: Vec::new(),
        workspace_path: None,
        stream_id: None,
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
        attachments: Vec::new(),
        workspace_path: None,
        stream_id: None,
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
        attachments: Vec::new(),
        workspace_path: None,
        stream_id: None,
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
            attachments: Vec::new(),
            workspace_path: None,
            stream_id: None,
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
            attachments: Vec::new(),
            workspace_path: None,
            stream_id: None,
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
            attachments: Vec::new(),
            workspace_path: None,
            stream_id: None,
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
        attachments: Vec::new(),
        workspace_path: None,
        stream_id: None,
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
            attachments: Vec::new(),
            workspace_path: None,
            stream_id: None,
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
            attachments: Vec::new(),
            workspace_path: None,
            stream_id: None,
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
            attachments: Vec::new(),
            workspace_path: None,
            stream_id: None,
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
        capabilities: vec![],
        preset: AgentPreset::default(),
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    };
    assert!(shared.agent_registry.register(agent));
    assert!(shared.agent_registry.get("a1").is_some());

    // Health
    assert!(gw.is_healthy());
}
