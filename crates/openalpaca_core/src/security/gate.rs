//! SecurityGate: facade composing all three security layers.
//!
//! - Layer 1: CapabilityManager (TrustGate wrap + per-agent capabilities)
//! - Layer 2: InputSanitizer (user input + tool arg validation)
//! - Layer 3: SandboxManager (tool execution with timeout + events)

use crate::security::capabilities::CapabilityManager;
use crate::security::sandbox::SandboxManager;
use crate::security::sanitizer::InputSanitizer;
use crate::security::policy::{Principal, Scope};
use crate::types::Capability;
use std::sync::Arc;

/// Unified security facade for the Orchestrator.
pub struct SecurityGate {
    pub sandbox_manager: Arc<SandboxManager>,
}

impl SecurityGate {
    pub fn new(sandbox_manager: Arc<SandboxManager>) -> Self {
        Self { sandbox_manager }
    }

    /// Layer 1: Check principal access (delegates to TrustGate via CapabilityManager).
    pub fn check_access(
        principal: &Principal,
        capability: &Capability,
        scope: &Scope,
    ) -> Result<(), String> {
        CapabilityManager::check_principal(principal, capability, scope)
            .map_err(|v| v.to_string())
    }

    /// Layer 2: Sanitize user input.
    ///
    /// Pass `None` for `max_input_length` to use the compiled default (32768 bytes).
    pub fn sanitize_input(input: &str, max_input_length: Option<usize>) -> Result<String, String> {
        InputSanitizer::sanitize_user_input(input, max_input_length).map_err(|v| v.to_string())
    }

    /// Layer 3: Access the sandbox manager for tool execution.
    pub fn sandbox(&self) -> &SandboxManager {
        &self.sandbox_manager
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::EventBus;
    use crate::security::sandbox::ToolExecutor;
    use async_trait::async_trait;

    struct NoopExecutor;

    #[async_trait]
    impl ToolExecutor for NoopExecutor {
        async fn execute(
            &self,
            _tool_name: &str,
            _arguments: &serde_json::Value,
        ) -> Result<String, String> {
            Ok("ok".to_string())
        }
        fn registered_tools(&self) -> Vec<String> {
            vec![]
        }
    }

    fn make_gate() -> SecurityGate {
        let sandbox = Arc::new(SandboxManager::new(
            Arc::new(NoopExecutor),
            EventBus::default(),
        ));
        SecurityGate::new(sandbox)
    }

    #[test]
    fn test_check_access_delegates() {
        let cap = Capability {
            name: "chat.respond".to_string(),
        };
        // System principal should pass
        assert!(SecurityGate::check_access(&Principal::System, &cap, &Scope::Global).is_ok());

        // External principal should fail for chat.respond
        let result = SecurityGate::check_access(
            &Principal::External {
                provider: "telegram".to_string(),
                id: "x".to_string(),
            },
            &cap,
            &Scope::Global,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Access denied"));
    }

    #[test]
    fn test_sanitize_delegates() {
        assert!(SecurityGate::sanitize_input("hello world", None).is_ok());
        assert!(SecurityGate::sanitize_input("hello\0world", None).is_err());
    }

    #[test]
    fn test_sandbox_accessor() {
        let gate = make_gate();
        // Just verify we can access the sandbox
        let _sandbox = gate.sandbox();
    }
}
