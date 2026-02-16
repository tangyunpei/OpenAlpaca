use async_trait::async_trait;
use openalpaca_core::{
    gateway::{HandleResult, MessageHandler},
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
    ) -> Result<HandleResult, String> {
        // Clear any stale metadata from a previous call
        *self.orchestrator.last_llm_metadata.lock().unwrap() = None;

        let content = self
            .orchestrator
            .handle_message(request_id, source, content, principal, scope, lane_key)
            .await?;

        // Read metadata stored by query_handler / skill_handler
        let meta = self.orchestrator.last_llm_metadata.lock().unwrap().take();

        Ok(HandleResult {
            content,
            model: meta.as_ref().map(|m| m.model.clone()),
            tokens_in: meta.as_ref().map(|m| m.tokens_in),
            tokens_out: meta.as_ref().map(|m| m.tokens_out),
        })
    }
}
