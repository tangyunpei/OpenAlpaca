use async_trait::async_trait;
use openalpaca_core::{
    gateway::{HandleRequest, HandleResult, MessageHandler, ResolvedAttachment, SkippedAttachment},
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
    ///
    /// U3(b): `attachments_used` is what it says — the ids the adaptation
    /// recorded as withheld are removed from it and carried on
    /// `attachments_skipped` instead. The map is drained on every turn, with
    /// or without attachments, so nothing is left behind for the next request
    /// that happens to reuse a id (it cannot — ids are v4 — but the drain is
    /// unconditional for the same reason the other two are).
    fn build_result(
        &self,
        request_id: Uuid,
        content: Result<String, String>,
        attachment_ids: Vec<String>,
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
        let attachments_skipped: Vec<SkippedAttachment> = self
            .orchestrator
            .attachments_skipped_map
            .remove(&request_id)
            .map(|(_, v)| v)
            .unwrap_or_default();

        let content = content?;

        let attachments_used = used_after_skipping(attachment_ids, &attachments_skipped);

        Ok(HandleResult {
            content,
            model: meta.as_ref().map(|m| m.model.clone()),
            tokens_in: meta.as_ref().map(|m| m.tokens_in),
            tokens_out: meta.as_ref().map(|m| m.tokens_out),
            attachments_used,
            attachments_skipped,
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

/// U3(b) — `attachments_used` is what it says.
///
/// An attachment the adaptation withheld (no vision, no document support and
/// no extracted text, unreadable bytes) never reached the model, so it is not
/// "used". It leaves this list and travels on `attachments_skipped` instead,
/// with the reason.
fn used_after_skipping(
    attachment_ids: Vec<String>,
    skipped: &[SkippedAttachment],
) -> Vec<String> {
    attachment_ids
        .into_iter()
        .filter(|id| !skipped.iter().any(|s| &s.id == id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skipped(id: &str) -> SkippedAttachment {
        SkippedAttachment {
            id: id.to_string(),
            reason: "the answering model does not support image input".to_string(),
        }
    }

    /// **U3(b).** The withheld id leaves `attachments_used`; the one that got
    /// through stays. Before this the `done` frame listed both "as if they had
    /// been used", which is how a turn that answered from a placeholder looked
    /// exactly like one that had read the file.
    #[test]
    fn a_withheld_attachment_is_not_reported_as_used() {
        let used = used_after_skipping(
            vec!["a".to_string(), "b".to_string()],
            &[skipped("b")],
        );
        assert_eq!(used, vec!["a".to_string()]);
    }

    #[test]
    fn nothing_skipped_leaves_the_list_alone() {
        let used = used_after_skipping(vec!["a".to_string(), "b".to_string()], &[]);
        assert_eq!(used, vec!["a".to_string(), "b".to_string()]);
    }
}
