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
        let result = self
            .orchestrator
            .handle_message(request_id, source, content, principal, scope, lane_key)
            .await;

        // Always drain metadata (even on error) to prevent unbounded map growth.
        // Handlers may insert metadata before returning Err (e.g. LLM error with empty content).
        let meta = self.orchestrator.llm_metadata_map.remove(&request_id).map(|(_, v)| v);

        let content = result?;

        Ok(HandleResult {
            content,
            model: meta.as_ref().map(|m| m.model.clone()),
            tokens_in: meta.as_ref().map(|m| m.tokens_in),
            tokens_out: meta.as_ref().map(|m| m.tokens_out),
        })
    }
}
