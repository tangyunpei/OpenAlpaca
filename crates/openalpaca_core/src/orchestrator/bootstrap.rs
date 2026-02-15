use super::Orchestrator;
use crate::events::SystemEvent;
use crate::middleware::identity::identity_document_has_content;
use crate::middleware::user::user_document_has_content;
use chrono::Utc;

impl Orchestrator {
    /// Check if IDENTITY.md and USER.md have been populated; if so, finalize bootstrap.
    ///
    /// Runs post-response (like `maybe_extract_user_traits`) and:
    /// 1. Checks if `bootstrap_document` is Some (bootstrap mode active)
    /// 2. Checks if `identity_document` has content AND `user_document` has content
    /// 3. If both populated: delete BOOTSTRAP.md, clear state, publish event
    pub(super) async fn maybe_complete_bootstrap(&self) {
        // Quick check: are we even in bootstrap mode?
        if !self.is_bootstrapping() {
            return;
        }

        // Check identity
        let identity_populated = self
            .identity_document
            .read()
            .map(|g| {
                g.as_ref()
                    .map_or(false, |d| identity_document_has_content(d))
            })
            .unwrap_or(false);

        // Check user
        let user_populated = self
            .user_document
            .read()
            .map(|g| {
                g.as_ref()
                    .map_or(false, |d| user_document_has_content(d))
            })
            .unwrap_or(false);

        if !identity_populated || !user_populated {
            tracing::debug!(
                "Bootstrap check: identity={}, user={} -- not yet complete",
                identity_populated,
                user_populated
            );
            return;
        }

        // Both populated — complete bootstrap
        tracing::info!("Bootstrap onboarding complete! Identity and user profile populated.");

        // Delete BOOTSTRAP.md from disk
        if let Ok(guard) = self.bootstrap_path.read() {
            if let Some(ref path) = *guard {
                match std::fs::remove_file(path) {
                    Ok(()) => tracing::info!("Deleted BOOTSTRAP.md: {}", path.display()),
                    Err(e) => tracing::warn!("Failed to delete BOOTSTRAP.md: {e}"),
                }
            }
        }

        // Clear in-memory state
        self.update_bootstrap_document(None);

        // Publish event
        self.bus.publish(SystemEvent::BootstrapCompleted {
            identity_populated,
            user_populated,
            timestamp: Utc::now(),
        });
    }
}
