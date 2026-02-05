use crate::context::SharedContext;
use crate::lane::{LaneKey, LaneManager};
use std::sync::Arc;

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
}

impl Gateway {
    pub fn new(shared_context: Arc<SharedContext>, lane_manager: Arc<LaneManager>) -> Self {
        Self {
            shared_context,
            lane_manager,
        }
    }

    /// Handle an inbound message. Creates or retrieves a conversation lane
    /// and returns a stub response.
    pub fn handle_message(
        &self,
        user_id: &str,
        source: &str,
        content: &str,
    ) -> GatewayResponse {
        let key = LaneKey::new(user_id, source);
        let _lane = self.lane_manager.get_or_create_conversation(key.clone());

        GatewayResponse {
            lane_key: key,
            content: format!("Received: {content}"),
        }
    }

    /// Health check.
    pub fn is_healthy(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_gateway() -> Gateway {
        Gateway::new(
            Arc::new(SharedContext::new()),
            Arc::new(LaneManager::new()),
        )
    }

    #[test]
    fn test_gateway_creation() {
        let gw = make_gateway();
        assert!(gw.is_healthy());
    }

    #[test]
    fn test_handle_message() {
        let gw = make_gateway();
        let resp = gw.handle_message("user1", "cli", "hello");
        assert_eq!(resp.lane_key.user_id, "user1");
        assert_eq!(resp.lane_key.source, "cli");
        assert_eq!(resp.content, "Received: hello");
    }

    #[test]
    fn test_handle_message_creates_lane() {
        let gw = make_gateway();
        assert_eq!(gw.lane_manager.conversation_count(), 0);

        gw.handle_message("user1", "telegram", "hi");
        assert_eq!(gw.lane_manager.conversation_count(), 1);

        // Same user+source should not create a new lane
        gw.handle_message("user1", "telegram", "again");
        assert_eq!(gw.lane_manager.conversation_count(), 1);
    }

    #[test]
    fn test_health_check() {
        let gw = make_gateway();
        assert!(gw.is_healthy());
    }

    #[test]
    fn test_full_gateway_stack_integration() {
        // Exercises Gateway -> LaneManager -> SharedContext together
        let shared = Arc::new(SharedContext::new());
        let lanes = Arc::new(LaneManager::new());
        let gw = Gateway::new(shared.clone(), lanes.clone());

        // Register a task in shared context
        assert!(gw
            .shared_context
            .task_registry
            .register("task-1".into(), "integration test".into()));

        // Handle messages from multiple sources
        let r1 = gw.handle_message("alice", "telegram", "hello");
        let r2 = gw.handle_message("bob", "gui", "/status");
        let r3 = gw.handle_message("alice", "telegram", "follow-up");

        // Verify lanes
        assert_eq!(lanes.conversation_count(), 2); // alice+telegram, bob+gui
        assert_eq!(r1.lane_key.user_id, "alice");
        assert_eq!(r2.lane_key.source, "gui");
        assert_eq!(r3.content, "Received: follow-up");

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
