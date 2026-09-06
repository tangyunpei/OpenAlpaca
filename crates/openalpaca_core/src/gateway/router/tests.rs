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
            source: EventSource::Gui {
                connection_id: "user1".to_string(),
            },
            content: "hello".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
            attachments: Vec::new(),
            workspace_path: None,
            stream_id: None,
            lane_override: None,
        })
        .await;
    assert_eq!(resp.lane_key.user_id, "user1");
    assert_eq!(resp.lane_key.source, "gui");
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
            source: EventSource::Gui {
                connection_id: "user1".to_string(),
            },
            content: "do a big task".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
            attachments: Vec::new(),
            workspace_path: None,
            stream_id: None,
            lane_override: None,
        })
        .await;
    assert!(!resp.is_error);
    let delegation = resp.delegation.expect("delegation should propagate");
    assert_eq!(delegation.task_id, "task-42");
    assert_eq!(delegation.title, "Research Rust");
}

/// GAP-23: the delegating turn is stored *carrying* its run, so a reload can
/// still tell which assistant message started which workflow.
#[tokio::test]
async fn test_delegating_turn_persists_its_task_id() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let gw = Gateway::new(
        Arc::new(SharedContext::new()),
        Arc::new(LaneManager::new()),
        Arc::new(DelegatingHandler),
        EventBus::default(),
        Some(db.clone()),
    );

    gw.handle_event(GatewayRequest {
        source: EventSource::Gui {
            connection_id: "user1".to_string(),
        },
        content: "do a big task".to_string(),
        principal: Principal::System,
        scope: Scope::Global,
        attachments: Vec::new(),
        workspace_path: None,
        stream_id: None,
        lane_override: None,
    })
    .await;

    let messages = openalpaca_storage::ConversationRepository::new(&db)
        .list_by_lane("user1:gui", 50, 0)
        .unwrap();
    assert_eq!(messages.len(), 2);
    // The user's own turn started nothing.
    assert_eq!(messages[0].role, "user");
    assert!(messages[0].task_id.is_none());
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].task_id.as_deref(), Some("task-42"));
}

/// A plain chat turn stores no run link — the column stays `NULL` rather than
/// picking up whatever the lane happens to be running.
#[tokio::test]
async fn test_plain_turn_persists_no_task_id() {
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
        source: EventSource::Gui {
            connection_id: "user1".to_string(),
        },
        content: "hello".to_string(),
        principal: Principal::System,
        scope: Scope::Global,
        attachments: Vec::new(),
        workspace_path: None,
        stream_id: None,
        lane_override: None,
    })
    .await;

    let messages = openalpaca_storage::ConversationRepository::new(&db)
        .list_by_lane("user1:gui", 50, 0)
        .unwrap();
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|m| m.task_id.is_none()));
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
        lane_override: None,
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
        lane_override: None,
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
            lane_override: None,
        })
        .await;
    assert!(resp.is_error);
    assert_eq!(resp.content, "Access denied");
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
        lane_override: None,
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
        lane_override: None,
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
            lane_override: None,
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
            lane_override: None,
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
            lane_override: None,
        })
        .await;
    assert_eq!(resp3.lane_key.user_id, "tg_user_789");
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
        lane_override: None,
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
        .get_active_session_for_lane("alice:telegram")
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
            lane_override: None,
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
            lane_override: None,
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
            lane_override: None,
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

#[tokio::test]
async fn test_handle_event_lane_override_pins_originating_lane() {
    // Routing V2 lane continuity: an Internal-source re-entry (follow-up
    // runner) with lane_override lands on the ORIGINATING lane, not the
    // "user:internal" lane the source would derive.
    let gw = make_gateway();
    let resp = gw
        .handle_event(GatewayRequest {
            source: EventSource::Internal,
            content: "follow-up work".to_string(),
            principal: Principal::User {
                global_id: "junpei".to_string(),
            },
            scope: Scope::Global,
            attachments: Vec::new(),
            workspace_path: None,
            stream_id: None,
            lane_override: Some("junpei:cli".to_string()),
        })
        .await;
    assert_eq!(resp.lane_key.user_id, "junpei");
    assert_eq!(resp.lane_key.source, "cli");
    assert!(!resp.is_error);
}

#[tokio::test]
async fn test_handle_event_malformed_lane_override_falls_back() {
    let gw = make_gateway();
    let resp = gw
        .handle_event(GatewayRequest {
            source: EventSource::Internal,
            content: "hello".to_string(),
            principal: Principal::User {
                global_id: "junpei".to_string(),
            },
            scope: Scope::Global,
            attachments: Vec::new(),
            workspace_path: None,
            stream_id: None,
            lane_override: Some("no_colon".to_string()),
        })
        .await;
    // Malformed override → derived lane (principal + internal source).
    assert_eq!(resp.lane_key.user_id, "junpei");
    assert_eq!(resp.lane_key.source, "internal");
}

/// A handler that opens a **new** session on the lane while the turn is still
/// running — what `POST /v1/sessions` (the GUI's "New chat") does to a lane
/// whose agentic loop is mid-flight.
struct NewChatMidTurnHandler {
    db: openalpaca_storage::Database,
}

#[async_trait]
impl MessageHandler for NewChatMidTurnHandler {
    async fn handle(
        &self,
        _request_id: Uuid,
        _source: String,
        _content: String,
        _principal: Principal,
        _scope: Scope,
        lane_key: String,
        _workspace_path: Option<String>,
        _stream_id: Option<String>,
    ) -> Result<HandleResult, String> {
        openalpaca_storage::ConversationRepository::new(&self.db)
            .create_session(&lane_key, "gui", None, Some("New chat"))
            .map_err(|e| e.to_string())?;
        Ok(HandleResult::text("answer".to_string()))
    }
}

/// §5.1: the gateway resolves lane → session **once per turn**. A "New chat"
/// that lands between the question and the answer must not split the turn
/// across two conversations — the answer belongs where the question was asked,
/// not wherever the lane is pointing by the time the loop returns.
#[tokio::test]
async fn test_a_new_chat_mid_turn_keeps_the_turn_in_one_session() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let gw = Gateway::new(
        Arc::new(SharedContext::new()),
        Arc::new(LaneManager::new()),
        Arc::new(NewChatMidTurnHandler { db: db.clone() }),
        EventBus::default(),
        Some(db.clone()),
    );

    gw.handle_event(GatewayRequest {
        source: EventSource::Gui {
            connection_id: "user1".to_string(),
        },
        content: "question".to_string(),
        principal: Principal::System,
        scope: Scope::Global,
        attachments: Vec::new(),
        workspace_path: None,
        stream_id: None,
        lane_override: None,
    })
    .await;

    let repo = openalpaca_storage::ConversationRepository::new(&db);
    let messages = repo.list_by_lane("user1:gui", 50, 0).unwrap();
    assert_eq!(messages.len(), 2);
    let asked_in = messages[0]
        .session_id
        .clone()
        .expect("the user message names the session it was asked in");
    assert_eq!(
        messages[1].session_id.as_deref(),
        Some(asked_in.as_str()),
        "the answer must land in the session the question was asked in"
    );

    // And that session is the one the turn started in — the "New chat" the
    // handler opened archived it, and the turn stayed behind with it.
    let session = repo.get_session(&asked_in).unwrap().expect("session");
    assert_eq!(session.status, "archived");
    assert_eq!(
        session.message_count, 2,
        "both halves of the turn are counted where they landed"
    );
}

/// A project root the workspace resolver will recognise.
fn project_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    dir
}

fn project_path(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().to_string()
}

/// §5.1: a session belongs to exactly one project, and changing project is a
/// **new** session, never a re-pointed one. The repository half of that rule
/// (never re-bind) was already there; this is the half that opens the new
/// conversation, so a turn's session and the runs it starts cannot disagree
/// about which project they belong to.
#[tokio::test]
async fn test_changing_project_opens_a_new_session() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let bus = EventBus::default();
    let mut rx = bus.subscribe();
    let gw = Gateway::new(
        Arc::new(SharedContext::new()),
        Arc::new(LaneManager::new()),
        Arc::new(StubHandler),
        bus,
        Some(db.clone()),
    );

    let one = project_dir();
    let two = project_dir();
    let turn = |workspace_path: Option<String>| GatewayRequest {
        source: EventSource::Gui {
            connection_id: "user1".to_string(),
        },
        content: "hello".to_string(),
        principal: Principal::System,
        scope: Scope::Global,
        attachments: Vec::new(),
        workspace_path,
        stream_id: None,
        lane_override: None,
    };

    gw.handle_event(turn(Some(project_path(&one)))).await;
    gw.handle_event(turn(Some(project_path(&two)))).await;
    // A turn that carries no project changes nothing: an absent header is not
    // a project change (connector lanes never switch).
    gw.handle_event(turn(None)).await;

    let repo = openalpaca_storage::ConversationRepository::new(&db);
    let messages = repo.list_by_lane("user1:gui", 50, 0).unwrap();
    assert_eq!(messages.len(), 6);
    let session_of = |i: usize| messages[i].session_id.clone().expect("session id");
    let first = session_of(0);
    let second = session_of(2);
    assert_eq!(session_of(1), first, "the first turn is one conversation");
    assert_eq!(session_of(3), second, "and so is the second");
    assert_ne!(first, second, "the project change opened a new session");
    assert_eq!(session_of(4), second, "a turn with no project stays put");
    assert_eq!(session_of(5), second);

    // Each session is bound to the project its turns came from, resolved by
    // the one resolver a dispatched run records as `task.workspace_id` (R22) —
    // so the session and its runs agree about the project.
    let root = |path: &str| {
        crate::memory::scope_context::MemoryScopeContext::for_request(Some(path))
            .request_workspace_root
    };
    let first = repo.get_session(&first).unwrap().expect("first session");
    assert_eq!(first.workspace_id, root(&project_path(&one)));
    assert_eq!(first.status, "archived", "the incumbent stepped down");
    let second = repo.get_session(&second).unwrap().expect("second session");
    assert_eq!(second.workspace_id, root(&project_path(&two)));
    assert_eq!(second.status, "active");

    // The switch is announced, so a second window's sidebar does not keep
    // showing the conversation that is no longer the live one.
    let announced = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|e| match e {
            crate::events::SystemEvent::SessionChanged {
                session_id, status, ..
            } => Some((session_id, status)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(announced, vec![(second.id.clone(), "active".to_string())]);
}

// ── Session event log (§5.5) ────────────────────────────────────────

/// The gateway's three emit points, on one delegating turn: the boot-boundary
/// `session_start`, the `user_msg`, and the `assistant_msg` with the
/// `delegation` beside it. Content stays in `conversation_messages` — the
/// records carry the message id and a preview only (§5.3).
#[tokio::test]
async fn a_turn_writes_its_two_halves_and_its_delegation_to_the_session_log() {
    use crate::session_log::{SessionLogLimits, SessionLogService, read_records};

    let dir = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let ctx = Arc::new(SharedContext::new());
    let service = Arc::new(SessionLogService::new(
        logs.path().to_path_buf(),
        Some(db.clone()),
        SessionLogLimits::default(),
        "test".to_string(),
    ));
    ctx.set_session_log(service.clone());

    let gw = Gateway::new(
        ctx,
        Arc::new(LaneManager::new()),
        Arc::new(DelegatingHandler),
        EventBus::default(),
        Some(db.clone()),
    );
    gw.handle_event(GatewayRequest {
        source: EventSource::Gui {
            connection_id: "user1".to_string(),
        },
        content: "do a big task".to_string(),
        principal: Principal::System,
        scope: Scope::Global,
        attachments: Vec::new(),
        workspace_path: None,
        stream_id: None,
        lane_override: None,
    })
    .await;

    let session_id = openalpaca_storage::ConversationRepository::new(&db)
        .active_session_id("user1:gui")
        .unwrap()
        .expect("the turn opened a session");
    let handle = service.handle_for(&session_id);
    assert!(handle.flush().await);

    let records = read_records(&logs.path().join(&session_id)).unwrap();
    let kinds: Vec<&str> = records.iter().map(|r| r.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["session_start", "user_msg", "assistant_msg", "delegation"],
        "{kinds:?}"
    );

    assert_eq!(records[0].data["lane_key"], "user1:gui");
    assert_eq!(records[0].data["source"], "gui");
    assert_eq!(records[0].data["boot_id"], service.boot_id());

    let messages = openalpaca_storage::ConversationRepository::new(&db)
        .list_by_lane("user1:gui", 50, 0)
        .unwrap();
    assert_eq!(records[1].data["msg_id"], messages[0].id);
    assert_eq!(records[1].data["preview"], "do a big task");
    assert!(records[1].task_id.is_none(), "a user turn starts no run");

    assert_eq!(records[2].data["msg_id"], messages[1].id);
    assert_eq!(records[2].data["preview"], "ack");

    assert_eq!(records[3].data["task_id"], "task-42");
    assert_eq!(records[3].data["title"], "Research Rust");
    assert_eq!(records[3].task_id.as_deref(), Some("task-42"));
}

/// A gateway with no session log service behaves exactly as before — the log
/// is an addition beside the transcript, never a precondition for it.
#[tokio::test]
async fn a_gateway_without_a_session_log_still_persists_its_turn() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let gw = Gateway::new(
        Arc::new(SharedContext::new()),
        Arc::new(LaneManager::new()),
        Arc::new(StubHandler),
        EventBus::default(),
        Some(db.clone()),
    );
    let resp = gw
        .handle_event(GatewayRequest {
            source: EventSource::Gui {
                connection_id: "user1".to_string(),
            },
            content: "hello".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
            attachments: Vec::new(),
            workspace_path: None,
            stream_id: None,
            lane_override: None,
        })
        .await;
    assert!(!resp.is_error);
    let messages = openalpaca_storage::ConversationRepository::new(&db)
        .list_by_lane("user1:gui", 50, 0)
        .unwrap();
    assert_eq!(messages.len(), 2);
}
