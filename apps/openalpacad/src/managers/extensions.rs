//! [`Extensions`] — the aggregator `AppState` holds for the ENABLE axis
//! (extension design §6.2 #15).
//!
//! The ledger alone cannot serve a route: it cannot run T2–T4, write
//! `mcp.toml` or `.permissions.toml`, or `disconnect`. Those are supervisor
//! work, and there are two supervisors. This type is the one handle that owns
//! both, dispatches a verb to the right one by `kind`, and is the only thing
//! `apps/openalpacad/src/routes/extensions.rs` needs.
//!
//! It is **not** a third state machine: every method here is a thin dispatch
//! onto `ExtensionSupervisor` or onto a plugin-only verb. Nothing in this file
//! decides a state, a status code or a wording.

use std::sync::Arc;
use std::time::Duration;

use openalpaca_core::tools::extensions::{
    ExtensionError, ExtensionId, ExtensionKind, ExtensionRecord, ExtensionState,
    ExtensionSupervisor,
};
use openalpaca_plugins::PluginManager;

use crate::managers::mcp::McpSupervisor;

/// How long `shutdown_all` is allowed to take per supervisor.
///
/// C2's `shutdown_all` drains **sequentially** with a per-server bound of
/// `drain_timeout_secs` plus one in-flight call, and `main.rs` called it with no
/// outer bound at all — so N busy extensions could hold the daemon's exit for N
/// times that.
///
/// The number is deliberately a fraction of the daemon's own force-exit
/// watchdog (`main.rs`: `process::exit` 10 s after cancellation), because that
/// watchdog is the only thing bounding it today and everything *after* the
/// sweep — the connector shutdown, the wake manager — has to fit in the same
/// 10 s. Expiring costs nothing that was not already lost: it abandons the
/// *wait*, not the teardown. Each `disconnect`/`kill` has already been issued
/// and runs on to completion or to `process::exit`, which is exactly where a
/// straggler was before C2 existed.
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(5);

/// `{ ledger, mcp, plugins }` — design §3's aggregator.
pub struct Extensions {
    mcp: Arc<McpSupervisor>,
    plugins: Arc<PluginManager>,
}

impl Extensions {
    pub fn new(mcp: Arc<McpSupervisor>, plugins: Arc<PluginManager>) -> Arc<Self> {
        Arc::new(Self { mcp, plugins })
    }

    pub fn mcp(&self) -> &Arc<McpSupervisor> {
        &self.mcp
    }

    pub fn plugins(&self) -> &Arc<PluginManager> {
        &self.plugins
    }

    /// The `{kind}` path segment. An unknown word is a `404`, not a `400`:
    /// `/v1/extensions/banana/x/enable` names no resource.
    pub fn parse_kind(kind: &str) -> Option<ExtensionKind> {
        match kind {
            "mcp" => Some(ExtensionKind::Mcp),
            "plugin" => Some(ExtensionKind::Plugin),
            _ => None,
        }
    }

    fn supervisor(&self, kind: ExtensionKind) -> &dyn ExtensionSupervisor {
        match kind {
            ExtensionKind::Mcp => self.mcp.as_ref(),
            ExtensionKind::Plugin => self.plugins.as_ref(),
        }
    }

    // ── Reads ────────────────────────────────────────────────────────

    /// Every row from both supervisors, sorted by `(kind, id)` so the bare
    /// array is stable across calls (design §8; `DashMap` iteration jitters).
    ///
    /// `Orphaned` rows are omitted unless `include_orphaned` — `?include_orphaned=true`,
    /// default `false`.
    pub async fn list(&self, include_orphaned: bool) -> Vec<ExtensionRecord> {
        let mut rows = self.mcp.list().await;
        rows.extend(self.plugins.list().await);
        if !include_orphaned {
            rows.retain(|r| !matches!(r.state, ExtensionState::Orphaned));
        }
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        rows
    }

    // ── Verbs ────────────────────────────────────────────────────────

    pub async fn enable(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError> {
        self.supervisor(id.kind).enable(id).await
    }

    pub async fn disable(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError> {
        self.supervisor(id.kind).disable(id).await
    }

    pub async fn reload(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError> {
        self.supervisor(id.kind).reload(id).await
    }

    /// Plugins only: writing a server into your own `config/mcp.toml` *is* the
    /// consent, so `kind=mcp` is `409 unsupported_for_kind` (design §8).
    pub async fn approve(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError> {
        self.require_plugin(id)?;
        self.plugins.approve_plugin(&id.name).await
    }

    pub async fn deny(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError> {
        self.require_plugin(id)?;
        self.plugins.deny_plugin(&id.name).await
    }

    /// `DELETE /v1/extensions/plugin/{id}` — `Orphaned` rows only.
    pub async fn remove(&self, id: &ExtensionId) -> Result<(), ExtensionError> {
        self.require_plugin(id)?;
        self.plugins.remove_orphan(&id.name).await
    }

    fn require_plugin(&self, id: &ExtensionId) -> Result<(), ExtensionError> {
        if id.kind == ExtensionKind::Plugin {
            Ok(())
        } else {
            Err(ExtensionError::UnsupportedForKind)
        }
    }

    // ── Shutdown ─────────────────────────────────────────────────────

    /// **§3.5.** T2–T4 for every `Enabled` extension of both kinds, under one
    /// bounded budget per supervisor.
    pub async fn shutdown_all(&self) {
        for (kind, supervisor) in [
            ("mcp", self.supervisor(ExtensionKind::Mcp)),
            ("plugin", self.supervisor(ExtensionKind::Plugin)),
        ] {
            if tokio::time::timeout(SHUTDOWN_BUDGET, supervisor.shutdown_all())
                .await
                .is_err()
            {
                tracing::warn!(
                    kind,
                    budget_secs = SHUTDOWN_BUDGET.as_secs(),
                    "extension shutdown budget expired; abandoning the rest of the drain"
                );
            }
        }
    }
}
