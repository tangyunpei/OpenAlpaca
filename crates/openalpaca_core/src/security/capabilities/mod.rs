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

/// The ALLOW axis of tool governance: which capabilities a policy admits.
///
/// An *empty* allow list is a total restriction, not an absent one — the
/// distinction the `Vec<String>` this replaced could not express (bug A): a
/// plugin skill whose providing extension was absent resolved its requirements
/// to an empty list and was handed the entire tool surface, so disabling an
/// extension *widened* reach. [`Allowlist::Only`] with no entries now admits
/// nothing, and a caller that genuinely means "no allow-list restriction" has
/// to spell [`Allowlist::Unrestricted`].
///
/// Ambient capabilities are granted constructor-side, not here:
/// `AgentTemplate::to_subagent` appends `workspace_read`/`workspace_write` to
/// the template's list, so they arrive as ordinary members of `Only(..)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Allowlist {
    /// No allow-list restriction — only the deny list applies.
    Unrestricted,
    /// Exactly these capabilities (pre-lowercased), and nothing else.
    Only(Vec<String>),
}

impl Allowlist {
    /// The allow list an agent's constraints spell.
    ///
    /// Template-declared capabilities are a closed set: whatever the template
    /// granted is all the agent gets, and a template that granted nothing
    /// yields an agent that can call nothing.
    pub fn from_agent_constraints(constraints: &AgentConstraints) -> Self {
        Self::Only(constraints.allowed_capabilities.clone())
    }

    /// Does this allow list admit `capability` (already lowercased)?
    pub fn admits(&self, capability_lower: &str) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::Only(names) => names.iter().any(|n| n == capability_lower),
        }
    }
}

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
    /// - If the tool is on `denied`, always block — deny wins over allow.
    /// - If `allowed` is [`Allowlist::Only`] and the tool is not on it, block.
    ///   An empty `Only` therefore denies everything.
    /// - Otherwise, allow.
    pub fn check_agent_capability(
        agent_id: &str,
        tool_name: &str,
        allowed: &Allowlist,
        denied: &[String],
    ) -> Result<(), SecurityViolation> {
        // Check deny list first (case-insensitive; list entries are pre-normalized)
        let tool_lower = tool_name.to_lowercase();
        if denied.iter().any(|d| d == &tool_lower) {
            return Err(SecurityViolation::CapabilityDenied {
                agent_id: agent_id.to_string(),
                capability: tool_name.to_string(),
            });
        }

        if !allowed.admits(&tool_lower) {
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
