use super::Orchestrator;
use crate::events::SystemEvent;
use crate::middleware::identity::identity_document_has_content;
use crate::middleware::user::user_document_has_content;
use chrono::Utc;
use std::sync::atomic::Ordering;

impl Orchestrator {
    /// Check if IDENTITY.md and USER.md have been populated; if so, finalize bootstrap.
    ///
    /// Runs post-response (like `extract_user_traits_background`) and:
    /// 1. Checks if `bootstrap_document` is Some (bootstrap mode active)
    /// 2. Checks if `identity_document` has content AND `user_document` has content
    /// 3. If both populated: delete BOOTSTRAP.md, clear state, publish event
    ///
    /// An `AtomicBool` guard ensures only one concurrent caller can proceed
    /// past the check-then-act boundary, preventing duplicate completion events.
    pub(super) async fn maybe_complete_bootstrap(&self) {
        // Quick check: are we even in bootstrap mode?
        if !self.is_bootstrapping() {
            return;
        }

        // Atomic guard: only one task proceeds past this point.
        // swap(true) returns the *previous* value — if it was already true,
        // another task is completing bootstrap, so bail out.
        if self.bootstrap_completing.swap(true, Ordering::AcqRel) {
            return;
        }

        // Check identity
        let identity_populated = self
            .identity_document
            .read()
            .map(|g| g.as_ref().is_some_and(identity_document_has_content))
            .unwrap_or(false);

        // Check user
        let user_populated = self
            .user_document
            .read()
            .map(|g| g.as_ref().is_some_and(user_document_has_content))
            .unwrap_or(false);

        if !identity_populated || !user_populated {
            tracing::debug!(
                "Bootstrap check: identity={}, user={} -- not yet complete",
                identity_populated,
                user_populated
            );
            // Reset flag so future calls can retry once docs are populated.
            self.bootstrap_completing.store(false, Ordering::Release);
            return;
        }

        // Both populated — complete bootstrap
        tracing::info!("Bootstrap onboarding complete! Identity and user profile populated.");

        // Delete BOOTSTRAP.md from disk
        if let Ok(guard) = self.bootstrap_path.read()
            && let Some(ref path) = *guard
        {
            match std::fs::remove_file(path) {
                Ok(()) => tracing::info!("Deleted BOOTSTRAP.md: {}", path.display()),
                Err(e) => tracing::warn!("Failed to delete BOOTSTRAP.md: {e}"),
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

        // Note: bootstrap_completing stays true — bootstrap is done, no further
        // attempts should proceed even if is_bootstrapping() check were racy.
    }
}
