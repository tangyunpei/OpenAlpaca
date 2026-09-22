use async_trait::async_trait;
use openalpaca_core::{
    gateway::{HandleRequest, HandleResult, MessageHandler, ResolvedAttachment},
    orchestrator::Orchestrator,
};
use std::sync::Arc;

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
    async fn handle(&self, request: HandleRequest) -> Result<HandleResult, String> {
        self.orchestrator.handle_message_result(request).await
    }

    async fn handle_with_attachments(
        &self,
        request: HandleRequest,
        attachments: Vec<ResolvedAttachment>,
    ) -> Result<HandleResult, String> {
        self.orchestrator
            .handle_message_with_attachments_result(request, attachments)
            .await
    }
}
