use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::events::SystemEvent;
use crate::lane::{LaneKey, LaneManager};
use crate::security::policy::{Principal, Scope};
use chrono::Utc;
use openalpaca_api::events::EventSource;
use std::sync::Arc;
use uuid::Uuid;

/// Trait for processing messages through the pipeline.
/// The daemon implements this by delegating to CoreCtx.
pub trait MessageHandler: Send + Sync {
    fn handle(
        &self,
        request_id: Uuid,
        source: String,
        content: String,
        principal: Principal,
        scope: Scope,
    ) -> Result<String, String>;
}

/// Inbound request to the Gateway.
pub struct GatewayRequest {
    pub source: EventSource,
    pub content: String,
    pub principal: Principal,
    pub scope: Scope,
}

/// Response from the gateway after handling a message.
#[derive(Debug)]
pub struct GatewayResponse {
    pub lane_key: LaneKey,
    pub content: String,
}

/// The unified entry point for all inbound messages.
pub struct Gateway {
    pub shared_context: Arc<SharedContext>,
    pub lane_manager: Arc<LaneManager>,
    pub handler: Arc<dyn MessageHandler>,
    pub bus: EventBus,
}

impl Gateway {
    pub fn new(
        shared_context: Arc<SharedContext>,
        lane_manager: Arc<LaneManager>,
        handler: Arc<dyn MessageHandler>,
        bus: EventBus,
    ) -> Self {
        Self {
            shared_context,
            lane_manager,
            handler,
            bus,
        }
    }

    /// Handle an inbound event from any source.
    pub fn handle_event(&self, req: GatewayRequest) -> GatewayResponse {
        let (user_id, source_name) = derive_user_and_source(&req.source);
        let key = LaneKey::new(&user_id, &source_name);
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

        // Delegate to the handler (CoreCtx pipeline)
        match self.handler.handle(
            request_id,
            source_name,
            req.content,
            req.principal,
            req.scope,
        ) {
            Ok(content) => GatewayResponse {
                lane_key: key,
                content,
            },
            Err(e) => GatewayResponse {
                lane_key: key,
                content: format!("Error: {e}"),
            },
        }
    }

    /// Backward-compatible handle_message (delegates to handle_event with defaults).
    pub fn handle_message(
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
        })
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

    impl MessageHandler for StubHandler {
        fn handle(
            &self,
            _request_id: Uuid,
            _source: String,
            content: String,
            _principal: Principal,
            _scope: Scope,
        ) -> Result<String, String> {
            Ok(format!("Echo: {content}"))
        }
    }

    /// A handler that always fails.
    struct FailHandler;

    impl MessageHandler for FailHandler {
        fn handle(
            &self,
            _request_id: Uuid,
            _source: String,
            _content: String,
            _principal: Principal,
            _scope: Scope,
        ) -> Result<String, String> {
            Err("Access denied".to_string())
        }
    }

    fn make_gateway() -> Gateway {
        Gateway::new(
            Arc::new(SharedContext::new()),
            Arc::new(LaneManager::new()),
            Arc::new(StubHandler),
            EventBus::default(),
        )
    }

    fn make_failing_gateway() -> Gateway {
        Gateway::new(
            Arc::new(SharedContext::new()),
            Arc::new(LaneManager::new()),
            Arc::new(FailHandler),
            EventBus::default(),
        )
    }

    #[test]
    fn test_gateway_creation() {
        let gw = make_gateway();
        assert!(gw.is_healthy());
    }

    #[test]
    fn test_handle_event_echo() {
        let gw = make_gateway();
        let resp = gw.handle_event(GatewayRequest {
            source: EventSource::Cli {
                session_id: "user1".to_string(),
            },
            content: "hello".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
        });
        assert_eq!(resp.lane_key.user_id, "user1");
        assert_eq!(resp.lane_key.source, "cli");
        assert_eq!(resp.content, "Echo: hello");
    }

    #[test]
    fn test_handle_event_creates_lane() {
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
        });
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
        });
        assert_eq!(gw.lane_manager.conversation_count(), 1);
    }

    #[test]
    fn test_handle_event_error_propagation() {
        let gw = make_failing_gateway();
        let resp = gw.handle_event(GatewayRequest {
            source: EventSource::Api {
                request_id: "req1".to_string(),
            },
            content: "test".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
        });
        assert_eq!(resp.content, "Error: Access denied");
    }

    #[test]
    fn test_handle_event_emits_user_request() {
        let gw = make_gateway();
        let mut rx = gw.bus.subscribe();

        gw.handle_event(GatewayRequest {
            source: EventSource::Api {
                request_id: "req1".to_string(),
            },
            content: "hello bus".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
        });

        let event = rx.try_recv().unwrap();
        match event {
            SystemEvent::UserRequest { content, source, .. } => {
                assert_eq!(content, "hello bus");
                assert_eq!(source, "api");
            }
            _ => panic!("Expected UserRequest event"),
        }
    }

    #[test]
    fn test_handle_event_records_message_on_lane() {
        let gw = make_gateway();

        gw.handle_event(GatewayRequest {
            source: EventSource::Telegram {
                chat_id: "c1".to_string(),
                user_id: "u1".to_string(),
            },
            content: "msg1".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
        });
        gw.handle_event(GatewayRequest {
            source: EventSource::Telegram {
                chat_id: "c1".to_string(),
                user_id: "u1".to_string(),
            },
            content: "msg2".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
        });

        let key = LaneKey::new("u1", "telegram");
        let lane = gw.lane_manager.get_or_create_conversation(key);
        assert_eq!(lane.message_count(), 2);
    }

    #[test]
    fn test_backward_compat_handle_message() {
        let gw = make_gateway();
        let resp = gw.handle_message("user1", "cli", "hello");
        // handle_message now delegates through the handler
        assert!(resp.content.starts_with("Echo:"));
    }

    #[test]
    fn test_health_check() {
        let gw = make_gateway();
        assert!(gw.is_healthy());
    }

    #[test]
    fn test_full_gateway_stack_integration() {
        let shared = Arc::new(SharedContext::new());
        let lanes = Arc::new(LaneManager::new());
        let gw = Gateway::new(
            shared.clone(),
            lanes.clone(),
            Arc::new(StubHandler),
            EventBus::default(),
        );

        // Register a task in shared context
        assert!(gw
            .shared_context
            .task_registry
            .register("task-1".into(), "integration test".into()));

        // Handle messages from multiple sources
        let r1 = gw.handle_event(GatewayRequest {
            source: EventSource::Telegram {
                chat_id: "c1".to_string(),
                user_id: "alice".to_string(),
            },
            content: "hello".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
        });
        let r2 = gw.handle_event(GatewayRequest {
            source: EventSource::Gui {
                connection_id: "bob".to_string(),
            },
            content: "/status".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
        });
        let r3 = gw.handle_event(GatewayRequest {
            source: EventSource::Telegram {
                chat_id: "c1".to_string(),
                user_id: "alice".to_string(),
            },
            content: "follow-up".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
        });

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

        // Agent state
        shared.agent_states.set("a1".into(), "active".into());
        assert_eq!(shared.agent_states.get("a1").unwrap(), "active");

        // Health
        assert!(gw.is_healthy());
    }
}
