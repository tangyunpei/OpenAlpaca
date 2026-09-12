use async_trait::async_trait;
use openalpaca_core::{
    gateway::{HandleRequest, HandleResult, MessageHandler, ResolvedAttachment},
    orchestrator::Orchestrator,
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

    /// Drain LLM metadata and build HandleResult from orchestrator output.
    fn build_result(
        &self,
        request_id: Uuid,
        content: Result<String, String>,
        attachments_used: Vec<String>,
    ) -> Result<HandleResult, String> {
        let meta = self
            .orchestrator
            .llm_metadata_map
            .remove(&request_id)
            .map(|(_, v)| v);
        let delegation = self
            .orchestrator
            .delegation_map
            .remove(&request_id)
            .map(|(_, v)| v);

        let content = content?;

        Ok(HandleResult {
            content,
            model: meta.as_ref().map(|m| m.model.clone()),
            tokens_in: meta.as_ref().map(|m| m.tokens_in),
            tokens_out: meta.as_ref().map(|m| m.tokens_out),
            attachments_used,
            delegation,
        })
    }
}

#[async_trait]
impl MessageHandler for OrchestratorHandler {
    async fn handle(&self, request: HandleRequest) -> Result<HandleResult, String> {
        let request_id = request.request_id;
        let result = self.orchestrator.handle_message(request).await;

        self.build_result(request_id, result, Vec::new())
    }

    async fn handle_with_attachments(
        &self,
        request: HandleRequest,
        attachments: Vec<ResolvedAttachment>,
    ) -> Result<HandleResult, String> {
        let request_id = request.request_id;
        let attachment_ids: Vec<String> = attachments.iter().map(|a| a.file_id.clone()).collect();

        let result = self
            .orchestrator
            .handle_message_with_attachments(request, attachments)
            .await;

        // Only report attachments as used when the handler succeeded.
        // On error, no attachments were consumed into a response.
        let used = if result.is_ok() {
            attachment_ids
        } else {
            Vec::new()
        };

        self.build_result(request_id, result, used)
    }
}
