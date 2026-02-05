use crate::core_ctx::CoreCtx;
use openalpaca_core::{
    gateway::MessageHandler,
    middleware::prompt::AgentPersona,
    security::policy::{Principal, Scope},
};
use uuid::Uuid;

/// Bridges Gateway's MessageHandler trait to CoreCtx.
pub struct CoreCtxHandler {
    core_ctx: CoreCtx,
}

impl CoreCtxHandler {
    pub fn new(core_ctx: CoreCtx) -> Self {
        Self { core_ctx }
    }
}

impl MessageHandler for CoreCtxHandler {
    fn handle(
        &self,
        request_id: Uuid,
        source: String,
        content: String,
        principal: Principal,
        scope: Scope,
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
