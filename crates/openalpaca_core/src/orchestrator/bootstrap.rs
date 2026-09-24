use super::Orchestrator;
use crate::events::SystemEvent;
use crate::middleware::identity::{identity_document_has_content, parse_identity_markdown};
use crate::middleware::user::{parse_user_markdown, user_document_has_content};
use chrono::Utc;
use std::path::PathBuf;
use std::sync::RwLock;
use std::sync::atomic::Ordering;

impl Orchestrator {
    /// Check if IDENTITY.md and USER.md have been populated; if so, finalize bootstrap.
    ///
    /// Runs post-response and:
    /// 1. Checks if `bootstrap_document` is Some (bootstrap mode active)
    /// 2. Checks if IDENTITY.md has content AND USER.md has content
    /// 3. If both populated: delete BOOTSTRAP.md, clear state, publish event
    ///
    /// **The files are the evidence (MD-3).** The in-memory documents are
    /// consulted first, and when either says *not yet* the file on disk is
    /// read and parsed — that is what `update_persona` writes during
    /// onboarding, and it is the whole of what completion depends on. The
    /// background extractor used to be the other writer of the in-memory
    /// `USER.md`; it writes memory notes now, and onboarding must not wait
    /// for a reload that races the turn.
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

        // Check identity: the live document, else the file.
        let identity_populated = self
            .identity_document
            .read()
            .map(|g| g.as_ref().is_some_and(identity_document_has_content))
            .unwrap_or(false)
            || file_has_content(&self.identity_path, |text| {
                parse_identity_markdown(text)
                    .map(|doc| identity_document_has_content(&doc))
                    .unwrap_or(false)
            });

        // Check user: the live document, else the file.
        let user_populated = self
            .user_document
            .read()
            .map(|g| g.as_ref().is_some_and(user_document_has_content))
            .unwrap_or(false)
            || file_has_content(&self.user_path, |text| {
                parse_user_markdown(text)
                    .map(|doc| user_document_has_content(&doc))
                    .unwrap_or(false)
            });

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

        // Delete BOOTSTRAP.md from disk.
        //
        // **S8 — stop polling it first.** The wake watcher polls this exact
        // path, and a poll that walks a file this process has just removed
        // logs `WARN notify::poll::data: walkdir error scanning … NotFound`
        // naming the file. The hot-reload watcher unwatches it too, but only
        // once it has been *told* the file is gone — which is the scan that
        // already warned. Deleting it ourselves means we know first, so the
        // path leaves the poll set before there is anything to trip over. A
        // hand-deleted BOOTSTRAP.md still goes the reactive way.
        if let Ok(guard) = self.bootstrap_path.read()
            && let Some(ref path) = *guard
        {
            self.unwatch_path(path);
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

/// Whether the persona file at `path` — when one is set and readable — is
/// populated by `is_populated`'s reading of its text.
fn file_has_content(
    path: &RwLock<Option<PathBuf>>,
    is_populated: impl FnOnce(&str) -> bool,
) -> bool {
    let path = match path.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let Some(path) = path else {
        return false;
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => is_populated(&text),
        Err(e) => {
            tracing::debug!("Bootstrap check: cannot read {}: {e}", path.display());
            false
        }
    }
}
