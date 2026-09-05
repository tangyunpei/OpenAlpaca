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
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use openalpaca_core::daemon_config::DaemonConfig;
use openalpaca_core::tools::extensions::{
    ExtensionError, ExtensionId, ExtensionKind, ExtensionRecord, ExtensionState,
    ExtensionSupervisor,
};
use openalpaca_plugins::PluginManager;

use crate::managers::mcp::McpSupervisor;

/// What a sweep's T4 may add on top of the T3 drain: the plugin half waits
/// `CHILD_EXIT_TIMEOUT` (2 s) for a child to exit after `shutdown`, and the MCP
/// half waits one `request_timeout` for `disconnect` to take the transport
/// mutex. One bound covers the last straggler of either kind.
const T4_BOUND: Duration = Duration::from_secs(2);

/// What the sweep must leave behind for everything that shuts down **after** it
/// — the connector manager, the wake manager, the discovery file — before the
/// daemon's force-exit watchdog fires. The sweep gets the window minus this,
/// never the whole window.
const POST_SWEEP_RESERVE: Duration = Duration::from_secs(3);

/// `{ ledger, mcp, plugins }` — design §3's aggregator.
pub struct Extensions {
    mcp: Arc<McpSupervisor>,
    plugins: Arc<PluginManager>,
    /// `[extensions] drain_timeout_secs`, read at shutdown so an owner who
    /// raises the drain gets a sweep budget that follows it.
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
}

impl Extensions {
    pub fn new(
        mcp: Arc<McpSupervisor>,
        plugins: Arc<PluginManager>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            mcp,
            plugins,
            daemon_config,
        })
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

    /// The config pair's guard: `plugin` kind **and** an id this daemon knows.
    ///
    /// `409 unsupported_for_kind` for `mcp`, `404` for an id no scan and no
    /// ledger record has ever seen — the same `404` every other verb answers,
    /// so the family reads one way.
    pub async fn known_plugin(&self, id: &ExtensionId) -> Result<(), ExtensionError> {
        self.require_plugin(id)?;
        self.plugins.known(id).await
    }

    // ── Shutdown ─────────────────────────────────────────────────────

    /// **§3.5.** T2–T4 for every `Enabled` extension of both kinds, under **one
    /// budget shared by the two supervisors**.
    ///
    /// C2's `shutdown_all` drains **sequentially** with a per-server bound of
    /// `drain_timeout_secs` plus one in-flight call, and `main.rs` called it
    /// with no outer bound at all — so N busy extensions could hold the
    /// daemon's exit for N times that.
    ///
    /// The budget is `min(drain_timeout_secs + T4, window − reserve)`:
    ///
    /// * the configured half follows `[extensions] drain_timeout_secs`, so
    ///   raising the drain raises the sweep rather than leaving it at a
    ///   literal;
    /// * `window` is what is left of the daemon's force-exit watchdog
    ///   (`main.rs`: `process::exit` after `FORCE_EXIT_GRACE`), which is the
    ///   only bound the whole shutdown has, minus what the connector and wake
    ///   shutdowns after the sweep need.
    ///
    /// It is one deadline across **both** supervisors, not one each: two
    /// per-supervisor budgets add up, and the worst case has to fit inside the
    /// same window. Each supervisor gets an even share of what is *left* when
    /// its turn comes, so a slow MCP drain cannot starve the plugin teardown
    /// (which is the half that leaves child processes behind), and an early
    /// finish hands its unused time to the next one.
    ///
    /// Expiring costs nothing that was not already lost: it abandons the
    /// *wait*, not the teardown. Every `disconnect`/`kill` already issued runs
    /// on to completion or to `process::exit`, which is exactly where a
    /// straggler was before C2 existed.
    pub async fn shutdown_all(&self, window: Duration) {
        let configured =
            Duration::from_secs(self.daemon_config.load().extensions.drain_timeout_secs) + T4_BOUND;
        let budget = configured.min(window.saturating_sub(POST_SWEEP_RESERVE));
        let deadline = Instant::now() + budget;
        tracing::debug!(
            budget_ms = budget.as_millis(),
            configured_ms = configured.as_millis(),
            window_ms = window.as_millis(),
            "extension shutdown sweep starting"
        );

        let sweeps = [
            ("mcp", self.supervisor(ExtensionKind::Mcp)),
            ("plugin", self.supervisor(ExtensionKind::Plugin)),
        ];
        let total = sweeps.len();
        for (index, (kind, supervisor)) in sweeps.into_iter().enumerate() {
            let left = deadline.saturating_duration_since(Instant::now());
            let slice = left / (total - index) as u32;
            if tokio::time::timeout(slice, supervisor.shutdown_all())
                .await
                .is_err()
            {
                tracing::warn!(
                    kind,
                    slice_ms = slice.as_millis(),
                    "extension shutdown budget expired; abandoning the rest of the drain"
                );
            }
        }
    }
}
