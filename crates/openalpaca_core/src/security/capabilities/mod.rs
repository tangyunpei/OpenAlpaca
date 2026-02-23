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
        // Check deny list first (case-insensitive; constraint entries are pre-normalized)
        let tool_lower = tool_name.to_lowercase();
        if constraints.denied_capabilities.contains(&tool_lower) {
            return Err(SecurityViolation::CapabilityDenied {
                agent_id: agent_id.to_string(),
                capability: tool_name.to_string(),
            });
        }

        // If allow list is non-empty, tool must be on it (case-insensitive)
        if !constraints.allowed_capabilities.is_empty()
            && !constraints.allowed_capabilities.contains(&tool_lower)
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
        // Check deny list first (case-insensitive; constraint entries are pre-normalized)
        let model_lower = model_id.to_lowercase();
        if constraints.denied_models.contains(&model_lower) {
            return Err(SecurityViolation::UnauthorizedModelAccess {
                agent_id: agent_id.to_string(),
                model_id: model_id.to_string(),
                reason: format!("Model '{}' is on the deny list", model_id),
            });
        }

        // If allow list is non-empty, model must be on it (case-insensitive)
        if !constraints.allowed_models.is_empty()
            && !constraints.allowed_models.contains(&model_lower)
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
mod tests;
