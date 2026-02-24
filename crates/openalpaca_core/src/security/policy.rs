use crate::types::Capability;
use serde::{Deserialize, Serialize};

/// The entity requesting access.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Principal {
    /// Internal system components (Full access)
    System,
    /// A linked, authenticated global user (High trust)
    User { global_id: String },
    /// An unlinked external identity (Low trust)
    External { provider: String, id: String },
}

/// The resource scope being accessed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Scope {
    /// Global scope, affects everything
    Global,
    /// Restricted to a specific workspace (e.g. current project)
    Workspace { path: String },
    /// Restricted to the current conversation
    Conversation { id: String },
}

/// The Gatekeeper for all sensitive operations.
/// Implements Capability-Based Access Control (CBAC).
pub struct TrustGate;

impl TrustGate {
    /// Check if a principal is allowed to perform a capability on a scope.
    pub fn check(
        principal: &Principal,
        capability: &Capability,
        scope: &Scope,
    ) -> Result<(), String> {
        let action = &capability.name;

        // 1. System is God
        if matches!(principal, Principal::System) {
            return Ok(());
        }

        // 2. Define High-Risk Actions
        let is_high_risk = action.starts_with("system.")
            || action.starts_with("fs.write")
            || action.starts_with("net.connect");

        // 3. Unlinked External Users cannot do High-Risk Actions
        if let Principal::External { provider, .. } = principal {
            if is_high_risk {
                return Err(format!(
                    "Permission Denied: Unlinked {} user cannot perform high-risk action '{}'. Please link your account.",
                    provider, action
                ));
            }

            // Explicitly deny chat.respond for unlinked users to force /link flow
            // Exception: We might want allow-listed commands later, but currently
            // the connector handles /link *before* calling TrustGate.
            if action == "chat.respond" {
                return Err(format!(
                    "Permission Denied: Unlinked {} user cannot chat. Please link your account.",
                    provider
                ));
            }

        }

        // 4. Linked Users can generally do anything, BUT specific scopes might be guarded
        if let Principal::User { .. } = principal {
            // Future: Check granular ACLs here
            // For now, Trusted User is allowed.
            if let Scope::Global = scope {
                // Critical system config changes might still require explicit confirmation (OTP)
                // The TrustGate just checks "Permission", separate step for "Confirmation".
                if action == "system.shutdown" {
                    // OK, but caller should verify OTP if managed.
                    // TrustGate says "You have RIGHT", PolicyMiddleware checks "You have PROOF".
                }
            }
        }

        Ok(())
    }
}
