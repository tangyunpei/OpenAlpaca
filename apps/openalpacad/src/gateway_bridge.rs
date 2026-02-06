#[allow(deprecated)]
use crate::core_ctx::CoreCtx;
use async_trait::async_trait;
use openalpaca_core::{
    gateway::MessageHandler,
    middleware::prompt::AgentPersona,
    orchestrator::Orchestrator,
    security::policy::{Principal, Scope},
};
use std::sync::Arc;
use uuid::Uuid;

/// Bridges Gateway's MessageHandler trait to CoreCtx.
#[allow(deprecated, dead_code)]
pub struct CoreCtxHandler {
    core_ctx: CoreCtx,
}

#[allow(deprecated, dead_code)]
impl CoreCtxHandler {
    pub fn new(core_ctx: CoreCtx) -> Self {
        Self { core_ctx }
    }
}

#[allow(deprecated)]
#[async_trait]
impl MessageHandler for CoreCtxHandler {
    async fn handle(
        &self,
        request_id: Uuid,
        source: String,
        content: String,
        principal: Principal,
        scope: Scope,
        _lane_key: String,
    ) -> Result<String, String> {
        let agent_persona = AgentPersona {
            role: "Assistant".to_string(),
            tone: "Friendly".to_string(),
            domain_knowledge: vec![],
        };
        let output =
            self.core_ctx
                .handle_user_request(request_id, source, content, principal, scope, &agent_persona)?;
        Ok(output.content)
    }
}

/// Bridges Gateway's MessageHandler trait to the new Orchestrator.
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
