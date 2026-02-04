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

            // iMessage Special Policy: Even low risks might need confirmation if not whitelisted
            // (Strictly enforce "No Write" for now for unknown external)
            if provider == "imessage" && action.starts_with("fs.") {
                return Err(
                    "Permission Denied: iMessage user restricted from FS access.".to_string(),
                );
            }
        }

        // 4. Linked Users can generally do anything, BUT specific scopes might be guarded
        if let Principal::User { .. } = principal {
            // Future: Check granular ACLs here
            // For now, Trusted User is allowed.
            match scope {
                Scope::Global => {
                    // Critical system config changes might still require explicit confirmation (OTP)
                    // The TrustGate just checks "Permission", separate step for "Confirmation".
                    if action == "system.shutdown" {
                        // OK, but caller should verify OTP if managed.
                        // TrustGate says "You have RIGHT", PolicyMiddleware checks "You have PROOF".
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }
}
