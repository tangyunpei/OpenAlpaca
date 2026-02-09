use async_trait::async_trait;
use openalpaca_core::{
    gateway::MessageHandler,
    orchestrator::Orchestrator,
    security::policy::{Principal, Scope},
};
use std::sync::Arc;
use uuid::Uuid;

/// Bridges Gateway's MessageHandler trait to the Orchestrator.
pub struct OrchestratorHandler {
    orchestrator: Arc<Orchestrator>,
}

impl OrchestratorHandler {
    pub fn new(orchestrator: Arc<Orchestrator>) -> Self {
        Self { orchestrator }
    }
}

#[async_trait]
impl MessageHandler for OrchestratorHandler {
    async fn handle(
        &self,
        request_id: Uuid,
        source: String,
        content: String,
        principal: Principal,
        scope: Scope,
        lane_key: String,
    ) -> Result<String, String> {
        self.orchestrator
            .handle_message(request_id, source, content, principal, scope, lane_key)
            .await
    }
}
