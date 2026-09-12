//! What every per-incarnation plugin handle carries besides its channel: which
//! **load** it belongs to, and the ledger that says whether that load is still
//! the current one (extension design §3.0 Fact 3, §3.6 item 2).
//!
//! One binding, three handles. `PluginToolProxy`, `PluginSkillBridge` and
//! `PluginAgentBridge` are all built by `PluginManager` at the same point of
//! the same load, they all speak over a channel whose process can die, and they
//! all owe the same two things when it does:
//!
//! 1. **A log line.** The registry's `Plugin` arm returns the executor's error
//!    string verbatim with no `warn!` of its own, so a plugin transport failure
//!    was silent everywhere. It is logged here, where the typed error is.
//! 2. **An attributed refusal.** A `ChannelClosed`/`ProcessCrashed` from a
//!    plugin that is being disabled, was denied, or has already been marked
//!    failed must reach the model as the §7.1 wording, never as a broken-pipe
//!    string — and one that comes from a **previous** load must neither say
//!    "crashed" nor flip the healthy row that replaced it.

use std::sync::Arc;

use openalpaca_core::tools::extensions::{
    Audience, Described, ExtensionId, ExtensionLedger, FailureReason,
};
use tracing::warn;

use crate::error::PluginError;

/// The load a handle belongs to, and the ledger it answers to.
#[derive(Clone)]
pub(crate) struct LoadBinding {
    extension: ExtensionId,
    generation: u64,
    ledger: Arc<ExtensionLedger>,
}

impl LoadBinding {
    pub(crate) fn new(plugin_id: &str, generation: u64, ledger: Arc<ExtensionLedger>) -> Self {
        Self {
            extension: ExtensionId::plugin(plugin_id.to_string()),
            generation,
            ledger,
        }
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Is this handle from a load the ledger has since replaced?
    fn is_stale(&self) -> bool {
        self.ledger
            .generation(&self.extension)
            .is_some_and(|current| current != self.generation)
    }

    /// The S4 refusal owed for this handle right now, or `None` while the row
    /// reads `Enabled` at this load's generation.
    ///
    /// State is checked before generation (design §3.0 rule 3), so a stale
    /// handle to an extension that is *again* disabled gets the disabled
    /// wording, which is the more useful of the two.
    pub(crate) fn refusal(&self, tool: Option<&str>) -> Option<String> {
        if let Some(refusal) = self.ledger.refusal_if_not_enabled(&self.extension, tool) {
            return Some(refusal);
        }
        if !self.is_stale() {
            return None;
        }
        Some(match tool {
            Some(name) => Described::stale(&self.extension, name, Audience::Model)
                .render_model(Some(name)),
            None => Described::stale_run(&self.extension, Audience::Model).render_model(None),
        })
    }

    /// Turn one RPC failure into what the caller should see.
    ///
    /// A `ChannelClosed`/`ProcessCrashed` is the crash-detection trigger
    /// (design §3.6 item 2): log it, then `mark_failed` at **this** load's
    /// generation — which the ledger no-ops both outside `Enabled` and from a
    /// previous load, so a stale handle can never tear down the incarnation
    /// that replaced it. Whatever the ledger then reads decides the wording;
    /// only a genuinely `Enabled`, current-generation plugin surfaces its own
    /// error text.
    pub(crate) fn describe_failure(
        &self,
        subject: &str,
        tool: Option<&str>,
        error: &PluginError,
    ) -> String {
        if matches!(
            error,
            PluginError::ChannelClosed | PluginError::ProcessCrashed
        ) {
            warn!(
                plugin = %self.extension.name,
                subject,
                generation = self.generation,
                error = %error,
                "plugin transport failure"
            );
            self.ledger.mark_failed(
                &self.extension,
                self.generation,
                FailureReason::Crashed,
                error.to_string(),
            );
        }
        self.refusal(tool).unwrap_or_else(|| {
            format!("plugin {}::{}: {}", self.extension.name, subject, error)
        })
    }
}
