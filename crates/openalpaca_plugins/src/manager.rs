//! The plugin half of the ENABLE axis (extension design §2.2, §3, ADR-030).
//!
//! `PluginManager` **is** the plugin supervisor: it implements
//! [`ExtensionSupervisor`] rather than delegating to a new object, because it
//! is already the thing that owns the child processes, the permissions store
//! and the three registries a plugin publishes into.
//!
//! Three things changed shape here, and each closes a live hole:
//!
//! 1. **Consent and the toggle are different bits.** `enable` no longer calls
//!    `approve()` and `disable` no longer calls `deny()`, so turning an
//!    integration off no longer revokes its trust decision and turning it back
//!    on no longer silently re-grants consent for whatever the manifest
//!    currently declares (design §2.2, §6.2 #8/#9).
//! 2. **Observed state lives in the ledger, not in a status word.** The old
//!    `PluginStatus` could not distinguish "off" from "blocked", pinned a
//!    failed load at `Loading` forever, and had no notion of *which load* a
//!    handle belonged to. `ExtensionState` plus the ledger's `generation`
//!    replace it; [`legacy_status_word`] keeps `GET /v1/plugins` and the GUI's
//!    Plugins panel working until C7 deletes the route (design §4.3).
//! 3. **Every transition runs under a per-extension mutex**, held across the
//!    whole thing (design §3). That is what lets the ledger's CAS replace the
//!    contained claim-token machinery Phase 0 A2 added: two overlapping verbs
//!    serialise on the mutex, and the second one's CAS then tells it truthfully
//!    what happened.
//!
//! The disable sequence is W → T0 → T1 → T2 → T3 → T4 → T5 and the enable
//! sequence is W → E0 → E-PRE → E1 → E2 → E3 → E4 → E5; each step is a method
//! named after it below.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::{Arc, Mutex as StdMutex};

use arc_swap::ArcSwap;
use serde_json::Value;
use tokio::sync::RwLock;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tracing::{debug, error, info, warn};

use openalpaca_api::events::ServerEvent;
use openalpaca_core::agent::registry::AgentRegistry;
use openalpaca_core::agent::template::{AgentSource, AgentTemplate, AgentTemplateFrontmatter};
use openalpaca_core::bus::EventBus;
use openalpaca_core::daemon_config::DaemonConfig;
use openalpaca_core::events::SystemEvent;
use openalpaca_core::middleware::skill::{InvokeConfig, RoutingConfig, SkillFrontmatter};
use openalpaca_core::orchestrator::skill_catalog::SkillCatalog;
use openalpaca_core::tools::ToolRegistry;
use openalpaca_core::tools::extensions::{
    Consent, DeclaredContributions, DependentScan, ExtensionError, ExtensionId, ExtensionKind,
    ExtensionLedger, ExtensionRecord, ExtensionState, ExtensionSupervisor, FailureReason,
    PendingScan, Transition, UnapprovedReason, WithdrawalCause, WithdrawnSet,
};
use openalpaca_core::tools::registry::{
    CapabilityProvider, ProviderHandle, RegisteredTool, ToolBackend,
};
use openalpaca_llm::keys::KeyEncryptor;
use openalpaca_llm::{SecretStore, ToolDefinition};

use crate::bridge::{PluginAgentBridge, PluginSkillBridge, PluginToolProxy};
use crate::error::PluginError;
use crate::manifest::PluginManifest;
use crate::permission_gate::{PermissionGate, PermissionTable, SecretReference, secret_reference};
use crate::process_pool::PluginProcess;

/// How long a teardown waits for a killed plugin child to actually exit
/// before giving up and letting the kernel finish the job (design §3.2 T4).
const CHILD_EXIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// The drain bound when no `daemon.toml` is wired in — the same default
/// `[extensions] drain_timeout_secs` carries.
const DEFAULT_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How often T3 re-reads the in-flight counter while draining.
const DRAIN_POLL: std::time::Duration = std::time::Duration::from_millis(5);

// ── The legacy status vocabulary ────────────────────────────────────────

/// `ExtensionState` rendered in the words `GET /v1/plugins` and the GUI's
/// `PluginsSection` still parse (design §4.3).
///
/// The shim exists for exactly one commit range: C6 adds `/v1/extensions`, C7
/// deletes the plugins route and this function with it. Until then "tree green"
/// includes the GUI, which reads `running` / `disabled` / `waiting-approval`
/// and splits `crashed: …` / `needs-config (…)` on the first `:` or `(`.
///
/// Note the word that moves: a **denied** plugin reads `waiting-approval` here,
/// because `denied` is a *consent* word and consent now has its own field on the
/// extension row (`consent: "denied"`), not the state's. And `Orphaned` has no
/// legacy spelling — nothing could produce one before — so it renders as itself
/// and the GUI gives it the neutral tag any unknown word gets.
pub fn legacy_status_word(state: &ExtensionState) -> String {
    match state {
        ExtensionState::Enabled => "running".to_string(),
        ExtensionState::Disabled => "disabled".to_string(),
        ExtensionState::Unapproved { .. } => "waiting-approval".to_string(),
        ExtensionState::Failed {
            reason: FailureReason::NeedsConfig { missing },
            ..
        } => format!("needs-config ({})", missing.join(", ")),
        ExtensionState::Failed { detail, .. } => format!("crashed: {detail}"),
        ExtensionState::Enabling => "loading".to_string(),
        ExtensionState::Disabling => "stopped".to_string(),
        ExtensionState::Orphaned => "orphaned".to_string(),
    }
}

// ── PluginCapabilityProvider ────────────────────────────────────────────

/// A capability provider synthesized from a plugin's manifest-declared
/// virtual capabilities. In-process; no RPC cost at lookup time.
///
/// Emits the declared caps for every tool whose `author` field matches
/// `plugin:<plugin_name>` — i.e., every tool this plugin registered.
pub(crate) struct PluginCapabilityProvider {
    #[allow(dead_code)]
    plugin_name: String,
    author_prefix: String,
    virtual_caps: Vec<String>,
}

impl PluginCapabilityProvider {
    pub(crate) fn new(plugin_name: String, virtual_caps: Vec<String>) -> Self {
        let author_prefix = format!("plugin:{}", plugin_name);
        Self {
            plugin_name,
            author_prefix,
            virtual_caps,
        }
    }
}

impl CapabilityProvider for PluginCapabilityProvider {
    fn derive_capabilities(&self, tool: &RegisteredTool) -> Vec<String> {
        if tool.author == self.author_prefix {
            self.virtual_caps.clone()
        } else {
            Vec::new()
        }
    }

    fn known_capability_names(&self) -> Vec<String> {
        self.virtual_caps.clone()
    }
}

// ── PluginState ─────────────────────────────────────────────────────────

/// What the supervisor **holds** for one plugin directory.
///
/// The entry persists across `Disabled` / `Failed` / `Unapproved` — it carries
/// the manifest and the directory a later `enable` loads from — so "holds none"
/// never means "no entry" (design §3.3.1). Observed state is not here: it is the
/// ledger's, keyed by the same directory name.
pub struct PluginState {
    pub manifest: PluginManifest,
    pub process: Option<PluginProcess>,
    pub registered_tools: Vec<String>,
    pub registered_connector: Option<String>,
    pub registered_provider: Option<String>,
    pub registered_models: Vec<String>,
    pub registered_skills: Vec<String>,
    pub registered_agents: Vec<String>,
    pub plugin_dir: PathBuf,
    pub capability_provider_handle: Option<ProviderHandle>,
    /// The exit the §3.6 `try_wait` sweep observed, if it has.
    ///
    /// T4 reads it to skip `shutdown()`/`kill()`: after a reaped exit tokio's
    /// `Child::start_kill` returns `InvalidInput` and `PluginProcess::kill` logs
    /// it at `error!`, so without the skip every sweep-detected crash would be
    /// followed by a spurious *"failed to kill plugin process"* line from the
    /// reaper's T4 (design §3.2 T4).
    pub(crate) exit_status: Option<ExitStatus>,
}

impl PluginState {
    /// A handle-free entry: no child process, no capability provider, nothing
    /// registered. Every state that "holds none" is one of these.
    fn handle_free(manifest: PluginManifest, plugin_dir: PathBuf) -> Self {
        Self {
            manifest,
            process: None,
            registered_tools: Vec::new(),
            registered_connector: None,
            registered_provider: None,
            registered_models: Vec::new(),
            registered_skills: Vec::new(),
            registered_agents: Vec::new(),
            plugin_dir,
            capability_provider_handle: None,
            exit_status: None,
        }
    }

    /// Does this entry hold a **live handle** — a child process or a capability
    /// provider?
    ///
    /// The insert guard of design §2.2 keys on exactly this: replacing such an
    /// entry would orphan whatever it holds, which only a teardown can release.
    /// After E-PRE/T4 it is never true on a legitimate load, which is what makes
    /// [`PluginError::HandleHeld`] an assertion rather than a race.
    fn holds_handle(&self) -> bool {
        self.process.is_some() || self.capability_provider_handle.is_some()
    }
}

/// What a load discovered from a plugin, before any of it is published.
///
/// Discovery (E3) and publication (E4) stay separate so E4b can unwind a
/// partial load: if `register` refuses a name mid-loop, everything this attempt
/// published comes back off before the handle is torn down.
struct Discovered {
    tools: Vec<RegisteredTool>,
    connector: Option<String>,
    provider: Option<String>,
    models: Vec<String>,
    skill: Option<(String, SkillFrontmatter, Arc<PluginSkillBridge>)>,
    agent: Option<AgentTemplate>,
}

// ── PluginInfo ──────────────────────────────────────────────────────

/// Summary of a plugin returned by [`PluginManager::list_plugins`] — the shape
/// `GET /v1/plugins` serialises until C7 replaces it with the extension row.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    /// [`legacy_status_word`] of the ledger's state (design §4.3).
    pub status: String,
    pub tools: Vec<String>,
    pub connector: Option<String>,
    pub provider: Option<String>,
    pub models: Vec<String>,
    pub skills: Vec<String>,
    pub agents: Vec<String>,
}

// ── PluginManager ───────────────────────────────────────────────────────

/// Callback through which the `PluginManager` emits lifecycle events
/// (`ServerEvent::Plugin*` variants). The daemon wires this to its
/// `EventBroadcaster` so WebSocket clients (e.g. the GUI plugin panel)
/// see plugin state changes live. A plain callback keeps this crate free
/// of any dependency on the daemon's event infrastructure.
///
/// Superseded by the event bus ([`PluginManager::with_event_bus`]), which
/// carries `SystemEvent::ExtensionStateChanged`; both fire until C7 deletes the
/// legacy route, the `ServerEvent::Plugin*` producers and this type together
/// (design §7.3).
pub type PluginEventSink = Arc<dyn Fn(ServerEvent) + Send + Sync>;

/// The plugin supervisor: discovery, load/unload, consent, disposition, tool
/// registration and crash detection.
pub struct PluginManager {
    plugins: Arc<RwLock<HashMap<String, PluginState>>>,
    plugin_dir: PathBuf,
    permission_gate: PermissionGate,
    tool_registry: Arc<ToolRegistry>,
    ledger: Arc<ExtensionLedger>,
    skill_catalog: Option<Arc<SkillCatalog>>,
    agent_registry: Option<Arc<AgentRegistry>>,
    event_sink: Option<PluginEventSink>,
    /// Installed by `with_event_bus` so T5/E5 can publish
    /// `SystemEvent::ExtensionStateChanged` before C4 gives the ledger a bus of
    /// its own (design §7.3).
    bus: Option<EventBus>,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
    /// Resolves `secret_ref` config values. Absent by default — which of the two
    /// stores is the default is design §13 Q12 and is not decided here.
    secret_store: Option<Arc<dyn SecretStore>>,
    /// The per-extension mutex, **held across the whole transition** (design
    /// §3), so two toggles serialise and a toggle never interleaves with a
    /// reconcile or with the crash reaper.
    locks: StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// The crash-reaper receiver, parked until [`Self::spawn_reaper`] takes it.
    /// A test drives [`Self::reap`] by hand instead, which is what makes the
    /// *reaper superseded* scenarios deterministic rather than raced.
    reaper_rx: StdMutex<Option<UnboundedReceiver<(ExtensionId, u64)>>>,
    /// The daemon's default lane, `{local_user_id}:gui`, carried on T1 step 3's
    /// event so the `NotificationDispatcher` knows where to write the cron
    /// notice (design §7.3 step 1). Empty until `with_notice_lane`.
    notice_lane: String,
}

impl PluginManager {
    /// Create a new `PluginManager`.
    ///
    /// - `plugin_dir`: root directory containing plugin subdirectories.
    /// - `tool_registry`: shared tool registry where discovered plugin tools are registered.
    /// - `skill_catalog`: optional skill catalog for registering plugin-backed skills.
    /// - `agent_registry`: optional agent registry for registering plugin-backed agents.
    pub fn new(
        plugin_dir: PathBuf,
        tool_registry: Arc<ToolRegistry>,
        skill_catalog: Option<Arc<SkillCatalog>>,
        agent_registry: Option<Arc<AgentRegistry>>,
    ) -> Self {
        let permission_gate = PermissionGate::new(&plugin_dir);
        let ledger = Arc::clone(tool_registry.extensions());
        let (tx, rx) = unbounded_channel();
        ledger.on_crash(ExtensionKind::Plugin, tx);
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            plugin_dir,
            permission_gate,
            tool_registry,
            ledger,
            skill_catalog,
            agent_registry,
            event_sink: None,
            bus: None,
            daemon_config: None,
            secret_store: None,
            locks: StdMutex::new(HashMap::new()),
            reaper_rx: StdMutex::new(Some(rx)),
            notice_lane: String::new(),
        }
    }

    /// Attach the daemon's default lane, `{local_user_id}:gui` — where T1 step
    /// 3's cron notice is written (design §7.3 step 1).
    pub fn with_notice_lane(mut self, lane: impl Into<String>) -> Self {
        self.notice_lane = lane.into();
        self
    }

    /// Attach a lifecycle event sink. Events emitted before this is called
    /// are dropped, so attach it before [`start`](Self::start).
    pub fn with_event_sink(mut self, sink: PluginEventSink) -> Self {
        self.event_sink = Some(sink);
        self
    }

    /// Attach the daemon's event bus, so T5/E5 announce themselves as
    /// `SystemEvent::ExtensionStateChanged` (design §7.3).
    pub fn with_event_bus(mut self, bus: EventBus) -> Self {
        self.bus = Some(bus);
        self
    }

    /// Attach `daemon.toml`, whose `[extensions] drain_timeout_secs` bounds T3.
    pub fn with_daemon_config(mut self, config: Arc<ArcSwap<DaemonConfig>>) -> Self {
        self.daemon_config = Some(config);
        self
    }

    /// Attach the OS secret store, so a `secret_ref` config value can be
    /// resolved for the plugin's `initialize` (design §8, X-29).
    pub fn with_secret_store(mut self, store: Arc<dyn SecretStore>) -> Self {
        self.secret_store = Some(store);
        self
    }

    /// Start the crash reaper: one sequential task that drains `mark_failed`'s
    /// channel and runs T1 → T2 → T4 on each message it is still entitled to
    /// (design §3.6).
    pub fn spawn_reaper(self: &Arc<Self>) {
        let Some(mut rx) = self.reaper_rx.lock_or_recover().take() else {
            warn!("plugin crash reaper already started");
            return;
        };
        let me = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some((ext, generation)) = rx.recv().await {
                let Some(sup) = me.upgrade() else { break };
                sup.reap(&ext, generation).await;
            }
        });
    }

    /// Emit a legacy lifecycle event to the attached sink (no-op when absent).
    fn emit(&self, event: ServerEvent) {
        if let Some(ref sink) = self.event_sink {
            sink(event);
        }
    }

    /// The one transition announcement. `state` is always a state word rendered
    /// from the ledger.
    fn emit_state(&self, ext: &ExtensionId, state: &str, generation: u64) {
        if let Some(bus) = &self.bus {
            bus.publish(SystemEvent::ExtensionStateChanged {
                extension: ext.clone(),
                state: state.to_string(),
                generation,
                tools_changed: false,
                timestamp: chrono::Utc::now(),
            });
        }
    }

    fn emit_record(&self, ext: &ExtensionId) {
        if let Some(record) = self.ledger.record(ext) {
            self.emit_state(ext, record.state.word(), record.generation);
        }
    }

    async fn lock_for(&self, name: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.locks.lock_or_recover();
            Arc::clone(locks.entry(name.to_string()).or_default())
        };
        lock.lock_owned().await
    }

    fn drain_timeout(&self) -> std::time::Duration {
        match &self.daemon_config {
            Some(cfg) => std::time::Duration::from_secs(cfg.load().extensions.drain_timeout_secs),
            None => DEFAULT_DRAIN_TIMEOUT,
        }
    }

    // ── Scan and reconcile ───────────────────────────────────────────

    /// Scan the plugin directory and reconcile every plugin in it.
    ///
    /// Each subdirectory containing a `plugin.toml` is treated as a plugin, and
    /// the **directory name is the extension id** (design §2.2, X-3). Errors in
    /// individual plugins are logged but do not abort the scan.
    pub async fn start(&self) -> Result<(), PluginError> {
        info!(dir = %self.plugin_dir.display(), "scanning plugin directory");

        let dirs = self.plugin_directories().await?;
        let table = self.permission_gate.load_table();
        for dir in &dirs {
            self.reconcile_dir(dir, &table).await;
        }
        self.park_vanished(&dirs, &table).await;

        // §6.2a's fail-open is safe only while an unrecorded registration is
        // visible: after a scan, every plugin tool this supervisor registered
        // must have a ledger record.
        let orphans = self
            .ledger
            .audit_kind(&self.tool_registry, ExtensionKind::Plugin);
        if !orphans.is_empty() {
            error!(?orphans, "plugin tools registered with no ledger record");
        }

        let plugins = self.plugins.read().await;
        info!(count = plugins.len(), "plugin scan complete");
        Ok(())
    }

    /// Every subdirectory of the plugins root that carries a `plugin.toml`.
    async fn plugin_directories(&self) -> Result<Vec<PathBuf>, PluginError> {
        let mut entries = tokio::fs::read_dir(&self.plugin_dir).await.map_err(|e| {
            PluginError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "failed to read plugin dir {}: {}",
                    self.plugin_dir.display(),
                    e
                ),
            ))
        })?;

        let mut dirs = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() && path.join("plugin.toml").exists() {
                dirs.push(path);
            }
        }
        dirs.sort();
        Ok(dirs)
    }

    /// A plugin whose directory is gone parks as `Orphaned`, disposition and
    /// consent preserved — **never** deleted (design §5.1). Deleting the
    /// permissions entry would silently flip the extension back on at the next
    /// reconcile, and a vanished directory is very often a path difference
    /// rather than an uninstall.
    ///
    /// The vanished set is **`.permissions.toml`'s entries ∪ the in-memory
    /// map**, minus the directories that are present. The map alone would give
    /// `Orphaned` no trigger at the one moment it has one: at a daemon start
    /// the map is empty, so an entry whose directory is gone would produce no
    /// record at all — nothing for `?include_orphaned=true` to show and nothing
    /// for `DELETE /v1/extensions/plugin/{id}` to target (design §4.1
    /// *declaration gone — plugin*, §5.1 row 2).
    async fn park_vanished(
        &self,
        present: &[PathBuf],
        table: &Result<PermissionTable, PluginError>,
    ) {
        let live: std::collections::BTreeSet<String> = present
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
            .collect();

        let mut vanished: std::collections::BTreeSet<String> =
            self.plugins.read().await.keys().cloned().collect();
        if let Ok(table) = table {
            vanished.extend(table.names().into_iter().map(String::from));
        }

        for name in vanished.into_iter().filter(|name| !live.contains(name)) {
            self.orphan(&name, table).await;
        }
    }

    /// Park one vanished plugin as `Orphaned`, tearing down anything it still
    /// holds first (§4.1's *declaration gone — plugin* column).
    async fn orphan(&self, name: &str, table: &Result<PermissionTable, PluginError>) {
        let ext = ExtensionId::plugin(name.to_string());
        let _lock = self.lock_for(name).await;
        self.teardown(&ext, WithdrawalCause::DeclarationGone).await;
        self.t4(name).await;
        let bit = table.as_ref().map(|t| t.enabled(name)).unwrap_or(true);
        self.ledger.upsert(&ext, bit, ExtensionState::Orphaned);
        self.plugins.write().await.remove(name);
        let generation = self.ledger.generation(&ext).unwrap_or(0);
        self.emit_state(&ext, "orphaned", generation);
        info!(plugin = %name, "plugin directory is gone; the record is kept as orphaned");
    }

    /// Bring one plugin directory in line with its declaration, its consent and
    /// its bit.
    ///
    /// **The order is pinned** (design §2.2): manifest name check → consent →
    /// bit. The name check runs before the §6.2 #7 gate reads anything, so a
    /// directory whose manifest disagrees with it never reaches consent and
    /// never spawns.
    async fn reconcile_dir(&self, dir: &Path, table: &Result<PermissionTable, PluginError>) {
        let Some(id) = dir.file_name().and_then(|n| n.to_str()).map(String::from) else {
            warn!(dir = %dir.display(), "plugin directory has no usable name");
            return;
        };
        let manifest = match PluginManifest::from_dir(dir) {
            Ok(m) => m,
            Err(e) => {
                warn!(plugin = %id, error = %e, "plugin manifest could not be read; skipping");
                return;
            }
        };
        let ext = ExtensionId::plugin(id.clone());
        let _lock = self.lock_for(&id).await;

        // X-3: the directory is the id, and a manifest that disagrees is a
        // config error, not a rename. Two directories could otherwise share one
        // `PluginState` entry and one `.permissions.toml` entry — a second route
        // to the capability-provider leak.
        if manifest.plugin.name != id {
            // The bit is reported **as read** from the directory-keyed entry:
            // this is the one `Failed` row that reached neither W nor E0, so
            // §4's `Failed ⇒ bit == true` rule does not apply to it.
            let bit = table.as_ref().map(|t| t.enabled(&id)).unwrap_or(true);
            self.park(
                &ext,
                dir,
                manifest,
                bit,
                self.failure(
                    FailureReason::ConfigInvalid,
                    "manifest name does not match directory",
                ),
                // The directory no longer declares *this* extension, which is
                // what `DeclarationGone` names; a live load of the old manifest
                // is torn down before the row is parked.
                WithdrawalCause::DeclarationGone,
            )
            .await;
            return;
        }

        // §5.1: the store fails closed, so an unreadable one parks *every*
        // plugin — nothing loads and nothing is written.
        let table = match table {
            Ok(table) => table,
            Err(e) => {
                self.park(
                    &ext,
                    dir,
                    manifest,
                    true,
                    self.failure(FailureReason::ConfigInvalid, e.to_string()),
                    // Fail-closed (§5.1): a plugin whose disposition nobody can
                    // read does not keep running under a row that says it
                    // failed.
                    WithdrawalCause::Watcher,
                )
                .await;
                return;
            }
        };

        // §6.2 #7, read straight off §4.1's table: consent pre-empts the switch.
        //
        // The cause each branch carries is the one the §7.1 wording keys on
        // while the teardown runs: `Deny` ("denied") only where the store
        // actually reads `approved = false`; `Watcher` ("disabled") for a
        // decision that is no longer there and for the cleared bit — the
        // out-of-band read, which is what a reconcile is.
        let bit = table.enabled(&id);
        match table.approved(&id) {
            None => {
                self.park(
                    &ext,
                    dir,
                    manifest.clone(),
                    bit,
                    unapproved(UnapprovedReason::NeverSeen),
                    WithdrawalCause::Watcher,
                )
                .await;
                self.emit(ServerEvent::PluginPendingApproval {
                    plugin_id: id.clone(),
                    capabilities: manifest.capabilities.provides.clone(),
                });
            }
            Some(false) => {
                self.park(
                    &ext,
                    dir,
                    manifest,
                    bit,
                    unapproved(UnapprovedReason::Denied),
                    WithdrawalCause::Deny,
                )
                .await;
            }
            Some(true) if !bit => {
                self.park(
                    &ext,
                    dir,
                    manifest,
                    false,
                    ExtensionState::Disabled,
                    WithdrawalCause::Watcher,
                )
                .await;
            }
            Some(true) => {
                self.track(&ext, dir, manifest.clone()).await;
                let Transition::Took(generation) =
                    self.ledger.begin(&ext, ExtensionState::Enabling, None)
                else {
                    // Already `Enabled`/`Enabling` — a redundant load is a
                    // no-op, never a reload (design §3.3 E0).
                    return;
                };
                self.e_pre(&ext).await;
                self.load(&ext, dir, manifest, table, generation).await;
            }
        }
    }

    /// Park a plugin at a terminal, **handle-free** state: tear down whatever it
    /// still holds, record the declaration, then store the state.
    ///
    /// Every state parked here — `Unapproved{*}`, `Disabled`, the two
    /// `Failed{ConfigInvalid}` rows — is a *"holds none"* cell of §3.3.1's
    /// ownership matrix, so storing one **over a live handle** is precisely the
    /// hole §4.1 calls "a live hole in the approval gate": the row says the
    /// owner refused it, or turned it off, while the child keeps running with
    /// its tools registered and its capability provider installed. The scan and
    /// `reconcile`/`reconcile_all` reach here from `Enabled` whenever the store
    /// or the manifest changed under a running plugin — the watcher entrant
    /// §3.2 W lists — and C6 puts both behind routes and the CLI.
    ///
    /// The teardown runs **before** `track` replaces the cached manifest, so
    /// T2's virtual-capability tombstone is built from the manifest the live
    /// load actually registered rather than from the one that has just
    /// superseded it on disk.
    async fn park(
        &self,
        ext: &ExtensionId,
        dir: &Path,
        manifest: PluginManifest,
        bit: bool,
        state: ExtensionState,
        cause: WithdrawalCause,
    ) {
        self.teardown_held(ext, cause).await;
        self.track(ext, dir, manifest).await;
        let word = state.word().to_string();
        self.ledger.upsert(ext, bit, state);
        let generation = self.ledger.generation(ext).unwrap_or(0);
        self.emit_state(ext, &word, generation);
    }

    /// T0 → T1 → T2 → T3 → T4 on whatever the map still holds, or the residue's
    /// T1 → T2 → T4 when the record is not `Enabled`.
    ///
    /// The two shapes are `deny`'s (design §3.2 W-deny, §3.3.1): from `Enabled`
    /// the CAS flips the gate first, so the drain waits only for work that has
    /// already been refused; from a pre-reaper `Failed{Crashed}` there is no T0
    /// and no drain, because the residue exits never enter `Disabling` and the
    /// gate has refused since `mark_failed`.
    async fn teardown_held(&self, ext: &ExtensionId, cause: WithdrawalCause) {
        let holds = self
            .plugins
            .read()
            .await
            .get(&ext.name)
            .is_some_and(|s| s.holds_handle());
        if !holds {
            return;
        }
        info!(extension = %ext, cause = ?cause, "parking a plugin that still holds a handle");
        let from_enabled = matches!(self.ledger.state(ext), Some(ExtensionState::Enabled))
            && matches!(
                self.ledger
                    .begin(ext, ExtensionState::Disabling, Some(cause)),
                Transition::Took(_)
            );
        self.teardown(ext, cause).await;
        if from_enabled {
            self.t3(ext).await;
        }
        self.t4(&ext.name).await;
    }

    /// Make sure a handle-free `PluginState` exists for this directory, without
    /// disturbing one that holds a handle.
    async fn track(&self, ext: &ExtensionId, dir: &Path, manifest: PluginManifest) {
        let mut plugins = self.plugins.write().await;
        match plugins.get_mut(&ext.name) {
            Some(existing) => {
                existing.manifest = manifest;
                existing.plugin_dir = dir.to_path_buf();
            }
            None => {
                plugins.insert(
                    ext.name.clone(),
                    PluginState::handle_free(manifest, dir.to_path_buf()),
                );
            }
        }
    }

    fn failure(&self, reason: FailureReason, detail: impl Into<String>) -> ExtensionState {
        ExtensionState::Failed {
            reason,
            detail: detail.into(),
            since: chrono::Utc::now(),
        }
    }

    // ── W — PERSIST ──────────────────────────────────────────────────

    /// **Step W.** Write the disposition bit before any CAS (design §3.2/§3.3).
    ///
    /// A failed write returns `WriteFailed` — the route's `500` — and takes no
    /// transition: the plugin keeps running and the row still reads what the
    /// disk says, which is the truth. An **unreadable** store refuses the write
    /// up front and is `409 store_unreadable` instead, so no CAS is taken either
    /// way.
    ///
    /// Symmetric on both verbs: W is skipped when the bit already matches, so a
    /// redundant enable *or* disable against a read-only store is the
    /// `200`-current no-op §3.3.1 promises rather than a `500`.
    fn write_bit(&self, id: &str, enabled: bool) -> Result<(), ExtensionError> {
        let table = self.permission_gate.load_table().map_err(store_error)?;
        if table.entry(id).is_some() && table.enabled(id) == enabled {
            return Ok(());
        }
        self.permission_gate.set_enabled(id, enabled).map_err(|e| {
            error!(
                plugin = id,
                path = %self.permission_gate.permissions_path().display(),
                error = %e,
                "plugin store write failed"
            );
            store_error(e)
        })
    }

    // ── T1 — CAPABILITY WITHDRAWAL ───────────────────────────────────

    /// **T1 steps 1–2.** Tombstone each retained tool's capabilities and remove
    /// it from the registry. Returns the withdrawn count.
    ///
    /// Idempotent by construction: an absent registry entry contributes nothing,
    /// which is what makes a second pass — the reaper after a route disable, a
    /// disable after E-FAIL — a no-op. Only names the ledger *currently*
    /// attributes to this extension are touched (§10 case 13).
    ///
    /// **Step 3** is [`t1_t2`](Self::t1_t2)'s: for a plugin the withdrawn set is
    /// T1's tombstones **plus T2 step 1's virtual capabilities**, so the scan
    /// cannot run until T2 has withdrawn them (design §3.2 T1 step 3).
    fn t1(&self, ext: &ExtensionId) -> WithdrawnSet {
        let mut withdrawn = WithdrawnSet::default();
        for name in self.ledger.tool_names(ext) {
            if self.ledger.owner_of(&name).as_ref() != Some(ext) {
                continue;
            }
            if let Some(tool) = self.tool_registry.get(&name) {
                self.ledger.withdraw(ext, tool.provides_capabilities.clone());
                withdrawn.add_capabilities(tool.provides_capabilities.clone());
            }
            if self.tool_registry.remove(&name) {
                withdrawn.add_tool(name);
            }
        }
        withdrawn
    }

    /// **T1 → T2 → T1 step 3.** The order every teardown path takes.
    ///
    /// The scan runs after T2 because the withdrawn set it intersects with is
    /// the union of T1 step 1's per-tool capabilities and T2 step 1's virtual
    /// ones, which no tool carries (design §3.2 T1 step 3, T2 step 1). It fires
    /// Returns the set rather than announcing it, because `reload` publishes
    /// step 3 only once the outcome is known (§3.4.1); every other path goes
    /// through [`teardown`](Self::teardown), which announces immediately. Step 3
    /// fires only on a non-empty set, so a second, idempotent pass announces
    /// nothing: one transition, one announcement (§7.3).
    async fn t1_t2(&self, ext: &ExtensionId) -> PendingScan {
        let mut withdrawn = self.t1(ext);
        withdrawn.add_capabilities(self.t2(&ext.name).await);
        // Classify **now**, against the index T1/T2 just emptied: once a reload's
        // E4 has re-registered everything, nothing reads as lost.
        self.scan().classify(&withdrawn)
    }

    /// [`t1_t2`](Self::t1_t2) plus its step-3 publish — every path but `reload`.
    async fn teardown(&self, ext: &ExtensionId, cause: WithdrawalCause) {
        let pending = self.t1_t2(ext).await;
        self.publish_scan(ext, cause, &pending, false);
    }

    /// Publish a reload's deferred T1 step 3 (§3.4.1, §7.3): the event carries
    /// the reload's **outcome** state, and `affected_cron_skills` is emptied
    /// when it ended `Enabled` — the reload did not take the capability away.
    fn publish_reload_scan(&self, ext: &ExtensionId, pending: Option<PendingScan>) {
        let Some(pending) = pending else { return };
        let ended_enabled = self
            .ledger
            .state(ext)
            .is_some_and(|state| state.is_enabled());
        self.publish_scan(ext, WithdrawalCause::Reload, &pending, ended_enabled);
    }

    /// T1 step 3 proper: read the state the transition is in and announce.
    fn publish_scan(
        &self,
        ext: &ExtensionId,
        cause: WithdrawalCause,
        pending: &PendingScan,
        suppress_cron_notice: bool,
    ) {
        let state = self.ledger.state(ext).unwrap_or(ExtensionState::Disabling);
        self.scan()
            .announce(ext, &state, cause, pending, suppress_cron_notice);
    }

    /// **T1 step 3's** reader.
    fn scan(&self) -> DependentScan<'_> {
        DependentScan {
            registry: &self.tool_registry,
            agents: self.agent_registry.as_deref(),
            skills: self.skill_catalog.as_deref(),
            notice_lane: &self.notice_lane,
        }
    }

    // ── T2 — CONTRIBUTION WITHDRAWAL ─────────────────────────────────

    /// **T2.** Withdraw everything else a plugin contributes: its capability
    /// provider and virtual capabilities, its skills, its agent templates, and
    /// its connector/provider registrations (design §3.2 T2).
    ///
    /// Step 1's tombstone is not optional. The provider's `derive_capabilities`
    /// returns `capabilities.virtual_.provides`, a list **separate from**
    /// `capabilities.provides`, so T1's per-tool recording never sees it —
    /// without this a template naming a virtual capability would classify
    /// `unknown` (a `debug!` only) at spawn instead of `withheld`.
    ///
    /// Step 4 is the one `unload_plugin` used to decline. The connector and
    /// provider bridges are not wired into `ConnectorManager`/`LlmRouter` yet,
    /// so today clearing the registrations here **is** the whole withdrawal —
    /// and the row data is what proves it: a `disabled` row with a non-null
    /// `connector` is a T2 bug, and C3's guard test asserts it never happens.
    /// When the bridges land, `LlmRouter::deregister_provider` and a
    /// `ConnectorManager::unregister_platform` go here, beside these lines.
    /// **Whatever E4 registers, T2 deregisters, in the same supervisor.**
    ///
    /// Returns step 1's **virtual** capabilities, which T1 step 3 unions into
    /// the withdrawn set it scans with (design §7.3).
    async fn t2(&self, id: &str) -> Vec<String> {
        let mut plugins = self.plugins.write().await;
        let Some(state) = plugins.get_mut(id) else {
            return Vec::new();
        };
        let ext = ExtensionId::plugin(id.to_string());

        // 1. capability provider + the virtual capabilities it derives.
        if let Some(handle) = state.capability_provider_handle.take() {
            if self.tool_registry.remove_capability_provider(handle) {
                debug!(plugin = id, %handle, "removed plugin capability provider");
            } else {
                warn!(plugin = id, %handle, "capability provider handle not found during unload");
            }
        }
        let virtual_caps = state.manifest.capabilities.virtual_.provides.clone();
        if !virtual_caps.is_empty() {
            self.ledger.withdraw(&ext, virtual_caps.clone());
        }

        // 2. skills. The removal leaves a **tombstone** (design §10 case 5(a)):
        //    `remove` scrubs the command and alias indices, so without one a
        //    `/slash` or `invoke_skill` for a withdrawn plugin skill reads as
        //    an unknown name and gets a dump of every catalog entry.
        let skills: Vec<String> = state.registered_skills.drain(..).collect();
        if let Some(catalog) = &self.skill_catalog {
            for skill_id in &skills {
                catalog.remove_plugin_skill(skill_id, id);
                debug!(plugin = id, skill = %skill_id, "unregistered plugin skill");
            }
        }

        // 3. agent templates — same tombstone, for `spawn_subagent`.
        let agents: Vec<String> = state.registered_agents.drain(..).collect();
        if let Some(registry) = &self.agent_registry {
            for agent_id in &agents {
                registry.remove_plugin_template(agent_id, id);
                debug!(plugin = id, agent = %agent_id, "unregistered plugin agent template");
            }
        }

        // 4. connector + provider.
        state.registered_connector = None;
        state.registered_provider = None;
        state.registered_models.clear();
        state.registered_tools.clear();

        virtual_caps
    }

    // ── T3 — DRAIN ───────────────────────────────────────────────────

    /// **T3.** Wait for the plugin's in-flight work to finish, bounded by
    /// `[extensions] drain_timeout_secs`. Returns the straggler count.
    ///
    /// The counter sees **two** kinds of work: tool calls, guarded inside
    /// `ToolRegistry`, and the two out-of-process runs — a plugin skill's
    /// `skill/invoke` and a plugin agent's `spawn`/`step` loop — guarded at
    /// their in-process entry points (design §3.2 T3(b)). A drain that counted
    /// only the first could read zero while a multi-minute `skill/invoke` was in
    /// flight and kill the child under it.
    async fn t3(&self, ext: &ExtensionId) -> usize {
        let drained = tokio::time::timeout(self.drain_timeout(), async {
            while self.ledger.in_flight(ext) > 0 {
                tokio::time::sleep(DRAIN_POLL).await;
            }
        })
        .await;
        if drained.is_ok() {
            return 0;
        }
        let in_flight = self.ledger.in_flight(ext);
        warn!(
            extension = %ext,
            in_flight,
            "disable draining timed out; forcing teardown"
        );
        in_flight
    }

    // ── T4 — TEARDOWN ────────────────────────────────────────────────

    /// **T4.** Tear down whatever child the map holds for this plugin and take
    /// it out of the entry.
    ///
    /// It **asks the map, not the state**: the handle it tears down is the live
    /// one from `Enabled` or the residue of a pre-reaper `Failed{Crashed}`, and
    /// from any state that holds none it is a no-op.
    async fn t4(&self, id: &str) {
        let taken = {
            let mut plugins = self.plugins.write().await;
            plugins.get_mut(id).and_then(|state| {
                state.process.take().map(|process| {
                    // The observed exit belongs to the process just taken: a
                    // path that reused this entry afterwards would otherwise
                    // read a stale "already exited" and skip a real kill.
                    let exited = state.exit_status.take();
                    (process, exited)
                })
            })
        };
        let Some((process, exited)) = taken else {
            return;
        };
        shutdown_child(id, process, exited).await;
    }

    /// **E-PRE.** Tear down whatever the map still holds before building
    /// anything — the first step after E0's CAS on the shared load path, so no
    /// entrant can skip it (design §3.3 E-PRE).
    ///
    /// It runs the reaper's shape — T1 → T2 → T4, with **no** T3 drain, because
    /// the gate has refused since `mark_failed` — on the only state that can
    /// reach here holding anything: a pre-reaper `Failed{Crashed}`. Without it a
    /// Retry that wins the mutex before the reaper would build load N+1 on top
    /// of load N's live residue, and the reaper would then find "superseded" and
    /// do nothing.
    async fn e_pre(&self, ext: &ExtensionId) {
        let holds = self
            .plugins
            .read()
            .await
            .get(&ext.name)
            .is_some_and(|s| s.holds_handle());
        if !holds {
            return;
        }
        info!(extension = %ext, "E-PRE: tearing down a previous load's residue");
        // Cause `Crash`, for parity with the other residue exits: the state
        // this tears down is a pre-reaper `Failed{Crashed}` (design §3.3 E-PRE,
        // §3.3.1).
        self.teardown(ext, WithdrawalCause::Crash).await;
        self.t4(&ext.name).await;
    }

    // ── E1–E5 — the load path ────────────────────────────────────────

    /// E1 → E5 for one plugin, under a CAS that has already taken and an E-PRE
    /// that has already run. Commits the state it reached.
    async fn load(
        &self,
        ext: &ExtensionId,
        dir: &Path,
        manifest: PluginManifest,
        table: &PermissionTable,
        generation: u64,
    ) {
        let id = ext.name.clone();

        // ── E1 — CONSENT + DRIFT ──────────────────────────────────────
        //
        // The consent gate itself ran before this point; the **drift** check is
        // what E1 adds, and what splitting `approved` from `enabled` makes
        // possible: the capability list recorded at approval time has been
        // written since day one and never read back. If the manifest has grown
        // since, the plugin needs a fresh approve — otherwise a background
        // update could add capabilities with no re-consent (§3.3 E1, §10 #12).
        let recorded = table.recorded_capabilities(&id);
        let added: Vec<String> = manifest
            .capabilities
            .provides
            .iter()
            .filter(|cap| !recorded.contains(cap))
            .cloned()
            .collect();
        if !added.is_empty() {
            info!(plugin = %id, ?added, "plugin capabilities grew since approval");
            self.commit(ext, unapproved(UnapprovedReason::CapabilitiesGrew { added }));
            return;
        }

        // ── E2 — BRING UP ─────────────────────────────────────────────
        let stored = self.permission_gate.load_plugin_config(&id);
        let provided = match self.resolve_config(&id, stored) {
            Ok(config) => config,
            Err(missing) => {
                info!(plugin = %id, ?missing, "a plugin secret could not be resolved");
                self.emit(ServerEvent::PluginNeedsConfig {
                    plugin_id: id.clone(),
                    missing_keys: missing.clone(),
                });
                self.commit(
                    ext,
                    self.failure(
                        FailureReason::NeedsConfig { missing },
                        "a configured secret could not be resolved",
                    ),
                );
                return;
            }
        };
        let missing = manifest.missing_config_keys(&provided);
        if !missing.is_empty() {
            info!(plugin = %id, ?missing, "plugin needs configuration");
            self.emit(ServerEvent::PluginNeedsConfig {
                plugin_id: id.clone(),
                missing_keys: missing.clone(),
            });
            let detail = format!("missing configuration: {}", missing.join(", "));
            self.commit(
                ext,
                self.failure(FailureReason::NeedsConfig { missing }, detail),
            );
            return;
        }

        // `spawn` failing is the one exit with nothing to unwind.
        let process = match PluginProcess::spawn(&manifest, dir) {
            Ok(process) => process,
            Err(e) => {
                warn!(plugin = %id, error = %e, "plugin failed to spawn");
                self.commit(ext, self.failure(FailureReason::Unreachable, e.to_string()));
                return;
            }
        };

        if !manifest.plugin.mcp_compatible {
            let config_json: HashMap<String, Value> = provided
                .iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect();
            if let Err(e) = process
                .initialize(
                    &id,
                    &manifest.plugin.version,
                    &manifest.capabilities.provides,
                    config_json,
                )
                .await
            {
                // E-FAIL: the handle exists, so it goes before `Failed` is
                // committed. Without this a `Failed` row would sit over a live
                // child that nothing ever drops, and a later `disable` — legal
                // from `Failed` — would run T4 on nothing while the child
                // outlived the switch.
                warn!(plugin = %id, error = %e, "plugin initialize failed");
                shutdown_child(&id, process, None).await;
                self.commit(ext, self.failure(classify_bringup(&e), e.to_string()));
                return;
            }
        }

        // ── E3 — DISCOVER ─────────────────────────────────────────────
        let discovered = match self.discover(&id, &manifest, &process, generation).await {
            Ok(discovered) => discovered,
            Err(e) => {
                warn!(plugin = %id, error = %e, "plugin tool discovery failed");
                shutdown_child(&id, process, None).await; // E-FAIL
                self.commit(ext, self.failure(classify_bringup(&e), e.to_string()));
                return;
            }
        };

        // ── E4 + E4b + E5 ─────────────────────────────────────────────
        match self
            .publish(ext, dir, manifest, process, discovered)
            .await
        {
            Ok(tools) => {
                self.ledger.restore(ext);
                self.ledger.record_tools(ext, tools.clone());
                self.commit(ext, ExtensionState::Enabled);
                self.emit(ServerEvent::PluginLoaded {
                    plugin_id: id.clone(),
                    tools,
                });
                info!(plugin = %id, generation, "plugin loaded successfully");
            }
            Err(e) => {
                error!(plugin = %id, error = %e, "plugin publication failed");
                self.commit(ext, self.failure(FailureReason::Unreachable, e.to_string()));
            }
        }
    }

    /// **E3.** Everything the plugin contributes, discovered but not published.
    ///
    /// `tools/list` failing is fatal to the load; the optional `*/info` probes
    /// are not — a plugin that declares a contribution it cannot describe loses
    /// that contribution, not its tools, exactly as before.
    async fn discover(
        &self,
        id: &str,
        manifest: &PluginManifest,
        process: &PluginProcess,
        generation: u64,
    ) -> Result<Discovered, PluginError> {
        let mut tools = Vec::new();
        if manifest.types.tools {
            tools = self
                .discover_tools(id, manifest, process, generation)
                .await?;
        }

        let mut connector = None;
        if manifest.types.connector {
            match process
                .channel
                .call("connector/info", Value::Object(Default::default()))
                .await
            {
                Ok(info) => {
                    let platform = info
                        .get("platform")
                        .and_then(|p| p.as_str())
                        .unwrap_or(id)
                        .to_string();
                    info!(plugin = %id, platform = %platform, "discovered connector");
                    connector = Some(platform);
                }
                Err(e) => warn!(plugin = %id, error = %e, "connector/info failed"),
            }
        }

        let mut provider = None;
        let mut models = Vec::new();
        if manifest.types.provider {
            match process
                .channel
                .call("provider/info", Value::Object(Default::default()))
                .await
            {
                Ok(info) => {
                    let provider_name = info
                        .get("provider_name")
                        .and_then(|p| p.as_str())
                        .unwrap_or(id)
                        .to_string();
                    if let Some(found) = info.get("models").and_then(|m| m.as_array()) {
                        for model in found {
                            if let Some(model_id) = model.get("id").and_then(|v| v.as_str()) {
                                models.push(model_id.to_string());
                            }
                        }
                    }
                    info!(
                        plugin = %id,
                        provider = %provider_name,
                        models = models.len(),
                        "discovered provider"
                    );
                    provider = Some(provider_name);
                }
                Err(e) => warn!(plugin = %id, error = %e, "provider/info failed"),
            }
        }

        let mut skill = None;
        if manifest.types.skill {
            match process
                .channel
                .call("skill/info", Value::Object(Default::default()))
                .await
            {
                Ok(info) => {
                    let skill_id = info
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or(id)
                        .to_string();
                    let bridge = Arc::new(PluginSkillBridge::new(
                        id.to_string(),
                        skill_id.clone(),
                        process.channel.clone(),
                        generation,
                        Arc::clone(&self.ledger),
                    ));
                    let frontmatter = build_skill_frontmatter_from_info(&info, id);
                    info!(plugin = %id, skill = %skill_id, "discovered plugin skill");
                    skill = Some((skill_id, frontmatter, bridge));
                }
                Err(e) => warn!(plugin = %id, error = %e, "skill/info failed"),
            }
        }

        let mut agent = None;
        if manifest.types.agent {
            match process
                .channel
                .call("agent/info", Value::Object(Default::default()))
                .await
            {
                Ok(info) => {
                    let agent_id = info
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or(id)
                        .to_string();
                    let bridge = Arc::new(PluginAgentBridge::new(
                        id.to_string(),
                        agent_id.clone(),
                        process.channel.clone(),
                        generation,
                        Arc::clone(&self.ledger),
                    ));
                    info!(plugin = %id, agent = %agent_id, "discovered plugin agent");
                    agent = Some(build_agent_template_from_info(&info, id, bridge));
                }
                Err(e) => warn!(plugin = %id, error = %e, "agent/info failed"),
            }
        }

        Ok(Discovered {
            tools,
            connector,
            provider,
            models,
            skill,
            agent,
        })
    }

    /// **E4 + E4b + E5's map write.** Publish a load's discoveries and install
    /// the entry that owns them.
    ///
    /// Registration is `replace` — remove-then-register — because `register`
    /// overwrites `tools` but *appends* to `capability_index` with no dedupe,
    /// and only `remove` scrubs: an enable/disable/enable cycle that skipped the
    /// remove would leak duplicate index edges that every read survives and no
    /// test would fail on (design §3.3 E4).
    ///
    /// **E4b:** any failure after the first registration takes back everything
    /// this attempt published and tears the child down before returning, so the
    /// registry never holds a half-loaded extension.
    async fn publish(
        &self,
        ext: &ExtensionId,
        dir: &Path,
        manifest: PluginManifest,
        process: PluginProcess,
        discovered: Discovered,
    ) -> Result<Vec<String>, PluginError> {
        let id = ext.name.clone();
        let mut registered_tools: Vec<String> = Vec::with_capacity(discovered.tools.len());

        for tool in discovered.tools {
            let tool_name = tool.definition.name.clone();
            // §10 case 13: a name is blocked only by a **live** incumbent.
            if let Some(incumbent) = self.ledger.owner_of(&tool_name)
                && &incumbent != ext
                && self.ledger.state(&incumbent).is_some_and(|s| s.is_enabled())
                && !self.ledger.is_server_withdrawn(&incumbent, &tool_name)
            {
                warn!(
                    extension = %ext,
                    tool = %tool_name,
                    incumbent = %incumbent,
                    "tool name collision — skipping"
                );
                continue;
            }
            if let Err(e) = self.tool_registry.replace(tool) {
                // E4b — unwind everything this attempt published.
                warn!(plugin = %id, tool = %tool_name, error = %e, "plugin tool registration refused");
                for name in &registered_tools {
                    self.tool_registry.remove(name);
                }
                shutdown_child(&id, process, None).await;
                return Err(PluginError::InvalidManifest(format!(
                    "tool '{tool_name}': {e}"
                )));
            }
            debug!(plugin = %id, tool = %tool_name, "registered plugin tool");
            registered_tools.push(tool_name);
        }

        let mut registered_skills = Vec::new();
        if let (Some(catalog), Some((skill_id, frontmatter, bridge))) =
            (self.skill_catalog.as_ref(), discovered.skill)
        {
            // The catalog lowercases the id at insert (§6.2 #14), so the id this
            // load records — and T2 removes — is the lowercased one.
            let skill_id = skill_id.to_lowercase();
            catalog.register_plugin_skill(skill_id.clone(), frontmatter, bridge, id.clone());
            info!(plugin = %id, skill = %skill_id, "registered plugin skill");
            registered_skills.push(skill_id);
        }

        let mut registered_agents = Vec::new();
        if let (Some(registry), Some(template)) = (self.agent_registry.as_ref(), discovered.agent) {
            let agent_id = template.frontmatter.id.clone();
            registry.register_template(template);
            info!(plugin = %id, agent = %agent_id, "registered plugin agent template");
            registered_agents.push(agent_id);
        }

        let capability_provider_handle = if manifest.capabilities.virtual_.provides.is_empty() {
            None
        } else {
            let provider = PluginCapabilityProvider::new(
                id.clone(),
                manifest.capabilities.virtual_.provides.clone(),
            );
            let handle = self
                .tool_registry
                .register_capability_provider(Arc::new(provider));
            info!(
                plugin = %id,
                %handle,
                cap_count = manifest.capabilities.virtual_.provides.len(),
                "registered plugin capability provider"
            );
            Some(handle)
        };

        // **E5's map write, guarded** (design §2.2). Refusing to replace an
        // entry that still holds a handle is what makes E-PRE/T4 an assertion
        // rather than a hope: after either, no live handle can be there.
        let mut plugins = self.plugins.write().await;
        if plugins.get(&id).is_some_and(|s| s.holds_handle()) {
            drop(plugins);
            error!(
                plugin = %id,
                "refusing to replace a plugin entry that still holds a live handle"
            );
            for name in &registered_tools {
                self.tool_registry.remove(name);
            }
            if let Some(handle) = capability_provider_handle {
                self.tool_registry.remove_capability_provider(handle);
            }
            if let Some(catalog) = &self.skill_catalog {
                for skill_id in &registered_skills {
                    catalog.remove(skill_id);
                }
            }
            if let Some(registry) = &self.agent_registry {
                for agent_id in &registered_agents {
                    registry.remove_template(agent_id);
                }
            }
            shutdown_child(&id, process, None).await;
            return Err(PluginError::HandleHeld(id));
        }

        plugins.insert(
            id.clone(),
            PluginState {
                process: Some(process),
                registered_tools: registered_tools.clone(),
                registered_connector: discovered.connector,
                registered_provider: discovered.provider,
                registered_models: discovered.models,
                registered_skills,
                registered_agents,
                capability_provider_handle,
                ..PluginState::handle_free(manifest, dir.to_path_buf())
            },
        );
        Ok(registered_tools)
    }

    /// Discover tools from a running plugin via `tools/list` and build a
    /// [`RegisteredTool`] for each.
    ///
    /// The registry name and the `author` are both built from the **directory**
    /// id, not from `manifest.plugin.name`, so `RegisteredTool::extension_id()`
    /// — which derives the id by stripping `plugin:` off the author — and the
    /// ledger key are the same string by construction (design §2.2, X-3).
    async fn discover_tools(
        &self,
        id: &str,
        manifest: &PluginManifest,
        process: &PluginProcess,
        generation: u64,
    ) -> Result<Vec<RegisteredTool>, PluginError> {
        let result = process
            .channel
            .call("tools/list", serde_json::json!({}))
            .await?;

        let tools_array = result
            .get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut discovered = Vec::with_capacity(tools_array.len());
        for tool_val in &tools_array {
            let bare_name = tool_val
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            if bare_name.is_empty() {
                warn!(plugin = id, "skipping tool with empty name");
                continue;
            }

            let description = tool_val
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            let input_schema = tool_val
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"type": "object"}));
            let namespaced_name = format!("{id}::{bare_name}");

            let proxy = PluginToolProxy::new(
                id.to_string(),
                process.channel.clone(),
                generation,
                Arc::clone(&self.ledger),
            );

            discovered.push(RegisteredTool {
                definition: ToolDefinition {
                    name: namespaced_name.clone(),
                    description,
                    parameters: input_schema,
                    strict: None,
                    input_examples: None,
                },
                backend: ToolBackend::Plugin(Arc::new(proxy)),
                provides_capabilities: manifest.capabilities.provides.clone(),
                exempt_from_timeout: false,
                annotations: None,
                version: manifest.plugin.version.clone(),
                author: format!("plugin:{id}"),
                created_at: chrono::Utc::now(),
            });
            debug!(plugin = id, tool = %namespaced_name, "discovered plugin tool");
        }

        info!(plugin = id, count = discovered.len(), "discovered tools");
        Ok(discovered)
    }

    /// **E5 / T5.** Leave the transient state and announce it.
    fn commit(&self, ext: &ExtensionId, state: ExtensionState) {
        let word = state.word().to_string();
        if !self.ledger.commit(ext, state) {
            debug!(extension = %ext, "commit found no transient state to leave");
        }
        let generation = self.ledger.generation(ext).unwrap_or(0);
        self.emit_state(ext, &word, generation);
    }

    // ── §3.6 — crash detection ───────────────────────────────────────

    /// **The read-side sweep.** `try_wait` every `Enabled` plugin's child and
    /// `mark_failed` the ones that have exited (design §3.6 item 3).
    ///
    /// `try_wait` takes `&mut self`, so the sweep takes the **write** lock — one
    /// non-blocking syscall per plugin, microseconds — and **nothing `.await`s
    /// under it**: `mark_failed` is a CAS plus an unbounded-channel `send`. That
    /// is the whole "monitor": the row is correct whenever someone looks at it.
    async fn sweep(&self) {
        let mut plugins = self.plugins.write().await;
        for (id, state) in plugins.iter_mut() {
            if state.exit_status.is_some() {
                continue;
            }
            let ext = ExtensionId::plugin(id.clone());
            let Some(generation) = self.ledger.generation(&ext) else {
                continue;
            };
            if !self.ledger.state(&ext).is_some_and(|s| s.is_enabled()) {
                continue;
            }
            let Some(process) = state.process.as_mut() else {
                continue;
            };
            let Some(status) = process.try_wait() else {
                continue;
            };
            state.exit_status = Some(status);
            // This is the live process, by construction, so the record's own
            // current generation is the right one to mark.
            self.ledger.mark_failed(
                &ext,
                generation,
                FailureReason::Crashed,
                format!("plugin process exited ({status})"),
            );
        }
    }

    /// One reaper message. `mark_failed` already set the state; the reaper
    /// **never writes state** and never takes T0 — it enters at T1.
    ///
    /// It re-reads the record under the per-extension mutex and proceeds only if
    /// the row still reads `Failed{Crashed}` at the generation the message
    /// carries. The mutex prevents *interleaving*, not *reordering*: a Retry
    /// that took the mutex first has already built load N+1, and an
    /// unconditional teardown here would unpublish its tools and kill its live
    /// process while leaving the row `Enabled`.
    pub async fn reap(&self, ext: &ExtensionId, generation: u64) {
        // The crash's announcement is `mark_failed`'s own, published over the
        // ledger's bus at the instant the state changed (design §3.6) — not the
        // reaper's on dequeue, which C3 used only because the ledger had no bus
        // until C4. A superseded reap still announces the crash that *did*
        // happen, for the same reason it did then.
        let _lock = self.lock_for(&ext.name).await;
        let entitled = self.ledger.record(ext).is_some_and(|r| {
            matches!(
                r.state,
                ExtensionState::Failed {
                    reason: FailureReason::Crashed,
                    ..
                }
            ) && r.generation == generation
        });
        if !entitled {
            debug!(extension = %ext, generation, "crash reap superseded");
            return;
        }

        self.teardown(ext, WithdrawalCause::Crash).await;
        self.t4(&ext.name).await;
    }

    // ── Consent verbs ────────────────────────────────────────────────

    /// **approve.** Record consent against the manifest's **current**
    /// capability list, then load only from `Unapproved` with the bit set
    /// (design §8, §4.1).
    ///
    /// From `Disabled` it records and stays `Disabled`; from `Enabled` or
    /// `Failed{*}` it re-records and returns the row unchanged — never a load
    /// (Retry on a `Failed` row is `enable`). Approving does not set `enabled`.
    pub async fn approve_plugin(&self, name: &str) -> Result<ExtensionRecord, ExtensionError> {
        let ext = ExtensionId::plugin(name.to_string());
        self.guard_orphan(&ext)?;
        let _lock = self.lock_for(name).await;
        let (dir, manifest) = self.declaration(&ext).await?;

        // W first: the consent decision reaches disk before anything starts.
        self.permission_gate.load_table().map_err(store_error)?;
        self.permission_gate
            .approve(name, &manifest.capabilities.provides)
            .map_err(store_error)?;

        if !matches!(
            self.ledger.state(&ext),
            Some(ExtensionState::Unapproved { .. })
        ) {
            return self.row(&ext).await;
        }

        let table = self.permission_gate.load_table().map_err(store_error)?;
        if !table.enabled(name) {
            self.ledger.upsert(&ext, false, ExtensionState::Disabled);
            self.emit_record(&ext);
            return self.row(&ext).await;
        }

        let Transition::Took(generation) = self.ledger.begin(&ext, ExtensionState::Enabling, None)
        else {
            return self.row(&ext).await;
        };
        self.e_pre(&ext).await;
        self.load(&ext, &dir, manifest, &table, generation).await;
        self.row(&ext).await
    }

    /// **deny.** Record the refusal, then perform the full unload (design §4.1,
    /// §6.2 #8).
    ///
    /// Write-first: `approved = false` reaches `.permissions.toml` before
    /// anything is torn down, so a crash between the two can only leave a denied
    /// plugin still loaded until the next boot — never a plugin the owner
    /// refused that boots as approved. `enabled` is deliberately untouched, so a
    /// later approve restores the owner's last toggle position.
    ///
    /// Today's behaviour was the hole this closes: `deny_plugin` wrote the
    /// denial, relabelled the plugin, and left the child running with its tools,
    /// skill and agent template registered until the next restart.
    pub async fn deny_plugin(&self, name: &str) -> Result<ExtensionRecord, ExtensionError> {
        let ext = ExtensionId::plugin(name.to_string());
        self.guard_orphan(&ext)?;
        self.known(&ext).await?;
        let _lock = self.lock_for(name).await;

        // W-deny.
        self.permission_gate.load_table().map_err(store_error)?;
        self.permission_gate.deny(name).map_err(store_error)?;

        match self.ledger.state(&ext) {
            // From `Enabled`: the full T0–T4.
            Some(ExtensionState::Enabled) => {
                if let Transition::Took(_) = self.ledger.begin(
                    &ext,
                    ExtensionState::Disabling,
                    Some(WithdrawalCause::Deny),
                ) {
                    self.teardown(&ext, WithdrawalCause::Deny).await;
                    self.t3(&ext).await;
                    self.t4(name).await;
                }
            }
            // From a pre-reaper `Failed{Crashed}`: T1 → T2 → T4 with cause
            // `Crash` and **no** T0 — the residue exits never enter `Disabling`
            // (design §3.2 W-deny, §3.3.1).
            Some(ExtensionState::Failed { .. }) => {
                self.teardown(&ext, WithdrawalCause::Crash).await;
                self.t4(name).await;
            }
            _ => {}
        }

        // T5-deny stores the target state **unconditionally** under the mutex;
        // it is not a CAS from `Disabling`.
        self.ledger
            .store_state(&ext, unapproved(UnapprovedReason::Denied));
        self.emit_record(&ext);
        self.emit(ServerEvent::PluginUnloaded {
            plugin_id: name.to_string(),
        });
        self.emit(ServerEvent::PluginDisabled {
            plugin_id: name.to_string(),
            reason: "denied by user".to_string(),
        });
        info!(plugin = name, "plugin denied");
        self.row(&ext).await
    }

    // ── Reads ────────────────────────────────────────────────────────

    /// List all tracked plugins with the legacy metadata `GET /v1/plugins`
    /// serialises.
    ///
    /// It runs the **same `try_wait` sweep** `list()` does: between C3 and C7
    /// this is the only route to the sweep, because the plugins panel still
    /// reads the legacy endpoint (design §3.6 item 3).
    pub async fn list_plugins(&self) -> Vec<PluginInfo> {
        self.sweep().await;
        let plugins = self.plugins.read().await;
        let mut rows: Vec<PluginInfo> = plugins
            .iter()
            .map(|(name, state)| {
                let status = self
                    .ledger
                    .state(&ExtensionId::plugin(name.clone()))
                    .map(|s| legacy_status_word(&s))
                    .unwrap_or_else(|| "loading".to_string());
                PluginInfo {
                    name: name.clone(),
                    version: state.manifest.plugin.version.clone(),
                    status,
                    tools: state.registered_tools.clone(),
                    connector: state.registered_connector.clone(),
                    provider: state.registered_provider.clone(),
                    models: state.registered_models.clone(),
                    skills: state.registered_skills.clone(),
                    agents: state.registered_agents.clone(),
                }
            })
            .collect();
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        rows
    }

    /// The extension row for one plugin: the ledger's record plus the row data
    /// only the supervisor holds — consent, the manifest's declarations, the
    /// live contributions (design §8).
    async fn row(&self, ext: &ExtensionId) -> Result<ExtensionRecord, ExtensionError> {
        let mut record = self
            .ledger
            .record(ext)
            .ok_or_else(|| ExtensionError::NotFound(ext.clone()))?;
        let table = self.permission_gate.load_table();
        record.disposition_readable = table.is_ok();
        // §4: on a row whose store is unreadable *neither* fact is readable, so
        // consent is `None` rather than the affirmative word `pending` — which
        // would report a decision nobody can see as "no decision".
        record.consent = table
            .as_ref()
            .ok()
            .map(|t| Consent::from_approved(t.approved(&ext.name)));
        if let ExtensionState::Failed {
            reason: FailureReason::NeedsConfig { missing },
            ..
        } = &record.state
        {
            record.missing_config_keys = missing.clone();
        }
        if let Some(state) = self.plugins.read().await.get(&ext.name) {
            record.declared = Some(DeclaredContributions {
                capabilities: state.manifest.capabilities.provides.clone(),
                virtual_capabilities: state.manifest.capabilities.virtual_.provides.clone(),
                types: declared_types(&state.manifest),
            });
            record.skills = state.registered_skills.clone();
            record.agents = state.registered_agents.clone();
            record.connector = state.registered_connector.clone();
            record.provider = state.registered_provider.clone();
        }
        Ok(record)
    }

    /// The plugin's declaration — its directory and manifest — **re-read from
    /// disk** under the verb's mutex hold, or `NotFound`.
    ///
    /// A verb must not build from the manifest the last scan happened to cache:
    /// the E1 drift check compares the manifest against the list recorded at
    /// approval, and a plugin that updated itself between the scan and the
    /// toggle would then be compared against its *old* declaration and start
    /// with capabilities nobody consented to. (The MCP supervisor re-reads its
    /// store under the same hold for the same reason.)
    ///
    /// A `plugin.toml` that no longer parses, or whose name no longer matches
    /// its directory, leaves the cached declaration in place with a `warn!`:
    /// re-identifying a plugin is the scan's job (§2.2), never a side effect of
    /// a toggle.
    async fn declaration(
        &self,
        ext: &ExtensionId,
    ) -> Result<(PathBuf, PluginManifest), ExtensionError> {
        let cached = self
            .plugins
            .read()
            .await
            .get(&ext.name)
            .map(|s| (s.plugin_dir.clone(), s.manifest.clone()))
            .ok_or_else(|| ExtensionError::NotFound(ext.clone()))?;

        let (dir, manifest) = cached;
        match PluginManifest::from_dir(&dir) {
            Ok(fresh) if fresh.plugin.name == ext.name => {
                if let Some(state) = self.plugins.write().await.get_mut(&ext.name) {
                    state.manifest = fresh.clone();
                }
                Ok((dir, fresh))
            }
            Ok(_) => {
                warn!(
                    extension = %ext,
                    "the manifest on disk no longer matches its directory; keeping the last-good declaration"
                );
                Ok((dir, manifest))
            }
            Err(e) => {
                warn!(
                    extension = %ext,
                    error = %e,
                    "the manifest on disk does not parse; keeping the last-good declaration"
                );
                Ok((dir, manifest))
            }
        }
    }

    async fn known(&self, ext: &ExtensionId) -> Result<(), ExtensionError> {
        if self.plugins.read().await.contains_key(&ext.name) || self.ledger.record(ext).is_some() {
            Ok(())
        } else {
            Err(ExtensionError::NotFound(ext.clone()))
        }
    }

    fn guard_kind(&self, id: &ExtensionId) -> Result<(), ExtensionError> {
        if id.kind == ExtensionKind::Plugin {
            Ok(())
        } else {
            Err(ExtensionError::UnsupportedForKind)
        }
    }

    /// §4.1's `Orphaned` row: every verb is a `409`, and `DELETE` (C6) is the
    /// only one that applies.
    ///
    /// It is checked before the declaration lookup, which would otherwise
    /// answer `404` — an orphan's `PluginState` is gone, but its *record*, its
    /// disposition and its consent are deliberately kept (design §5.1).
    fn guard_orphan(&self, id: &ExtensionId) -> Result<(), ExtensionError> {
        match self.ledger.state(id) {
            Some(ExtensionState::Orphaned) => Err(ExtensionError::NotOrphaned),
            _ => Ok(()),
        }
    }

    // ── Config ───────────────────────────────────────────────────────

    /// Set a configuration key for a plugin, and retry the load if the plugin
    /// was parked on a missing key.
    ///
    /// A key the manifest marks `sensitive` is **refused** here (design §8,
    /// X-29): its value must never reach `plugins/.config/<name>.toml`, and
    /// which of the two secret stores holds it instead is the caller's decision
    /// — §13 Q12 has not fixed a default, so nothing picks one by omission. Use
    /// [`set_plugin_secret`](Self::set_plugin_secret).
    pub async fn set_plugin_config(
        &self,
        name: &str,
        key: &str,
        value: toml::Value,
    ) -> Result<(), PluginError> {
        if self.is_sensitive(name, key).await {
            return Err(PluginError::PermissionDenied(format!(
                "config key '{key}' of plugin '{name}' is declared sensitive; \
                 store it as a secret reference, not in the plugin's TOML"
            )));
        }
        self.permission_gate.set_plugin_config(name, key, value)?;
        self.retry_after_config(name).await;
        Ok(())
    }

    /// Store a sensitive configuration value, keeping only a **reference** in
    /// the plugin's TOML (design §8, X-29).
    ///
    /// `store` says where the value itself goes — the OS keychain
    /// (`secret_ref`) or AES-256-GCM under `state/.master_key`
    /// (`secret_encrypted`). There is deliberately no default: design §13 Q12
    /// owes that decision and this commit does not make it.
    pub async fn set_plugin_secret(
        &self,
        name: &str,
        key: &str,
        value: &str,
        store: SecretStorage,
    ) -> Result<(), PluginError> {
        let reference = match store {
            SecretStorage::Keychain => {
                let Some(secret_store) = self.secret_store.as_ref() else {
                    return Err(PluginError::Unavailable(
                        "no OS secret store is available for secret_ref storage".to_string(),
                    ));
                };
                let secret_ref = format!("openalpaca-plugin-{name}-{key}");
                secret_store
                    .set(&secret_ref, value)
                    .map_err(PluginError::PermissionDenied)?;
                SecretReference::Keychain(secret_ref)
            }
            SecretStorage::Encrypted => {
                SecretReference::Encrypted(encryptor()?.encrypt(value).map_err(|e| {
                    PluginError::PermissionDenied(format!("could not encrypt the value: {e}"))
                })?)
            }
        };
        self.permission_gate
            .set_plugin_secret_reference(name, key, reference)?;
        self.retry_after_config(name).await;
        Ok(())
    }

    /// The plugin's configuration as a reader should see it: every secret
    /// reference replaced by `<redacted>` (design §8).
    pub fn plugin_config_redacted(&self, name: &str) -> HashMap<String, toml::Value> {
        self.permission_gate.redacted_plugin_config(name)
    }

    async fn is_sensitive(&self, name: &str, key: &str) -> bool {
        self.plugins
            .read()
            .await
            .get(name)
            .and_then(|s| s.manifest.config.get(key))
            .is_some_and(|field| field.sensitive)
    }

    /// A stored config map with its secret references resolved to the values
    /// the plugin's `initialize` needs.
    ///
    /// A reference that cannot be resolved is reported as a **missing** key
    /// rather than passed through, so a plugin never receives a ciphertext or a
    /// keychain handle where it expected a token.
    fn resolve_config(
        &self,
        name: &str,
        stored: HashMap<String, toml::Value>,
    ) -> Result<HashMap<String, toml::Value>, Vec<String>> {
        let mut resolved = HashMap::with_capacity(stored.len());
        let mut unresolved = Vec::new();
        for (key, value) in stored {
            let Some(reference) = secret_reference(&value) else {
                resolved.insert(key, value);
                continue;
            };
            match self.resolve_secret(&reference) {
                Some(plain) => {
                    resolved.insert(key, toml::Value::String(plain));
                }
                None => {
                    warn!(
                        plugin = name,
                        key, "a sensitive config value could not be resolved"
                    );
                    unresolved.push(key);
                }
            }
        }
        if unresolved.is_empty() {
            Ok(resolved)
        } else {
            unresolved.sort();
            Err(unresolved)
        }
    }

    fn resolve_secret(&self, reference: &SecretReference) -> Option<String> {
        match reference {
            SecretReference::Keychain(key) => self.secret_store.as_ref()?.get(key).ok().flatten(),
            SecretReference::Encrypted(ciphertext) => encryptor().ok()?.decrypt(ciphertext).ok(),
        }
    }

    async fn retry_after_config(&self, name: &str) {
        let ext = ExtensionId::plugin(name.to_string());
        let parked = matches!(
            self.ledger.state(&ext),
            Some(ExtensionState::Failed {
                reason: FailureReason::NeedsConfig { .. },
                ..
            })
        );
        if parked {
            info!(plugin = name, "config updated, retrying load");
            let _ = self.reconcile(&ext).await;
        }
    }

    // ── bridge accessors ─────────────────────────────────────────────

    /// Get the [`PluginConnector`](crate::bridge::PluginConnector) for a loaded
    /// connector plugin, or `None` if the plugin is not loaded or does not
    /// provide a connector.
    pub async fn get_plugin_connector(&self, name: &str) -> Option<crate::bridge::PluginConnector> {
        let plugins = self.plugins.read().await;
        let state = plugins.get(name)?;
        let platform = state.registered_connector.as_ref()?.clone();
        let channel = state.process.as_ref()?.channel.clone();
        Some(crate::bridge::PluginConnector::new(
            name.to_string(),
            platform,
            channel,
        ))
    }

    /// Get the [`PluginLlmProvider`](crate::bridge::PluginLlmProvider) for a
    /// loaded provider plugin, along with its discovered model IDs. Returns
    /// `None` if the plugin is not loaded or does not provide an LLM provider.
    pub async fn get_plugin_provider(
        &self,
        name: &str,
    ) -> Option<(crate::bridge::PluginLlmProvider, Vec<String>)> {
        let plugins = self.plugins.read().await;
        let state = plugins.get(name)?;
        let provider_name = state.registered_provider.as_ref()?.clone();
        let channel = state.process.as_ref()?.channel.clone();
        let models = state.registered_models.clone();
        let provider = crate::bridge::PluginLlmProvider::new(
            name.to_string(),
            provider_name,
            true,  // supports_tools (TODO: get from provider/info)
            false, // supports_streaming (TODO: get from provider/info)
            channel,
        );
        Some((provider, models))
    }
}

/// Where a sensitive plugin config value is kept. **Not defaulted** — design
/// §13 Q12 owes that decision (§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretStorage {
    Keychain,
    Encrypted,
}

// ── The trait ───────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl ExtensionSupervisor for PluginManager {
    /// W (`enabled = true`) then E0 → E-PRE → E1–E5. `200` even when bring-up
    /// fails: the write succeeded and the intent is durable; the outcome is a
    /// separate fact in the body.
    ///
    /// It **no longer approves**. `enable` used to call
    /// `permission_gate.approve(name, current_manifest_capabilities)`, so
    /// turning an integration back on silently re-granted consent for whatever
    /// the manifest declared by then (design §2.2, §6.2 #9).
    async fn enable(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError> {
        self.guard_kind(id)?;
        self.guard_orphan(id)?;
        let _lock = self.lock_for(&id.name).await;
        let (dir, manifest) = self.declaration(id).await?;

        self.write_bit(&id.name, true)?;
        self.ledger.set_disposition(id, true);

        // §4.1's `Unapproved` × `enable` cell: the bit goes to `true` and the
        // row stays `Unapproved` with its reason — **never a load**. Consent
        // pre-empts the switch, so `approve` is the verb that starts it.
        if let Some(ExtensionState::Unapproved { reason }) = self.ledger.state(id) {
            self.ledger
                .upsert(id, true, ExtensionState::Unapproved { reason });
            self.emit_record(id);
            return self.row(id).await;
        }

        let table = self.permission_gate.load_table().map_err(store_error)?;
        let Transition::Took(generation) = self.ledger.begin(id, ExtensionState::Enabling, None)
        else {
            // Enable on `Enabled` is a CAS failure returning the current row —
            // never a reload. A redundant load would overwrite the map entry
            // with a fresh one whose `capability_provider_handle` is `None`, so
            // `remove_capability_provider` would never be called for the old
            // handle: every redundant enable permanently leaked a duplicate
            // provider (design §3.3 E0).
            return self.row(id).await;
        };
        self.e_pre(id).await;
        self.load(id, &dir, manifest, &table, generation).await;
        self.row(id).await
    }

    /// W (`enabled = false`) then T0–T5.
    ///
    /// It **no longer denies**. `disable` used to call
    /// `permission_gate.deny(name)`, so an integration toggle revoked the
    /// plugin's trust decision (design §6.2 #9). On an `Unapproved` plugin the
    /// CAS refuses, nothing is torn down, and the decision-less entry W just
    /// wrote is what makes the pre-set bit survive a restart.
    async fn disable(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError> {
        self.guard_kind(id)?;
        self.guard_orphan(id)?;
        self.known(id).await?;
        let _lock = self.lock_for(&id.name).await;

        self.write_bit(&id.name, false)?;
        if self.ledger.record(id).is_none() {
            self.ledger.upsert(id, false, ExtensionState::Disabled);
            self.emit_state(id, "disabled", 0);
            return self.row(id).await;
        }
        self.ledger.set_disposition(id, false);

        let Transition::Took(_) = self.ledger.begin(
            id,
            ExtensionState::Disabling,
            Some(WithdrawalCause::Disable),
        ) else {
            // `Disabled`/`Unapproved`/`Orphaned` × disable: the bit is the whole
            // transition and the row already says the rest.
            self.emit_record(id);
            return self.row(id).await;
        };

        self.teardown(id, WithdrawalCause::Disable).await;
        let stragglers = self.t3(id).await;
        self.t4(&id.name).await;
        self.commit(id, ExtensionState::Disabled);
        // The two legacy producers this teardown replaces: `unload_plugin`
        // emitted `PluginUnloaded` on every unload and the verb emitted
        // `PluginDisabled`. Both keep firing until C7 deletes the route (§7.3).
        self.emit(ServerEvent::PluginUnloaded {
            plugin_id: id.name.clone(),
        });
        self.emit(ServerEvent::PluginDisabled {
            plugin_id: id.name.clone(),
            reason: "disabled by user".to_string(),
        });
        info!(extension = %id, "plugin disabled");

        let mut record = self.row(id).await?;
        if stragglers > 0 {
            // §8: a `200` still means the teardown happened; the warning says
            // it happened over N calls that had not finished draining.
            record
                .warnings
                .push(format!("torn down with {stragglers} call(s) in flight"));
        }
        Ok(record)
    }

    /// T0–T4 then E0–E5 under one hold of the per-extension mutex, bit
    /// untouched, **no W** (design §3.4.1).
    ///
    /// From `Enabled` and `Failed{*}` only. From `Failed{*}` there is no T0 and
    /// no drain: E0's CAS → E-PRE on any held handle → E1–E5.
    async fn reload(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError> {
        self.guard_kind(id)?;
        self.guard_orphan(id)?;
        let _lock = self.lock_for(&id.name).await;
        let (dir, manifest) = self.declaration(id).await?;

        let Some(record) = self.ledger.record(id) else {
            return Err(ExtensionError::NotLoaded);
        };

        // The store is read **before** T0, the way `enable`/`disable` put W
        // before their CAS: §3.2's persistence rule is that nothing is ever
        // left in `Enabling`/`Disabling` because of a disk error. Read after
        // T4, an unreadable store returned `409` with the child already killed,
        // every contribution withdrawn and the record stranded in `Disabling`,
        // where the gate refuses everything with *"is being reloaded right
        // now"* until someone repairs the file and enables it again.
        let table = self.permission_gate.load_table().map_err(store_error)?;

        // §3.4.1's suppression, the design's option (a): T1's step 3 is
        // **deferred** on a reload until the outcome is known, and its cron
        // notice is emptied when the reload ended `Enabled`. The dispatcher's
        // rule stays §7.3 step 2's verbatim, with no cause special case.
        let mut deferred: Option<PendingScan> = None;

        match record.state {
            ExtensionState::Enabled => {
                let Transition::Took(_) = self.ledger.begin(
                    id,
                    ExtensionState::Disabling,
                    Some(WithdrawalCause::Reload),
                ) else {
                    return Err(ExtensionError::NotLoaded);
                };
                deferred = Some(self.t1_t2(id).await);
                self.t3(id).await;
                self.t4(&id.name).await;
                // The `Disabling → Enabling` CAS follows: T5's dedup-clear and
                // emit do not run on this path — the verb has not ended yet.
            }
            ExtensionState::Failed { .. } => {}
            _ => return Err(ExtensionError::NotLoaded),
        }

        let Transition::Took(generation) = self.ledger.begin(id, ExtensionState::Enabling, None)
        else {
            self.publish_reload_scan(id, deferred);
            return self.row(id).await;
        };
        self.e_pre(id).await;
        self.load(id, &dir, manifest, &table, generation).await;
        self.publish_reload_scan(id, deferred);
        self.row(id).await
    }

    async fn reconcile(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError> {
        self.guard_kind(id)?;
        // The same `try_wait` sweep the read paths run (design §3.6 item 3): a
        // reconcile that rebuilt a row from a dead child would report it
        // `Enabled`.
        self.sweep().await;
        let table = self.permission_gate.load_table();
        let dir = self.plugin_dir.join(&id.name);
        if dir.join("plugin.toml").exists() {
            self.reconcile_dir(&dir, &table).await;
        } else if self.plugins.read().await.contains_key(&id.name) {
            self.orphan(&id.name, &table).await;
        }
        self.row(id).await
    }

    async fn reconcile_all(&self) {
        if let Err(e) = self.start().await {
            warn!(error = %e, "plugin reconcile failed");
        }
    }

    async fn list(&self) -> Vec<ExtensionRecord> {
        self.sweep().await;
        let ids: Vec<ExtensionId> = self
            .ledger
            .list()
            .into_iter()
            .filter(|r| r.id.kind == ExtensionKind::Plugin)
            .map(|r| r.id)
            .collect();
        let mut rows = Vec::with_capacity(ids.len());
        for id in ids {
            if let Ok(row) = self.row(&id).await {
                rows.push(row);
            }
        }
        rows
    }

    /// **§3.5.** T2–T4 for every loaded plugin on the way out.
    ///
    /// This closes an existing leak: nothing called `unload_plugin` at shutdown,
    /// and `kill_on_drop` does not fire on `process::exit`, so plugin children
    /// could outlive the daemon.
    async fn shutdown_all(&self) {
        let live: Vec<String> = self
            .plugins
            .read()
            .await
            .iter()
            .filter(|(_, state)| state.holds_handle())
            .map(|(name, _)| name.clone())
            .collect();
        let count = live.len();
        for name in live {
            let _lock = self.lock_for(&name).await;
            self.t2(&name).await;
            self.t3(&ExtensionId::plugin(name.clone())).await;
            self.t4(&name).await;
        }
        info!(plugins = count, "plugins shut down");
    }
}

// ── free functions ──────────────────────────────────────────────────────

fn unapproved(reason: UnapprovedReason) -> ExtensionState {
    ExtensionState::Unapproved { reason }
}

/// A store failure, mapped to the supervisor-level refusal C6 turns into a
/// status code. The code is the route's; this is the fact (design §4).
fn store_error(e: PluginError) -> ExtensionError {
    match e {
        PluginError::StoreUnreadable(detail) => ExtensionError::StoreUnreadable(detail),
        other => ExtensionError::WriteFailed(other.to_string()),
    }
}

/// The `[types]` table as declared, for the row's `declared` field (X-19).
fn declared_types(manifest: &PluginManifest) -> BTreeMap<String, bool> {
    BTreeMap::from([
        ("tool".to_string(), manifest.types.tools),
        ("connector".to_string(), manifest.types.connector),
        ("provider".to_string(), manifest.types.provider),
        ("skill".to_string(), manifest.types.skill),
        ("agent".to_string(), manifest.types.agent),
    ])
}

/// Bring-up classification for plugins (design §4.2).
///
/// **Honestly bounded:** §4.2 allows a plugin's `initialize` error to carry
/// `error.data.reason == "needs_authorization"`, but `StdioChannel` drops
/// `error.data` when it builds `PluginError::RpcError`, so that signal cannot
/// reach here today and a bring-up failure degrades to `Unreachable` — which is
/// exactly what §4.2 says happens absent the signal.
fn classify_bringup(error: &PluginError) -> FailureReason {
    match error {
        PluginError::MissingConfig(missing) => FailureReason::NeedsConfig {
            missing: missing.clone(),
        },
        PluginError::InvalidManifest(_) | PluginError::ManifestNotFound(_) => {
            FailureReason::ConfigInvalid
        }
        PluginError::ChannelClosed | PluginError::ProcessCrashed => FailureReason::Crashed,
        _ => FailureReason::Unreachable,
    }
}

fn encryptor() -> Result<KeyEncryptor, PluginError> {
    let dir = openalpaca_storage::store::master_key_dir()
        .map_err(|e| PluginError::Unavailable(format!("no master key directory: {e}")))?;
    KeyEncryptor::load_or_generate_at(&dir)
        .map_err(|e| PluginError::Unavailable(format!("no master key: {e}")))
}

/// Stop a plugin child: graceful shutdown RPC, then kill, then wait for it to
/// actually go (design §3.2 T4).
///
/// `kill()` is `Child::start_kill`, which only *initiates* termination: without
/// the wait, "the plugin's process is gone" would be a race the caller cannot
/// win. Waits at most [`CHILD_EXIT_TIMEOUT`].
///
/// **Both `shutdown()` and `kill()` are skipped when `exited` says the process
/// has already gone.** After a reaped exit tokio's `start_kill` returns
/// `InvalidInput` and `PluginProcess::kill` logs it at `error!`, so without the
/// skip every sweep-detected crash would be followed by a spurious *"failed to
/// kill plugin process"* line from the reaper's T4.
async fn shutdown_child(name: &str, mut process: PluginProcess, exited: Option<ExitStatus>) {
    if let Some(status) = exited {
        debug!(plugin = name, ?status, "plugin child had already exited");
        return;
    }
    if let Err(e) = process.shutdown().await {
        warn!(plugin = name, error = %e, "shutdown RPC failed, killing");
    }
    process.kill();
    match tokio::time::timeout(CHILD_EXIT_TIMEOUT, process.child.wait()).await {
        Ok(Ok(status)) => debug!(plugin = name, ?status, "plugin child exited"),
        Ok(Err(e)) => warn!(plugin = name, error = %e, "failed to wait for plugin child"),
        Err(_) => warn!(
            plugin = name,
            "child did not exit after SIGKILL within {}s",
            CHILD_EXIT_TIMEOUT.as_secs()
        ),
    }
}

/// Build a [`SkillFrontmatter`] from a `skill/info` JSON response.
///
/// Extracts name, description, invoke config, and routing patterns from the
/// plugin's response, using sensible defaults for any missing fields.
///
/// **`invoke.cron` is deliberately not mapped** (design §10 case 5). Plugin
/// skills are registered into the same catalog `scheduled_skills::sync_all`
/// iterates, so a cron expression here *would* register a wake job — one T2
/// does not withdraw, because `PluginManager` holds no `WakeManager`. The pin is
/// `plugin_skill_frontmatter_never_carries_cron`; if cron is ever mapped, T2
/// step 2 must gain `wake.remove_job(skill_job_id(id))` for each withdrawn
/// plugin skill, which means handing this manager a `WakeManager` handle.
fn build_skill_frontmatter_from_info(info: &Value, plugin_name: &str) -> SkillFrontmatter {
    let name = info
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or(plugin_name)
        .to_string();

    let description = info
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();

    let mode = info
        .get("invoke")
        .and_then(|inv| inv.get("mode"))
        .and_then(|m| m.as_str())
        .unwrap_or("manual")
        .to_string();

    let slash = info
        .get("invoke")
        .and_then(|inv| inv.get("slash"))
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());

    let aliases: Vec<String> = info
        .get("invoke")
        .and_then(|inv| inv.get("aliases"))
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let intent: Vec<String> = info
        .get("routing")
        .and_then(|r| r.get("intent"))
        .and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let keywords: Vec<String> = info
        .get("routing")
        .and_then(|r| r.get("keywords"))
        .and_then(|k| k.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    SkillFrontmatter {
        name,
        description,
        invoke: InvokeConfig {
            mode,
            slash,
            aliases,
            ..Default::default()
        },
        routing: RoutingConfig {
            intent,
            keywords,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Build an [`AgentTemplate`] from an `agent/info` JSON response.
///
/// Creates a minimal template with the plugin bridge as the execution source.
fn build_agent_template_from_info(
    info: &Value,
    plugin_name: &str,
    bridge: Arc<PluginAgentBridge>,
) -> AgentTemplate {
    let id = info
        .get("id")
        .and_then(|i| i.as_str())
        .unwrap_or(plugin_name)
        .to_string();

    let name = info
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or(plugin_name)
        .to_string();

    let description = info
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();

    let capabilities: Vec<String> = info
        .get("capabilities")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let model = info
        .get("model")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string());

    let singleton = info
        .get("singleton")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);

    AgentTemplate {
        frontmatter: AgentTemplateFrontmatter {
            id,
            name,
            description,
            icon: None,
            singleton,
            capabilities,
            denied_capabilities: Vec::new(),
            temperature: 0.5,
            verbosity: "normal".to_string(),
            model,
            fallback_models: Vec::new(),
            max_tool_calls: None,
            timeout_seconds: None,
            max_cost_per_task: None,
            max_rounds: None,
            require_confirmation_for: Vec::new(),
        },
        body: String::new(),
        sections: HashMap::new(),
        source: AgentSource::Plugin {
            plugin_id: plugin_name.to_string(),
            executor: bridge,
        },
    }
}

/// Convert a `toml::Value` to a `serde_json::Value`.
fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(dt) => Value::String(dt.to_string()),
        toml::Value::Array(arr) => Value::Array(arr.iter().map(toml_to_json).collect()),
        toml::Value::Table(tbl) => {
            let map: serde_json::Map<String, Value> = tbl
                .iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect();
            Value::Object(map)
        }
    }
}

/// Poison recovery for the std mutexes, matching the rest of the codebase.
trait LockOrRecover<T> {
    fn lock_or_recover(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockOrRecover<T> for StdMutex<T> {
    fn lock_or_recover(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|p| {
            warn!("plugin manager mutex poisoned — recovering");
            p.into_inner()
        })
    }
}

#[cfg(test)]
mod tests;
