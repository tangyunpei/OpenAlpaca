//! Layer 1: Capability-based access control.
//!
//! Wraps the existing TrustGate with per-agent capability enforcement.

use crate::agent::subagent::AgentConstraints;
use crate::security::policy::{Principal, Scope, TrustGate};
use crate::types::Capability;
use std::fmt;

/// A security policy violation.
#[derive(Debug, Clone)]
pub enum SecurityViolation {
    /// TrustGate denied access for a principal.
    AccessDenied { reason: String },
    /// Agent tried to use a capability on the deny list.
    CapabilityDenied {
        agent_id: String,
        capability: String,
    },
    /// Agent tried to use a capability not on the allow list.
    CapabilityNotAllowed {
        agent_id: String,
        capability: String,
    },
    /// User input was blocked by sanitization.
    InputBlocked { reason: String },
    /// Tool execution violated sandbox policy.
    SandboxViolation {
        agent_id: String,
        tool_name: String,
        reason: String,
    },
    /// Agent tried to use a model it's not authorized for.
    UnauthorizedModelAccess {
        agent_id: String,
        model_id: String,
        reason: String,
    },
}

impl fmt::Display for SecurityViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccessDenied { reason } => write!(f, "Access denied: {}", reason),
            Self::CapabilityDenied {
                agent_id,
                capability,
            } => write!(
                f,
                "Capability '{}' denied for agent '{}'",
                capability, agent_id
            ),
            Self::CapabilityNotAllowed {
                agent_id,
                capability,
            } => write!(
                f,
                "Capability '{}' not in allow list for agent '{}'",
                capability, agent_id
            ),
            Self::InputBlocked { reason } => write!(f, "Input blocked: {}", reason),
            Self::SandboxViolation {
                agent_id,
                tool_name,
                reason,
            } => write!(
                f,
                "Sandbox violation: agent='{}', tool='{}', reason='{}'",
                agent_id, tool_name, reason
            ),
            Self::UnauthorizedModelAccess {
                agent_id,
                model_id,
                reason,
            } => write!(
                f,
                "Model access denied: agent='{}', model='{}', reason='{}'",
                agent_id, model_id, reason
            ),
        }
    }
}

impl std::error::Error for SecurityViolation {}

/// Manages capability checks at principal and agent level.
pub struct CapabilityManager;

impl CapabilityManager {
    /// Delegate to TrustGate for principal-level access control.
    pub fn check_principal(
        principal: &Principal,
        capability: &Capability,
        scope: &Scope,
    ) -> Result<(), SecurityViolation> {
        TrustGate::check(principal, capability, scope)
            .map_err(|reason| SecurityViolation::AccessDenied { reason })
    }

    /// Check whether an agent is allowed to use a particular tool/capability.
    ///
    /// Rules:
    /// - If the tool is on `denied_capabilities`, always block.
    /// - If `allowed_capabilities` is non-empty and the tool is NOT on it, block.
    /// - Otherwise, allow.
    pub fn check_agent_capability(
        agent_id: &str,
        tool_name: &str,
        constraints: &AgentConstraints,
    ) -> Result<(), SecurityViolation> {
        // Check deny list first
        if constraints
            .denied_capabilities
            .iter()
            .any(|d| d == tool_name)
        {
            return Err(SecurityViolation::CapabilityDenied {
                agent_id: agent_id.to_string(),
                capability: tool_name.to_string(),
            });
        }

        // If allow list is non-empty, tool must be on it
        if !constraints.allowed_capabilities.is_empty()
            && !constraints
                .allowed_capabilities
                .iter()
                .any(|a| a == tool_name)
        {
            return Err(SecurityViolation::CapabilityNotAllowed {
                agent_id: agent_id.to_string(),
                capability: tool_name.to_string(),
            });
        }

        Ok(())
    }

    /// Check whether an agent is allowed to use a particular model.
    ///
    /// Rules (same deny/allow pattern as capabilities):
    /// - If the model is on `denied_models`, always block.
    /// - If `allowed_models` is non-empty and the model is NOT on it, block.
    /// - Otherwise, allow.
    pub fn check_model_access(
        agent_id: &str,
        model_id: &str,
        constraints: &AgentConstraints,
    ) -> Result<(), SecurityViolation> {
        // Check deny list first
        if constraints.denied_models.iter().any(|d| d == model_id) {
            return Err(SecurityViolation::UnauthorizedModelAccess {
                agent_id: agent_id.to_string(),
                model_id: model_id.to_string(),
                reason: format!("Model '{}' is on the deny list", model_id),
            });
        }

        // If allow list is non-empty, model must be on it
        if !constraints.allowed_models.is_empty()
            && !constraints.allowed_models.iter().any(|a| a == model_id)
        {
            return Err(SecurityViolation::UnauthorizedModelAccess {
                agent_id: agent_id.to_string(),
                model_id: model_id.to_string(),
                reason: format!("Model '{}' is not in the allow list", model_id),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_constraints() -> AgentConstraints {
        AgentConstraints::default()
    }

    #[test]
    fn test_system_principal_bypasses() {
        let cap = Capability {
            name: "system.shutdown".to_string(),
        };
        assert!(CapabilityManager::check_principal(&Principal::System, &cap, &Scope::Global).is_ok());
    }

    #[test]
    fn test_external_user_blocked_high_risk() {
        let cap = Capability {
            name: "system.shutdown".to_string(),
        };
        let principal = Principal::External {
            provider: "telegram".to_string(),
            id: "user123".to_string(),
        };
        let result = CapabilityManager::check_principal(&principal, &cap, &Scope::Global);
        assert!(result.is_err());
        match result.unwrap_err() {
            SecurityViolation::AccessDenied { reason } => {
                assert!(reason.contains("Permission Denied"));
            }
            other => panic!("Expected AccessDenied, got: {:?}", other),
        }
    }

    #[test]
    fn test_user_principal_allowed() {
        let cap = Capability {
            name: "chat.respond".to_string(),
        };
        let principal = Principal::User {
            global_id: "user1".to_string(),
        };
        assert!(CapabilityManager::check_principal(&principal, &cap, &Scope::Global).is_ok());
    }

    #[test]
    fn test_denied_capability_blocked() {
        let constraints = AgentConstraints {
            denied_capabilities: vec!["shell_execute".to_string()],
            ..default_constraints()
        };
        let result =
            CapabilityManager::check_agent_capability("agent1", "shell_execute", &constraints);
        assert!(result.is_err());
        match result.unwrap_err() {
            SecurityViolation::CapabilityDenied { agent_id, capability } => {
                assert_eq!(agent_id, "agent1");
                assert_eq!(capability, "shell_execute");
            }
            other => panic!("Expected CapabilityDenied, got: {:?}", other),
        }
    }

    #[test]
    fn test_allowed_capability_enforced() {
        let constraints = AgentConstraints {
            allowed_capabilities: vec!["web_search".to_string(), "summarize".to_string()],
            ..default_constraints()
        };
        // Allowed tool passes
        assert!(
            CapabilityManager::check_agent_capability("agent1", "web_search", &constraints).is_ok()
        );
        // Unlisted tool is blocked
        let result =
            CapabilityManager::check_agent_capability("agent1", "shell_execute", &constraints);
        assert!(result.is_err());
        match result.unwrap_err() {
            SecurityViolation::CapabilityNotAllowed { .. } => {}
            other => panic!("Expected CapabilityNotAllowed, got: {:?}", other),
        }
    }

    #[test]
    fn test_empty_allow_list_allows_all() {
        let constraints = default_constraints();
        assert!(
            CapabilityManager::check_agent_capability("agent1", "anything", &constraints).is_ok()
        );
    }

    // ── Model access tests ──────────────────────────────────────

    #[test]
    fn test_model_access_no_constraints() {
        let constraints = default_constraints();
        assert!(
            CapabilityManager::check_model_access("agent1", "claude-sonnet-4-5-20250929", &constraints).is_ok()
        );
    }

    #[test]
    fn test_model_access_denied_model() {
        let constraints = AgentConstraints {
            denied_models: vec!["gpt-4o".to_string()],
            ..default_constraints()
        };
        let result = CapabilityManager::check_model_access("agent1", "gpt-4o", &constraints);
        assert!(result.is_err());
        match result.unwrap_err() {
            SecurityViolation::UnauthorizedModelAccess { agent_id, model_id, .. } => {
                assert_eq!(agent_id, "agent1");
                assert_eq!(model_id, "gpt-4o");
            }
            other => panic!("Expected UnauthorizedModelAccess, got: {:?}", other),
        }
    }

    #[test]
    fn test_model_access_not_in_allow_list() {
        let constraints = AgentConstraints {
            allowed_models: vec!["claude-sonnet-4-5-20250929".to_string()],
            ..default_constraints()
        };
        let result = CapabilityManager::check_model_access("agent1", "gpt-4o", &constraints);
        assert!(result.is_err());
    }

    #[test]
    fn test_model_access_in_allow_list() {
        let constraints = AgentConstraints {
            allowed_models: vec!["claude-sonnet-4-5-20250929".to_string()],
            ..default_constraints()
        };
        assert!(
            CapabilityManager::check_model_access("agent1", "claude-sonnet-4-5-20250929", &constraints).is_ok()
        );
    }

    #[test]
    fn test_unauthorized_model_access_display() {
        let v = SecurityViolation::UnauthorizedModelAccess {
            agent_id: "a1".to_string(),
            model_id: "gpt-4o".to_string(),
            reason: "denied".to_string(),
        };
        let s = format!("{}", v);
        assert!(s.contains("a1"));
        assert!(s.contains("gpt-4o"));
    }
}
