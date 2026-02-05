//! Core Context (CoreCtx)
//!
//! The "Adapter Layer" that bridges openalpaca_core into the daemon.
//! This struct holds all core modules and provides a unified interface.

use chrono::Utc;
use openalpaca_core::{
    bus::EventBus,
    events::SystemEvent,
    middleware::{
        guard::OutputGuard,
        prompt::{AgentPersona, PromptAssembler, SystemPersona},
    },
    security::policy::{Principal, Scope, TrustGate},
    types::{AgentOutput, Capability},
};
use uuid::Uuid;

/// CoreCtx holds all core modules for the daemon.
/// This is the single integration point for new logic.
#[deprecated(since = "0.5.0", note = "Use Orchestrator via Gateway instead")]
#[derive(Clone)]
pub struct CoreCtx {
    pub bus: EventBus,
    pub system_persona: SystemPersona,
}

#[allow(deprecated)]
impl CoreCtx {
    pub fn new() -> Self {
        Self {
            bus: EventBus::default(),
            system_persona: SystemPersona::default(),
        }
    }

    /// The unified entry point for all user requests.
    /// This implements the 5-step pipeline:
    /// 1. TrustGate.check()
    /// 2. PromptAssembler.assemble()
    /// 3. Agent/LLM call (stubbed)
    /// 4. OutputGuard.validate_or_repair()
    /// 5. Emit SystemEvent::AgentResponse
    pub fn handle_user_request(
        &self,
        request_id: Uuid,
        _source: String,
        content: String,
        principal: Principal,
        scope: Scope,
        agent_persona: &AgentPersona,
    ) -> Result<AgentOutput, String> {
        // Step 1: Permission Check
        let capability = Capability {
            name: "chat.respond".to_string(),
        };
        TrustGate::check(&principal, &capability, &scope)?;

        // Step 2: Assemble Prompt
        let _full_prompt = PromptAssembler::assemble(&self.system_persona, agent_persona, &content);

        // Step 3: Agent/LLM Call (STUBBED for now)
        // In a real implementation, this would call an LLM or route to an agent.
        let raw_response = format!(
            "{{\"status\": \"ok\", \"echo\": \"Received: {}\"}}",
            content.chars().take(50).collect::<String>()
        );

        // Step 4: Output Guard (Validate/Repair JSON)
        let validated_response = OutputGuard::ensure_json(&raw_response)?;

        // Step 5: Emit Event
        let output = AgentOutput {
            content: validated_response.clone(),
        };

        self.bus.publish(SystemEvent::AgentResponse {
            request_id,
            agent_id: "stub_agent".to_string(),
            content: validated_response,
            timestamp: Utc::now(),
        });

        Ok(output)
    }
}
