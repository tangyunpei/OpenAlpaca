use async_trait::async_trait;
use openalpaca_core::{
    gateway::{
        HandleRequest, HandleResult, MemoryUsed, MessageHandler, ResolvedAttachment,
        SkippedAttachment,
    },
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
    /// unconditional for the same reason the other two are). `memory_used`
    /// (plan 3.3) is drained the same way, from the memory service, and so
    /// is the turn's continuity (design 11 §5).
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
        let memory_used: Vec<MemoryUsed> = self.orchestrator.take_memory_used(request_id);
        let continuity = self.orchestrator.take_continuity(request_id);

        let content = content?;

        let attachments_used = used_after_skipping(attachment_ids, &attachments_skipped);

        Ok(HandleResult {
            content,
            model: meta.as_ref().map(|m| m.model.clone()),
            tokens_in: meta.as_ref().map(|m| m.tokens_in),
            tokens_out: meta.as_ref().map(|m| m.tokens_out),
            attachments_used,
            attachments_skipped,
            memory_used,
            delegation,
            continuity,
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

    /// A provider that answers anything in one line.
    struct StubLlm;

    #[async_trait]
    impl openalpaca_llm::LlmProvider for StubLlm {
        fn name(&self) -> &str {
            "stub"
        }
        fn supports_tools(&self) -> bool {
            false
        }
        async fn chat(
            &self,
            _request: openalpaca_llm::ChatRequest,
        ) -> Result<openalpaca_llm::ChatResponse, openalpaca_llm::LlmError> {
            Ok(openalpaca_llm::ChatResponse {
                content: "we were comparing hotels".to_string(),
                tool_calls: vec![],
                model: "stub-model".to_string(),
                usage: openalpaca_llm::Usage::default(),
                finish_reason: openalpaca_llm::FinishReason::Stop,
                thinking: None,
                parts: None,
            })
        }
    }

    /// **Design 11 §5 (owner decision CQ-7).** The turn's continuity reaches
    /// the handler's result — what the gateway puts on `done` — and is
    /// drained there: a new conversation's turn given the previous one names
    /// it, and nothing is left behind for the request id.
    #[tokio::test]
    async fn a_turn_given_the_previous_conversation_carries_it_to_the_result() {
        use openalpaca_core::security::policy::{Principal, Scope};

        let dir = tempfile::tempdir().unwrap();
        let db = openalpaca_storage::Database::open(&dir.path().join("t.db")).unwrap();
        let repo = openalpaca_storage::ConversationRepository::new(&db);
        let closed = repo.create_session("user1:cli", "cli", None, None).unwrap();
        let state = serde_json::json!({
            "v": 1, "topic": "Lisbon trip", "goal": "", "done": [], "decisions": [],
            "open": [], "next": "Compare the hotels", "refs": [],
        });
        assert!(
            repo.update_working_state(&closed.id, 0, &state.to_string(), 0)
                .unwrap()
        );
        repo.create_session("user1:cli", "cli", None, None).unwrap();

        let bus = openalpaca_core::bus::EventBus::default();
        let registry = Arc::new(openalpaca_core::tools::ToolRegistry::default());
        let gate = Arc::new(openalpaca_core::security::gate::SecurityGate::new(
            Arc::new(
                openalpaca_core::security::sandbox::SandboxManager::with_defaults(
                    registry.clone(),
                    bus.clone(),
                ),
            ),
        ));
        let orchestrator = Arc::new(Orchestrator::new(
            Arc::new(openalpaca_core::context::SharedContext::new()),
            Arc::new(openalpaca_core::lane::LaneManager::new()),
            bus,
            openalpaca_core::middleware::prompt::SystemPersona::default(),
            Some(Arc::new(openalpaca_llm::LlmRouter::single_provider(
                Arc::new(StubLlm),
                openalpaca_llm::ProviderType::Anthropic,
                "stub-model".to_string(),
            ))),
            openalpaca_core::runner::LoopConfig::default(),
            gate,
            registry,
            Some(db.clone()),
            Arc::new(openalpaca_core::orchestrator::skill_catalog::SkillCatalog::new()),
            Arc::new(openalpaca_core::orchestrator::skill_router::SkillRouter::new(0.65, 0.45)),
            Arc::new(arc_swap::ArcSwap::from_pointee(
                openalpaca_core::daemon_config::DaemonConfig::default(),
            )),
        ));
        let handler = OrchestratorHandler::new(orchestrator.clone());

        let request_id = Uuid::new_v4();
        let result = handler
            .handle(HandleRequest::new(
                request_id,
                "cli",
                "where were we?",
                Principal::User {
                    global_id: "user1".to_string(),
                },
                Scope::Global,
                "user1:cli",
            ))
            .await
            .expect("the turn is answered");

        assert_eq!(
            result.continuity.continued_from,
            Some(openalpaca_core::gateway::ContinuedFrom {
                session_id: closed.id,
                topic: "Lisbon trip".to_string(),
            })
        );
        assert!(!result.continuity.working_state_updated);
        assert_eq!(
            orchestrator.take_continuity(request_id),
            Default::default(),
            "the bridge took it"
        );
    }
}
