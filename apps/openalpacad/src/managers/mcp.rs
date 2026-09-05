//! `McpSupervisor` — the ENABLE axis for MCP servers (extension design
//! ADR-030, §3; commit C2).
//!
//! One object owns everything an MCP server's lifecycle needs and that the
//! ledger deliberately does not: the `Arc<McpClient>` per server, the file
//! writer for `config/mcp.toml`, the per-extension mutex every transition runs
//! under, the crash reaper and the per-server `tools/list_changed` reader.
//! `ExtensionLedger` stays pure bookkeeping in `openalpaca_core` (§5).
//!
//! **The guarantee this exists for (S2):** a disabled server's child is killed,
//! its connection dropped, and nothing respawns it. Three independent
//! mechanisms hold it up — `reconcile_all` never connects a disabled server;
//! `reconnect` is reachable only from `list_tools`/`call_tool`, which the gate
//! refuses first; and the `closed` seal makes reconnection terminal even if
//! both were bypassed, including for a reconnect that was already in flight
//! when the switch flipped (§10 case 7).
//!
//! **Order, which is the whole design:** W (write the bit) → T0/E0 (the CAS) →
//! everything else. W runs *after* the per-extension mutex is taken, so the
//! file order and the transition order can never cross; a failed write is a
//! `500` with **no** CAS, so the in-memory state never runs ahead of the disk.
//!
//! Between C2 and C6 this is parked on the services bundle: the file watcher
//! finds it there (edge case 15) and the daemon shutdown path calls its
//! `shutdown_all()` directly. C6 folds it into the `Extensions` aggregator.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use arc_swap::ArcSwap;
use openalpaca_core::bus::EventBus;
use openalpaca_core::config_io::atomic_write_toml;
use openalpaca_core::daemon_config::DaemonConfig;
use openalpaca_core::events::SystemEvent;
use openalpaca_core::tools::ToolRegistry;
use openalpaca_core::tools::extensions::{
    ExtensionError, ExtensionId, ExtensionKind, ExtensionLedger, ExtensionRecord, ExtensionState,
    ExtensionSupervisor, FailureReason, Transition, WithdrawalCause,
};
use openalpaca_core::tools::mcp::{
    HttpAuthConfig, LoadError, McpConfig, McpDefaults, McpServerConfig, bridge,
    classify_bringup_failure, classify_call_failure, config_fingerprint,
};
use openalpaca_mcp::{
    ConnectionSnapshot, Implementation, McpClient, McpClientConfig, McpError, ServerChange,
    TransportKind,
};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// The id of the whole-file pseudo-record §5.1 registers when
/// `config/mcp.toml` does not parse. It can never collide with a real server:
/// `is_valid_server_name` rejects a name containing `/` or `.`.
pub const CONFIG_PSEUDO_ID: &str = "config/mcp.toml";

/// How long the drain polls between checks. Short enough that a `disable` of an
/// idle server returns immediately, long enough not to spin.
const DRAIN_POLL: Duration = Duration::from_millis(5);

/// How many of the supervisor's own writes are remembered for the watcher's
/// dedup ring. Matches the persona rings in `hot_reload.rs`.
const OWN_WRITE_RING: usize = 8;

// ============================================================================
// Handles
// ============================================================================

/// What the supervisor holds for one *loaded* server. Its presence in the map
/// is the answer to "does this extension hold a handle?" — T4 asks the map, not
/// the state, which is what makes it correct from `Enabled` **and** from a
/// pre-reaper `Failed{Crashed}` and a no-op from every other cell (§3.2 T4).
struct ServerHandle {
    client: Arc<McpClient>,
    generation: u64,
    /// The bound T4 waits for `disconnect` under, from this server's own
    /// declaration: `call_tool` holds the transport mutex across its await, so
    /// a straggler can hold `disconnect` off for up to one request.
    request_timeout: Duration,
}

/// The last **declaration set that parsed** (§10 case 15). An unparseable
/// rewrite keeps this and skips the diff, so nothing running is torn down by a
/// half-typed block.
#[derive(Default, Clone)]
struct Declared {
    defaults: McpDefaults,
    servers: BTreeMap<String, McpServerConfig>,
}

/// A `bearer_env` / `api_key_env` that is not set in the daemon's environment.
/// OpenAlpaca refuses up front rather than attempting the connection, so the
/// row can name the missing key (§4.2, X-31).
struct MissingEnv {
    var: String,
    message: String,
}

// ============================================================================
// The supervisor
// ============================================================================

pub struct McpSupervisor {
    /// A weak self-reference so the per-server reader tasks and the reaper can
    /// reach the supervisor without keeping it alive in a cycle.
    me: Weak<Self>,
    config_path: PathBuf,
    tool_registry: Arc<ToolRegistry>,
    ledger: Arc<ExtensionLedger>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    bus: EventBus,

    /// The per-extension mutex **held across the whole transition** (§3), so
    /// two toggles serialise and a toggle never interleaves with a reconcile,
    /// with the reaper, or with the *apply* half of a tool-list refresh.
    locks: StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// The live handles. Never read while an `.await` is pending.
    handles: StdMutex<HashMap<String, ServerHandle>>,
    declared: StdMutex<Declared>,
    /// Content hashes of writes this supervisor made, pushed **before** the
    /// rename so the watcher swallows the daemon's own write (§10 case 15).
    own_writes: StdMutex<VecDeque<String>>,
    /// The crash-reaper receiver, parked until [`Self::spawn_reaper`] takes it.
    /// A test drives [`Self::reap`] directly instead, which is what makes the
    /// *reaper superseded* scenarios deterministic.
    reaper_rx: StdMutex<Option<UnboundedReceiver<(ExtensionId, u64)>>>,
    /// Live `tools/list_changed` reader tasks, so a test can observe that the
    /// one belonging to a torn-down load exited (§3.3 E-PRE, E-FAIL).
    readers_running: Arc<AtomicUsize>,
}

impl McpSupervisor {
    /// Build the supervisor and register its crash-reaper channel with the
    /// ledger. Call [`Self::spawn_reaper`] to start draining it.
    pub fn new(
        config_path: PathBuf,
        tool_registry: Arc<ToolRegistry>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
        bus: EventBus,
    ) -> Arc<Self> {
        let ledger = Arc::clone(tool_registry.extensions());
        let (tx, rx) = unbounded_channel();
        ledger.on_crash(ExtensionKind::Mcp, tx);
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            config_path,
            tool_registry,
            ledger,
            daemon_config,
            bus,
            locks: StdMutex::new(HashMap::new()),
            handles: StdMutex::new(HashMap::new()),
            declared: StdMutex::new(Declared::default()),
            own_writes: StdMutex::new(VecDeque::with_capacity(OWN_WRITE_RING)),
            reaper_rx: StdMutex::new(Some(rx)),
            readers_running: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Start the crash reaper: one sequential task per kind that drains
    /// `mark_failed`'s channel and runs T1 → T4 on each message it is still
    /// entitled to (§3.6).
    pub fn spawn_reaper(self: &Arc<Self>) {
        let Some(mut rx) = self.reaper_rx.lock_or_recover().take() else {
            tracing::warn!("MCP crash reaper already started");
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

    /// Live `tools/list_changed` reader tasks. Zero once every load has been
    /// torn down — the observable behind "the old reader task exited".
    pub fn readers_running(&self) -> usize {
        self.readers_running.load(Ordering::SeqCst)
    }

    /// Did *this* supervisor write these bytes? The watcher observes the
    /// daemon's own write exactly as it observes a hand edit; the ring is what
    /// swallows it, so a route-driven toggle runs only its in-process
    /// transition (§10 case 15).
    pub fn swallow_own_write(&self, contents: &str) -> bool {
        let hash = content_hash(contents);
        let mut ring = self.own_writes.lock_or_recover();
        match ring.iter().position(|h| *h == hash) {
            Some(pos) => {
                ring.remove(pos);
                true
            }
            None => false,
        }
    }

    fn remember_own_write(&self, contents: &str) {
        let mut ring = self.own_writes.lock_or_recover();
        ring.push_back(content_hash(contents));
        while ring.len() > OWN_WRITE_RING {
            ring.pop_front();
        }
    }

    // ── Plumbing ─────────────────────────────────────────────────────────

    async fn lock_for(&self, name: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.locks.lock_or_recover();
            Arc::clone(locks.entry(name.to_string()).or_default())
        };
        lock.lock_owned().await
    }

    fn drain_timeout(&self) -> Duration {
        Duration::from_secs(self.daemon_config.load().extensions.drain_timeout_secs)
    }

    /// The one emitter. Every transition announces itself here and nowhere
    /// else, so `state` is always a state word rendered from the ledger — or
    /// the literal `"removed"` of T5-gone, where the row simply disappears.
    fn emit(&self, ext: &ExtensionId, state: &str, generation: u64, tools_changed: bool) {
        self.bus.publish(SystemEvent::ExtensionStateChanged {
            extension: ext.clone(),
            state: state.to_string(),
            generation,
            tools_changed,
            timestamp: chrono::Utc::now(),
        });
    }

    fn emit_record(&self, ext: &ExtensionId) {
        if let Some(record) = self.ledger.record(ext) {
            self.emit(ext, record.state.word(), record.generation, false);
        }
    }

    /// The record as the API reads it, with any per-call warnings attached.
    fn row(&self, ext: &ExtensionId, warnings: Vec<String>) -> Result<ExtensionRecord, ExtensionError> {
        let mut record = self
            .ledger
            .record(ext)
            .ok_or_else(|| ExtensionError::NotFound(ext.clone()))?;
        record.warnings = warnings;
        Ok(record)
    }

    fn is_declared(&self, name: &str) -> bool {
        self.declared.lock_or_recover().servers.contains_key(name)
    }

    fn declaration(&self, name: &str) -> Option<(McpServerConfig, McpDefaults)> {
        let declared = self.declared.lock_or_recover();
        declared
            .servers
            .get(name)
            .cloned()
            .map(|cfg| (cfg, declared.defaults.clone()))
    }

    // ── W — PERSIST ──────────────────────────────────────────────────────

    /// **Step W.** Write `enabled = <bool>` through the atomic, comment-
    /// preserving writer, **before** the CAS. A failed write returns
    /// `WriteFailed` (the route's `500`) and takes no transition: the extension
    /// keeps running and the row still reads what the disk says, which is the
    /// truth.
    ///
    /// The rendered document is re-parsed with `McpConfig`'s own parser before
    /// the rename, and the post-write hash is pushed into the dedup ring from
    /// inside that same closure — i.e. **before** the rename, which is what
    /// makes the watcher swallow the daemon's own write.
    ///
    /// **There is no off-route write site for MCP**, so §3.2's off-route
    /// persistence rule (log at `error`, keep the state as computed, retry at
    /// the next reconcile) has nothing to attach to here: all four non-route
    /// entrants are no-write *by construction* — on the watcher path the write
    /// **is** the trigger, on the declaration-gone path there is no block to
    /// write into, the reaper does not change the disposition, and `reload` has
    /// no W at all. The rule stays live for the plugin supervisor (C3) and for
    /// C6's config route.
    fn write_bit(&self, name: &str, enabled: bool) -> Result<(), ExtensionError> {
        let path = self.config_path.clone();
        atomic_write_toml(
            &path,
            |doc| {
                let declared = doc
                    .get("servers")
                    .and_then(|s| s.as_table_like())
                    .is_some_and(|t| t.get(name).is_some());
                if !declared {
                    // Never synthesize a block: the writer's mandatory re-parse
                    // would reject a `[servers.<n>]` table with no `transport`
                    // tag anyway, and "the declaration is the toggle" (§5.1).
                    return Err(format!(
                        "server '{name}' is not declared in {}",
                        path.display()
                    ));
                }
                doc["servers"][name]["enabled"] = toml_edit::value(enabled);
                Ok(())
            },
            |rendered| match McpConfig::parse(rendered) {
                Ok(_) => {
                    self.remember_own_write(rendered);
                    Ok(())
                }
                Err(e) => Err(e.to_string()),
            },
        )
        .map_err(|e| {
            tracing::error!(
                server = %name,
                path = %self.config_path.display(),
                error = %e,
                "MCP store write failed"
            );
            ExtensionError::WriteFailed(e.to_string())
        })
    }

    // ── T1 — CAPABILITY WITHDRAWAL ───────────────────────────────────────

    /// **T1 steps 1–2.** Tombstone each retained tool's capabilities and remove
    /// it from the registry.
    ///
    /// Idempotent by construction: an absent registry entry contributes
    /// nothing, which is what makes a second pass — the reaper after a route
    /// disable, a disable after E-FAIL — a no-op. Only names the ledger
    /// *currently* attributes to this extension are touched, so disabling A can
    /// never delete a tool B displaced it from (§10 case 13).
    ///
    /// **Step 3** — the dependent scan, `ExtensionCapabilityWithdrawn` and the
    /// cron notice — lands in C4 with the event and the agent-registry handle.
    /// It fires only on a non-empty withdrawn set, which is the count returned
    /// here.
    fn t1(&self, ext: &ExtensionId, _cause: WithdrawalCause) -> usize {
        let mut withdrawn = 0usize;
        for name in self.ledger.tool_names(ext) {
            if self.ledger.owner_of(&name).as_ref() != Some(ext) {
                continue;
            }
            if let Some(tool) = self.tool_registry.get(&name) {
                self.ledger.withdraw(ext, tool.provides_capabilities.clone());
            }
            if self.tool_registry.remove(&name) {
                withdrawn += 1;
            }
        }
        withdrawn
    }

    // ── T3 — DRAIN ───────────────────────────────────────────────────────

    /// **T3.** Wait for the extension's in-flight tool calls to finish,
    /// bounded by `[extensions] drain_timeout_secs`. Returns the straggler
    /// count, which is `0` on a clean drain.
    ///
    /// In-flight calls are allowed to finish; we do not cancel them. On a
    /// single-user daemon the risk worth engineering against is corruption — a
    /// half-written file, a duplicated POST — not a 200 ms exposure window. New
    /// calls have been refused since T0.
    async fn t3(&self, ext: &ExtensionId) -> usize {
        let deadline = self.drain_timeout();
        let drained = tokio::time::timeout(deadline, async {
            while self.ledger.in_flight(ext) > 0 {
                tokio::time::sleep(DRAIN_POLL).await;
            }
        })
        .await;
        if drained.is_ok() {
            return 0;
        }
        let in_flight = self.ledger.in_flight(ext);
        tracing::warn!(
            extension = %ext,
            in_flight,
            "disable draining timed out; forcing teardown"
        );
        in_flight
    }

    // ── T4 — TEARDOWN ────────────────────────────────────────────────────

    /// **T4 + T4b.** Tear down whatever handle the map holds for this
    /// extension and remove it from the map. A no-op from any state that holds
    /// none — it asks the map, never the state.
    ///
    /// `disconnect` seals the shared inner *before* it takes the service lock,
    /// so from that instant no clone can reconnect and no in-flight handshake
    /// can install a live child. It is awaited under `request_timeout + 1s`,
    /// because `call_tool` holds the transport mutex across its request; on
    /// expiry a **fresh** `disconnect` future is spawned detached (the timed-out
    /// one is consumed) and the caller is told, since the seal is already set
    /// and nothing can reconnect meanwhile.
    ///
    /// The `tools/list_changed` reader is not joined here: it is waiting on the
    /// very mutex this teardown holds, so joining it would deadlock. Dropping
    /// the sender inside `disconnect` ends its receiver, and it exits on its
    /// own next turn — after failing the state re-check that makes a straggler
    /// notification harmless.
    async fn t4(&self, name: &str) -> Option<String> {
        let handle = self.handles.lock_or_recover().remove(name);
        let handle = handle?;

        let bound = handle.request_timeout + Duration::from_secs(1);
        let generation = handle.generation;
        let client = Arc::clone(&handle.client);
        match tokio::time::timeout(bound, (*client).clone().disconnect()).await {
            Ok(Ok(())) => {
                tracing::debug!(server = %name, generation, "MCP client disconnected and sealed");
                None
            }
            Ok(Err(e)) => {
                tracing::warn!(
                    server = %name,
                    generation,
                    error = %e,
                    "MCP disconnect reported an error"
                );
                None
            }
            Err(_elapsed) => {
                tracing::warn!(
                    server = %name,
                    generation,
                    ?bound,
                    "MCP teardown detached: a call is still holding the transport"
                );
                let detached = Arc::clone(&handle.client);
                tokio::spawn(async move {
                    if let Err(e) = (*detached).clone().disconnect().await {
                        tracing::debug!(error = %e, "detached MCP disconnect finished with an error");
                    }
                });
                Some("teardown pending: 1 call still holding the transport".to_string())
            }
        }
    }

    /// **E-PRE.** Tear down whatever the map still holds for this extension
    /// before building anything — the first step after E0's CAS on the shared
    /// load path, so no entrant can skip it.
    ///
    /// It runs the reaper's shape (T1 → T4, **no** T3 drain — the gate has
    /// refused since `mark_failed`) with cause `Crash`, because the only state
    /// that reaches here holding anything is a pre-reaper `Failed{Crashed}`.
    /// Without it a Retry that wins the mutex before the reaper would build
    /// load N+1 on top of load N's live residue and the reaper would then find
    /// "superseded" and do nothing.
    async fn e_pre(&self, ext: &ExtensionId) {
        let residue = self
            .handles
            .lock_or_recover()
            .get(&ext.name)
            .map(|h| h.generation);
        let Some(residue) = residue else { return };
        tracing::info!(
            extension = %ext,
            residue_generation = residue,
            "E-PRE: tearing down a previous load's residue"
        );
        self.t1(ext, WithdrawalCause::Crash);
        self.t4(&ext.name).await;
    }

    // ── E2–E5 — the load path ────────────────────────────────────────────

    /// E2 → E5 for one server, under a CAS that has already taken. Returns the
    /// state it committed.
    ///
    /// E1 (consent) has no MCP analogue: writing a server into your own
    /// `config/mcp.toml` *is* the consent (§3.3 E1).
    async fn load(
        &self,
        name: &str,
        cfg: &McpServerConfig,
        defaults: &McpDefaults,
        generation: u64,
    ) -> ExtensionState {
        let ext = ExtensionId::mcp(name);

        // E2 — stamp the declaration this load is built from, before anything
        // can fail: edge case 15 compares it to decide whether a later watcher
        // event is a real change, and a `Failed` row needs it just as much as
        // an `Enabled` one.
        self.ledger
            .set_config_fingerprint(&ext, Some(config_fingerprint(cfg)));
        self.ledger.clear_tools_changed(&ext);

        // E2 — re-resolve `bearer_env`/`api_key_env` from the process env, so a
        // rotated credential takes effect here without a restart.
        let client_config = match build_client_config(name, cfg, defaults) {
            Ok(c) => c,
            Err(missing) => {
                return self.fail(
                    &ext,
                    FailureReason::NeedsConfig {
                        missing: vec![missing.var],
                    },
                    missing.message,
                );
            }
        };
        let request_timeout = client_config.request_timeout;

        let connect_timeout = Duration::from_secs(
            cfg.connect_timeout_secs()
                .unwrap_or(defaults.connect_timeout_secs),
        );
        let client = match tokio::time::timeout(connect_timeout, McpClient::connect(client_config))
            .await
        {
            Ok(Ok(client)) => Arc::new(client),
            Ok(Err(e)) => {
                // `connect` itself failed, so nothing was built and there is
                // nothing to unwind.
                let reason = classify_bringup_failure(&e);
                return self.fail(&ext, reason, format!("connect: {e}"));
            }
            Err(_elapsed) => {
                return self.fail(
                    &ext,
                    FailureReason::Unreachable,
                    format!("connect timed out after {connect_timeout:?}"),
                );
            }
        };

        let server_version = client
            .server_info()
            .map(|i| i.version.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // E2 — take the change receiver with the client, once, and start the
        // reader. Its first message is *applied* only after E5: step 3 takes
        // the mutex this load still holds and then re-checks for `Enabled`, so
        // a notification during a failed bring-up is dropped by that re-check.
        self.spawn_change_reader(&ext, generation, &client).await;

        // E3 — the one-shot fetch. A client that has just handshaken has no
        // reconnect to make, so bring-up stays bounded by `connect_timeout` +
        // one `request_timeout`, not four reconnect cycles under the mutex.
        let tools = match client.list_tools_once().await {
            Ok(tools) => tools,
            Err(e) => {
                // E-FAIL — this failed *after* connect, so a handle exists.
                let reason = self.classify_after_connect(&client, &e).await;
                self.tear_down_bringup(name, &client, request_timeout).await;
                return self.fail(&ext, reason, format!("list_tools: {e}"));
            }
        };

        // E4 — publish, remove-before-register, with the case-13 collision rule.
        let mut registered: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        for tool in tools {
            let registered_tool = bridge::rmcp_tool_to_registered(
                name,
                &server_version,
                tool,
                Arc::clone(&client),
                generation,
            );
            let tool_name = registered_tool.definition.name.clone();
            if self.is_live_incumbent(&ext, &tool_name) {
                tracing::warn!(
                    extension = %ext,
                    tool = %tool_name,
                    "tool name collision — skipping"
                );
                skipped.push(tool_name);
                continue;
            }
            match self.tool_registry.replace(registered_tool) {
                Ok(()) => registered.push(tool_name),
                Err(e) => {
                    // E4b — the registry never holds a half-loaded extension.
                    for published in &registered {
                        self.tool_registry.remove(published);
                    }
                    self.tear_down_bringup(name, &client, request_timeout).await;
                    return self.fail(
                        &ext,
                        FailureReason::Unreachable,
                        format!("tool '{tool_name}' could not be registered: {e}"),
                    );
                }
            }
        }

        // E5 — publish state. No file I/O: the bit was written at W.
        self.handles.lock_or_recover().insert(
            name.to_string(),
            ServerHandle {
                client,
                generation,
                request_timeout,
            },
        );
        self.ledger.restore(&ext);
        self.ledger.record_tools(&ext, registered.clone());
        self.ledger.commit(&ext, ExtensionState::Enabled);
        tracing::info!(
            extension = %ext,
            generation,
            tool_count = registered.len(),
            skipped = skipped.len(),
            "MCP server enabled"
        );
        self.emit(&ext, "enabled", generation, false);
        ExtensionState::Enabled
    }

    /// Is `tool_name` currently served by a **different**, live extension? Only
    /// a live incumbent blocks a name: not `Enabled`, or `Enabled` but holding
    /// the name in its own `server_withdrawn` set, means the newcomer takes it
    /// (§10 case 13).
    fn is_live_incumbent(&self, ext: &ExtensionId, tool_name: &str) -> bool {
        let Some(incumbent) = self.ledger.owner_of(tool_name) else {
            return false;
        };
        if &incumbent == ext {
            return false;
        }
        self.ledger
            .state(&incumbent)
            .is_some_and(|s| s.is_enabled() && !self.ledger.is_server_withdrawn(&incumbent, tool_name))
    }

    /// **E-FAIL.** Tear down a handle whose bring-up failed *after*
    /// `connect` returned, before `Failed{..}` is committed. Without this a
    /// `Failed` row would sit over a live, unsealed client and a child nothing
    /// ever drops — and a later `disable` from `Failed`, which §4.1 allows,
    /// would run T4 on nothing while the child outlived the switch.
    ///
    /// `disconnect` sets the seal first, then takes `changes_tx`, which ends
    /// the reader's receiver so that task exits.
    async fn tear_down_bringup(&self, name: &str, client: &Arc<McpClient>, bound: Duration) {
        let client = Arc::clone(client);
        let deadline = bound + Duration::from_secs(1);
        match tokio::time::timeout(deadline, (*client).clone().disconnect()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!(server = %name, error = %e, "E-FAIL disconnect reported an error");
            }
            Err(_elapsed) => {
                tracing::warn!(server = %name, "E-FAIL teardown detached");
                tokio::spawn(async move {
                    let _ = (*client).clone().disconnect().await;
                });
            }
        }
    }

    /// A bring-up error that arrived after a good handshake: prefer the
    /// client's own terminal state over the last error when it has one.
    async fn classify_after_connect(&self, client: &McpClient, error: &McpError) -> FailureReason {
        match classify_call_failure(error) {
            Some(reason) => reason,
            None => match client.connection_state().await {
                ConnectionSnapshot::Failed { .. } => FailureReason::Crashed,
                _ => classify_bringup_failure(error),
            },
        }
    }

    /// Commit a bring-up failure. `enabled` stays **true** — it was written at
    /// W, before E0, so the owner's intent is durable and orthogonal to whether
    /// the thing works; a restart reads it and tries again (§3.4).
    fn fail(&self, ext: &ExtensionId, reason: FailureReason, detail: impl Into<String>) -> ExtensionState {
        let detail = detail.into();
        let state = ExtensionState::Failed {
            reason: reason.clone(),
            detail: detail.clone(),
            since: chrono::Utc::now(),
        };
        tracing::warn!(
            extension = %ext,
            reason = reason.word(),
            detail = %detail,
            "MCP server bring-up failed"
        );
        // E0's CAS put the record in `Enabling`; `commit` is the exit from it.
        if !self.ledger.commit(ext, state.clone()) {
            self.ledger.store_state(ext, state.clone());
        }
        self.emit(ext, "failed", self.ledger.generation(ext).unwrap_or(0), false);
        state
    }

    // ── §3.7 — the server changes its own tool set ───────────────────────

    /// Take the change receiver (once, with the client) and start the
    /// per-server reader.
    ///
    /// The task tags every message with **this** incarnation's generation, so a
    /// straggler queued before a teardown fails the re-check in step 3 rather
    /// than editing a newer load's registry.
    ///
    /// Coalescing: one refresh runs at a time per server, and anything that
    /// arrives during a refresh is drained into a **single** follow-up. A
    /// chatty server cannot stampede `tools/list`.
    async fn spawn_change_reader(&self, ext: &ExtensionId, generation: u64, client: &Arc<McpClient>) {
        let Some(mut rx) = client.changes().await else {
            tracing::debug!(extension = %ext, "MCP change receiver already taken");
            return;
        };
        let me = self.me.clone();
        let ext = ext.clone();
        let client = Arc::clone(client);
        let running = Arc::clone(&self.readers_running);
        running.fetch_add(1, Ordering::SeqCst);
        tokio::spawn(async move {
            let _guard = ReaderGuard(running);
            while let Some(first) = rx.recv().await {
                let mut wanted = first == ServerChange::ToolList;
                // Drain whatever is already queued into this one refresh.
                while let Ok(next) = rx.try_recv() {
                    wanted |= next == ServerChange::ToolList;
                }
                if !wanted {
                    // Resources and prompts are stubbed; the variants exist so
                    // un-stubbing them is a supervisor change, not a second
                    // refresh route (§2.3, X-36).
                    continue;
                }
                let Some(sup) = me.upgrade() else { break };
                sup.on_tool_list_changed(&ext, generation, &client).await;
            }
            tracing::debug!(extension = %ext, generation, "MCP change reader exited");
        });
    }

    /// **§3.7.** One tool-list refresh for one incarnation.
    ///
    /// The fetch runs **outside** the mutex — deliberately, and it is the whole
    /// reason `list_tools_once` exists: `list_tools` loops through `reconnect()`
    /// on every retriable error, so a refresh triggered by a dying server would
    /// hold the per-extension mutex for minutes and a queued `disable` could
    /// perform neither W nor T0. The owner's switch would hang while calls kept
    /// succeeding.
    pub async fn on_tool_list_changed(
        &self,
        ext: &ExtensionId,
        generation: u64,
        client: &Arc<McpClient>,
    ) {
        // Step 2 — fetch, no mutex held.
        let fetched = match client.list_tools_once().await {
            Ok(tools) => tools,
            Err(e) => {
                // Keep the recorded set: a transient error must not unpublish a
                // working server.
                tracing::warn!(
                    extension = %ext,
                    error = %e,
                    "tool list refresh failed; keeping previously discovered tools"
                );
                // But the refresh must not leave a dead client under a live row.
                let reason = match classify_call_failure(&e) {
                    Some(reason) => Some(reason),
                    None => match client.connection_state().await {
                        ConnectionSnapshot::Failed { .. } => Some(FailureReason::Crashed),
                        _ => None,
                    },
                };
                if let Some(reason) = reason {
                    let detail = match client.connection_state().await {
                        ConnectionSnapshot::Failed { reason } => reason,
                        _ => e.to_string(),
                    };
                    self.ledger.mark_failed(ext, generation, reason, detail);
                }
                return;
            }
        };

        // Step 3 — mutex, then re-check. There is no path by which a
        // non-`Enabled` server, or an older generation, can change the registry.
        let _lock = self.lock_for(&ext.name).await;
        let current = self.ledger.record(ext);
        let live = current
            .as_ref()
            .is_some_and(|r| r.state.is_enabled() && r.generation == generation);
        if !live {
            tracing::debug!(extension = %ext, generation, "tool list change superseded");
            return;
        }

        // Step 4 — diff against the **live subset**: a name the server dropped
        // in one notification and re-added in a later one is still retained
        // (flagged), so diffing against the whole retained set would drop it
        // into `kept`, never re-register it, and leave step 6's unflag
        // unreachable.
        let withdrawn_flags: Vec<String> = self.ledger.server_withdrawn(ext);
        let flagged: std::collections::BTreeSet<String> =
            withdrawn_flags.iter().map(|n| n.to_lowercase()).collect();
        let retained = self.ledger.tool_names(ext);
        let live_before: std::collections::BTreeSet<String> = retained
            .iter()
            .filter(|n| !flagged.contains(&n.to_lowercase()))
            .cloned()
            .collect();

        let server_version = client
            .server_info()
            .map(|i| i.version.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let mut incoming: BTreeMap<String, openalpaca_mcp::Tool> = BTreeMap::new();
        for tool in fetched {
            incoming.insert(format!("{}__{}", ext.name, tool.name), tool);
        }

        let removed: Vec<String> = live_before
            .iter()
            .filter(|n| !incoming.contains_key(*n))
            .cloned()
            .collect();
        let added: Vec<String> = incoming
            .keys()
            .filter(|n| !live_before.contains(*n))
            .cloned()
            .collect();

        if removed.is_empty() && added.is_empty() {
            tracing::debug!(extension = %ext, "tool list refresh: no change");
            return;
        }

        // Step 5 — removed names: T1 verbatim, minus the state change. The name
        // stays retained and is *flagged*, which is what both gate arms read to
        // refuse it with the "withdrawn by the server" wording instead of an
        // unattributed not-found.
        for name in &removed {
            if let Some(tool) = self.tool_registry.get(name) {
                self.ledger.withdraw(ext, tool.provides_capabilities.clone());
            }
            self.tool_registry.remove(name);
            self.ledger.flag_server_withdrawn(ext, name);
        }
        // T1 step 3 over the removed set, with `WithdrawalCause::ServerListChange`
        // — the dependent scan, its event and the cron notice — is C4's.

        // Step 6 — added names: E4 verbatim, under the case-13 collision rule
        // and the **current** generation.
        let mut restored_caps: Vec<String> = Vec::new();
        let mut live_now: Vec<String> = live_before
            .iter()
            .filter(|n| !removed.contains(n))
            .cloned()
            .collect();
        for name in &added {
            if self.is_live_incumbent(ext, name) {
                tracing::warn!(extension = %ext, tool = %name, "tool name collision — skipping");
                continue;
            }
            let tool = incoming[name].clone();
            let registered = bridge::rmcp_tool_to_registered(
                &ext.name,
                &server_version,
                tool,
                Arc::clone(client),
                generation,
            );
            restored_caps.extend(registered.provides_capabilities.iter().cloned());
            match self.tool_registry.replace(registered) {
                Ok(()) => {
                    // A re-added name is not a collision — `owner_of` returns
                    // this extension — so it is registered like any other
                    // addition and simply unflagged.
                    self.ledger.clear_server_withdrawn(ext, name);
                    live_now.push(name.clone());
                }
                Err(e) => {
                    tracing::warn!(extension = %ext, tool = %name, error = %e, "added MCP tool could not be registered");
                }
            }
        }
        // Per capability, never per extension: a whole-extension `restore`
        // would erase the tombstones step 5 just wrote.
        self.ledger.restore_caps(ext, restored_caps);

        // Step 7 — record the union, so attribution of the removed names
        // survives exactly as it survives a disable. **The generation is not
        // bumped**: same incarnation, same client, and every snapshot's handle
        // to a *kept* tool stays valid.
        let still_flagged = self.ledger.server_withdrawn(ext);
        let mut union: std::collections::BTreeSet<String> = live_now.into_iter().collect();
        union.extend(still_flagged);
        self.ledger.record_tools(ext, union);
        self.ledger.stamp_tools_changed(ext);

        tracing::info!(
            extension = %ext,
            generation,
            added = added.len(),
            removed = removed.len(),
            "MCP server changed its tool set"
        );
        self.emit(ext, "enabled", generation, true);
    }

    // ── §3.6 — the crash reaper ──────────────────────────────────────────

    /// One reaper message. `mark_failed` already set the state; the reaper
    /// **never writes state** and never takes T0 — it enters at T1.
    ///
    /// It re-reads the record under the per-extension mutex and proceeds only
    /// if the row still reads `Failed{Crashed}` at the generation the message
    /// carries. The mutex prevents *interleaving*, not *reordering*: a Retry
    /// that took the mutex first has already built load N+1, and an
    /// unconditional T1 → T4 here would unpublish its tools and kill its live
    /// process while leaving the row `Enabled` — the exact stale-actor teardown
    /// the generations exist to prevent.
    pub async fn reap(&self, ext: &ExtensionId, generation: u64) {
        // Until C4 gives the ledger its own bus, the crash's announcement is
        // the reaper's, published on dequeue — before the re-check, so a
        // superseded reap still announces the crash that *did* happen. The
        // event carries the generation, so the log stays unambiguous even when
        // it lands after load N+1's `enabled`.
        self.emit(ext, "failed", generation, false);

        let _lock = self.lock_for(&ext.name).await;
        let current = self.ledger.record(ext);
        let entitled = current.as_ref().is_some_and(|r| {
            matches!(
                r.state,
                ExtensionState::Failed {
                    reason: FailureReason::Crashed,
                    ..
                }
            ) && r.generation == generation
        });
        if !entitled {
            // Including an absent record, dropped by T5-gone.
            tracing::debug!(extension = %ext, generation, "crash reap superseded");
            return;
        }

        // T1 → T4, but never T5: the disposition stays `true` and the state
        // stays `Failed{Crashed}`, so the row renders toggle ON + crashed +
        // Retry.
        self.t1(ext, WithdrawalCause::Crash);
        self.t4(&ext.name).await;
    }

    // ── Reconciliation ───────────────────────────────────────────────────

    /// Parse the store, or park the whole-file pseudo-record.
    ///
    /// At **boot** there is no last-good set to keep, so a parse failure means
    /// zero servers plus the pseudo-record — and the daemon boots, which it did
    /// not before. Under the **watcher** the last-good desired set is kept and
    /// the diff is skipped, so an editor's intermediate save cannot tear down
    /// every running server (§5.1, §10 case 15).
    fn read_declaration(&self) -> Option<Declared> {
        match McpConfig::load(&self.config_path) {
            Ok(config) => {
                self.clear_pseudo_record();
                Some(Declared {
                    defaults: config.defaults,
                    servers: config.servers,
                })
            }
            Err(LoadError::NotFound(_)) => {
                tracing::info!(
                    path = %self.config_path.display(),
                    "no config/mcp.toml — no MCP servers declared"
                );
                self.clear_pseudo_record();
                Some(Declared::default())
            }
            Err(e) => {
                tracing::error!(
                    path = %self.config_path.display(),
                    error = %e,
                    "config/mcp.toml could not be parsed; keeping the last good declaration set"
                );
                let backup = openalpaca_core::config_io::copy_unparseable_once(&self.config_path);
                let detail = match backup {
                    Some(path) => format!("{e}; last good copy: {}", path.display()),
                    None => e.to_string(),
                };
                let ext = ExtensionId::mcp(CONFIG_PSEUDO_ID);
                self.ledger.upsert(
                    &ext,
                    // The pseudo-record has no disposition anyone can read; the
                    // API row reports `enabled: null` and every verb on it is a
                    // `409 store_unreadable` (§4). The stored bit is never read
                    // for it.
                    false,
                    ExtensionState::Failed {
                        reason: FailureReason::ConfigInvalid,
                        detail,
                        since: chrono::Utc::now(),
                    },
                );
                self.emit(&ext, "failed", 0, false);
                None
            }
        }
    }

    fn clear_pseudo_record(&self) {
        let ext = ExtensionId::mcp(CONFIG_PSEUDO_ID);
        if self.ledger.drop_record(&ext) {
            tracing::info!("config/mcp.toml parses again; clearing its pseudo-record");
            self.emit(&ext, "removed", 0, false);
        }
    }

    /// Everything the declaration says, brought in line with reality.
    ///
    /// The per-server work runs concurrently (`join_all`) — each future takes
    /// its own per-extension mutex — which is the daemon's analogue of waiting
    /// for pending servers before the first turn: the first request after boot
    /// sees a connected or a `Failed` record, never a pending one (§5).
    async fn reconcile_all_inner(&self) {
        let Some(declared) = self.read_declaration() else {
            return; // unparseable: last-good set kept, diff skipped
        };
        *self.declared.lock_or_recover() = declared.clone();

        let known: Vec<ExtensionId> = self
            .ledger
            .list()
            .into_iter()
            .filter(|r| r.id.kind == ExtensionKind::Mcp && r.id.name != CONFIG_PSEUDO_ID)
            .map(|r| r.id)
            .collect();

        // Declarations that vanished: T0–T4 with **no file write**, then the
        // record is dropped. The bit left with the block, which is the correct
        // outcome for "the declaration is the toggle".
        let gone: Vec<String> = known
            .iter()
            .filter(|id| !declared.servers.contains_key(&id.name))
            .map(|id| id.name.clone())
            .collect();

        let mut work: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>>> =
            Vec::new();
        for name in gone {
            work.push(Box::pin(async move { self.drop_declaration(&name).await }));
        }
        for (name, cfg) in declared.servers.clone() {
            let defaults = declared.defaults.clone();
            work.push(Box::pin(async move {
                self.reconcile_declared(&name, &cfg, &defaults).await;
            }));
        }
        futures_util::future::join_all(work).await;

        let orphans = self
            .ledger
            .audit_kind(&self.tool_registry, ExtensionKind::Mcp);
        if !orphans.is_empty() {
            tracing::error!(
                orphans = ?orphans,
                "registered MCP tools with no ledger record — the gate fails open for these"
            );
        }
    }

    /// One declared server's diff arm. The key is **presence + `enabled` bit +
    /// `config_fingerprint`** (§10 case 15).
    async fn reconcile_declared(&self, name: &str, cfg: &McpServerConfig, defaults: &McpDefaults) {
        let _lock = self.lock_for(name).await;
        let ext = ExtensionId::mcp(name);
        let wants = cfg.is_enabled();
        let fingerprint = config_fingerprint(cfg);

        let Some(record) = self.ledger.record(&ext) else {
            // A block that appeared, or boot.
            if wants {
                self.load_under_cas(name, cfg, defaults).await;
            } else {
                // Not the bare `continue` this used to be: a disabled server is
                // **enumerable**, with its toggle off, rather than invisible.
                self.ledger
                    .upsert(&ext, false, ExtensionState::Disabled);
                self.ledger
                    .set_config_fingerprint(&ext, Some(fingerprint));
                tracing::info!(server = %name, "MCP server disabled by config");
                self.emit(&ext, "disabled", 0, false);
            }
            return;
        };

        self.ledger.set_disposition(&ext, wants);
        let changed = record.config_fingerprint.as_deref() != Some(fingerprint.as_str());

        match (&record.state, wants) {
            // The bit flipped to false — the watcher path, where the write *is*
            // the trigger and there is nothing for W to do.
            (ExtensionState::Enabled | ExtensionState::Failed { .. }, false) => {
                self.run_disable(name, WithdrawalCause::Watcher).await;
            }
            (ExtensionState::Disabled, true) => {
                self.load_under_cas(name, cfg, defaults).await;
            }
            // §3.4 trigger 2: for a `Failed` record the fingerprint half is
            // consulted **regardless** of §13 Q9 — it is what makes "edit the
            // declaration to retry" work without retrying every failed server
            // on any edit.
            (ExtensionState::Failed { .. }, true) if changed => {
                self.load_under_cas(name, cfg, defaults).await;
            }
            // A changed block on a live server takes effect at the next
            // `reload`/`enable`. Auto-applying it is §13 Q9 and is **not**
            // adopted here.
            (ExtensionState::Enabled, true) if changed => {
                tracing::info!(server = %name, "declaration changed; reload to apply");
            }
            (ExtensionState::Disabled, false) => {
                // Keep the stored fingerprint current so a later bit flip is
                // the only thing the diff has to notice.
                self.ledger
                    .set_config_fingerprint(&ext, Some(fingerprint));
            }
            _ => {}
        }
    }

    /// **T5-gone.** The block is no longer in `mcp.toml`.
    ///
    /// T0–T4 exactly as a disable, then — instead of T5 — the record is
    /// dropped and `ExtensionStateChanged { state: "removed" }` is emitted.
    /// **No file write is attempted**: there is no block to write into, and the
    /// writer's mandatory re-parse would reject a synthesized one.
    async fn drop_declaration(&self, name: &str) {
        let _lock = self.lock_for(name).await;
        let ext = ExtensionId::mcp(name);
        let Some(record) = self.ledger.record(&ext) else {
            return;
        };
        let generation = record.generation;

        match record.state {
            ExtensionState::Enabled => {
                if let Transition::Took(_) =
                    self.ledger
                        .begin(&ext, ExtensionState::Disabling, Some(WithdrawalCause::DeclarationGone))
                {
                    self.t1(&ext, WithdrawalCause::DeclarationGone);
                    self.t3(&ext).await;
                    self.t4(name).await;
                }
            }
            // A pre-reaper `Failed{Crashed}` still owns load N's residue. There
            // is no T0 and no drain on this exit — the gate has refused since
            // `mark_failed` — but the residue must still come down, or the
            // child would outlive the declaration.
            ExtensionState::Failed { .. } => {
                self.t1(&ext, WithdrawalCause::Crash);
                self.t4(name).await;
            }
            _ => {}
        }

        self.ledger.drop_record(&ext);
        self.handles.lock_or_recover().remove(name);
        tracing::info!(extension = %ext, "MCP declaration removed; record dropped");
        self.emit(&ext, "removed", generation, false);
    }

    /// E0's CAS then the load path. The caller holds the mutex.
    async fn load_under_cas(&self, name: &str, cfg: &McpServerConfig, defaults: &McpDefaults) {
        let ext = ExtensionId::mcp(name);
        let Transition::Took(generation) = self.ledger.begin(&ext, ExtensionState::Enabling, None)
        else {
            tracing::debug!(extension = %ext, "load refused: the record is not in a loadable state");
            return;
        };
        // E-PRE — before E1, before anything is built, on any handle the map
        // still holds.
        self.e_pre(&ext).await;
        self.load(name, cfg, defaults, generation).await;
    }

    /// T0 → T5 for a server that is up (or holds a residue). The caller holds
    /// the mutex and has already done whatever W this path requires.
    async fn run_disable(&self, name: &str, cause: WithdrawalCause) -> Vec<String> {
        let ext = ExtensionId::mcp(name);
        let Transition::Took(_) = self
            .ledger
            .begin(&ext, ExtensionState::Disabling, Some(cause))
        else {
            // `Disabled` × `disable` — nothing to do, the row already says so.
            return Vec::new();
        };

        let mut warnings = Vec::new();
        self.t1(&ext, cause);
        let stragglers = self.t3(&ext).await;
        if stragglers > 0 {
            warnings.push(format!("torn down with {stragglers} call(s) in flight"));
        }
        if let Some(pending) = self.t4(name).await {
            warnings.push(pending);
        }
        self.ledger.commit(&ext, ExtensionState::Disabled);
        tracing::info!(extension = %ext, cause = ?cause, "MCP server disabled");
        self.emit_record(&ext);
        warnings
    }
}

/// Decrements the live-reader count however the task ends — a natural exit
/// when `disconnect` drops the sender, or a cancellation.
struct ReaderGuard(Arc<AtomicUsize>);

impl Drop for ReaderGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

// ============================================================================
// The trait
// ============================================================================

#[async_trait::async_trait]
impl ExtensionSupervisor for McpSupervisor {
    /// W (`enabled = true`) then E0–E5. `200` even when bring-up fails: the
    /// write succeeded and the intent is durable; the connection outcome is a
    /// separate fact in the body.
    async fn enable(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError> {
        self.guard_kind(id)?;
        let name = &id.name;
        let Some((cfg, defaults)) = self.declaration(name) else {
            return Err(self.unknown_or_unreadable(id));
        };

        let _lock = self.lock_for(name).await;

        // W — skipped when the bit already matches (§8: "W is skipped — the bit
        // is already `true`"), which is also what keeps a redundant enable off
        // a read-only file.
        if self.ledger.record(id).map(|r| r.disposition.0) != Some(true) {
            self.write_bit(name, true)?;
        }
        self.ledger.set_disposition(id, true);

        // E0 — a CAS failure (enable on `Enabled`) returns the current row and
        // never reaches E-PRE. It is deliberately **not** a reload: a redundant
        // load leaks a capability provider and duplicate index edges.
        let Transition::Took(generation) = self.ledger.begin(id, ExtensionState::Enabling, None)
        else {
            return self.row(id, Vec::new());
        };
        self.e_pre(id).await;
        self.load(name, &cfg, &defaults, generation).await;
        self.row(id, Vec::new())
    }

    /// W (`enabled = false`) then T0–T5.
    async fn disable(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError> {
        self.guard_kind(id)?;
        let name = &id.name;
        if !self.is_declared(name) {
            return Err(self.unknown_or_unreadable(id));
        }

        let _lock = self.lock_for(name).await;

        // Symmetric with `enable`: W is skipped when the bit is already
        // `false`, so a redundant disable against a read-only file is the
        // `200`-current no-op §3.3.1 promises, not a `500`.
        if self.ledger.record(id).map(|r| r.disposition.0) != Some(false) {
            self.write_bit(name, false)?;
        }
        if self.ledger.record(id).is_none() {
            // Declared but never reconciled — nothing is loaded, so the bit is
            // the whole transition. The row is the one the next reconcile would
            // have produced.
            self.ledger.upsert(id, false, ExtensionState::Disabled);
            self.emit(id, "disabled", 0, false);
            return self.row(id, Vec::new());
        }
        self.ledger.set_disposition(id, false);

        let warnings = self.run_disable(name, WithdrawalCause::Disable).await;
        self.row(id, warnings)
    }

    /// T0–T4 then E0–E5 under one hold of the mutex, bit untouched, **no W**.
    ///
    /// From `Enabled` and `Failed{*}` only. From `Failed{*}` there is no T0 and
    /// no drain: E0's CAS → E-PRE on any held handle → E1–E5 (§3.4.1).
    async fn reload(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError> {
        self.guard_kind(id)?;
        let name = &id.name;
        let Some((cfg, defaults)) = self.declaration(name) else {
            return Err(self.unknown_or_unreadable(id));
        };

        let _lock = self.lock_for(name).await;
        let Some(record) = self.ledger.record(id) else {
            return Err(ExtensionError::NotLoaded);
        };

        match record.state {
            ExtensionState::Enabled => {
                // T0 records `Reload` as the pending cause, so a call landing
                // in the T0–E5 window is refused as *being reloaded right now*
                // — not *being turned off*, which would be false for a verb
                // that ends `Enabled`.
                let Transition::Took(_) = self.ledger.begin(
                    id,
                    ExtensionState::Disabling,
                    Some(WithdrawalCause::Reload),
                ) else {
                    return Err(ExtensionError::NotLoaded);
                };
                self.t1(id, WithdrawalCause::Reload);
                self.t3(id).await;
                self.t4(name).await;
                // The `Disabling → Enabling` CAS: T5's dedup-clear and emit do
                // not run on this path — the verb has not ended yet.
            }
            ExtensionState::Failed { .. } => {}
            _ => return Err(ExtensionError::NotLoaded),
        }

        let Transition::Took(generation) = self.ledger.begin(id, ExtensionState::Enabling, None)
        else {
            return self.row(id, Vec::new());
        };
        self.e_pre(id).await;
        self.load(name, &cfg, &defaults, generation).await;
        self.row(id, Vec::new())
    }

    async fn reconcile(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError> {
        self.guard_kind(id)?;
        let Some(declared) = self.read_declaration() else {
            return Err(ExtensionError::StoreUnreadable(
                self.config_path.display().to_string(),
            ));
        };
        *self.declared.lock_or_recover() = declared.clone();

        match declared.servers.get(&id.name) {
            Some(cfg) => {
                self.reconcile_declared(&id.name, cfg, &declared.defaults)
                    .await;
                self.row(id, Vec::new())
            }
            None => {
                self.drop_declaration(&id.name).await;
                Err(ExtensionError::NotFound(id.clone()))
            }
        }
    }

    async fn reconcile_all(&self) {
        self.reconcile_all_inner().await;
    }

    async fn list(&self) -> Vec<ExtensionRecord> {
        self.ledger
            .list()
            .into_iter()
            .filter(|r| r.id.kind == ExtensionKind::Mcp)
            .collect()
    }

    /// **§3.5.** Close every live connection on the way out. Nothing tore these
    /// down before, and rmcp's child cleanup does not fire on `process::exit`,
    /// so MCP children could outlive the daemon.
    ///
    /// State is left alone: the process is going away and the ledger is memory
    /// only.
    async fn shutdown_all(&self) {
        let live: Vec<String> = self.handles.lock_or_recover().keys().cloned().collect();
        let count = live.len();
        for name in live {
            let _lock = self.lock_for(&name).await;
            self.t4(&name).await;
        }
        tracing::info!(
            servers = count,
            // Each reader ends when its client's sender is dropped, which the
            // `disconnect` above has just done; a non-zero count here is one
            // that has not been polled since.
            readers_running = self.readers_running(),
            "MCP servers shut down"
        );
    }
}

impl McpSupervisor {
    fn guard_kind(&self, id: &ExtensionId) -> Result<(), ExtensionError> {
        if id.kind == ExtensionKind::Mcp {
            Ok(())
        } else {
            Err(ExtensionError::UnsupportedForKind)
        }
    }

    /// A name this supervisor cannot act on.
    ///
    /// While the store does not parse, a name that is not in the last-good set
    /// is `409 store_unreadable` rather than `404`: it may well be declared in
    /// the bytes on disk — nobody can tell — and the pseudo-record row itself
    /// has no disposition anyone can read, so no verb on it may take a
    /// transition (§4).
    ///
    /// A **live** server is a different case and deliberately not routed here:
    /// its bit *was* read, from the last-good parse, so its `disable` runs W
    /// like any other, W fails against the unparseable file, and that is the
    /// write-first `500` with nothing torn down (§8).
    fn unknown_or_unreadable(&self, id: &ExtensionId) -> ExtensionError {
        if self
            .ledger
            .record(&ExtensionId::mcp(CONFIG_PSEUDO_ID))
            .is_some()
        {
            ExtensionError::StoreUnreadable(self.config_path.display().to_string())
        } else {
            ExtensionError::NotFound(id.clone())
        }
    }
}

// ============================================================================
// Client config
// ============================================================================

fn build_client_config(
    server_name: &str,
    server_cfg: &McpServerConfig,
    defaults: &McpDefaults,
) -> Result<McpClientConfig, MissingEnv> {
    let request_timeout = Duration::from_secs(
        server_cfg
            .request_timeout_secs()
            .unwrap_or(defaults.request_timeout_secs),
    );

    let transport = match server_cfg {
        McpServerConfig::Stdio {
            command,
            args,
            env,
            cwd,
            ..
        } => TransportKind::Stdio {
            command: command.clone(),
            args: args.clone(),
            env: env.clone(),
            cwd: cwd.clone(),
        },
        McpServerConfig::Http {
            url,
            auth,
            extra_headers,
            ..
        } => TransportKind::Http {
            url: url.clone(),
            // Re-resolved on every load, which is how a rotated credential
            // takes effect without a restart (§3.3 E2).
            auth: resolve_http_auth(server_name, auth.as_ref())?,
            extra_headers: extra_headers.clone(),
        },
    };

    Ok(McpClientConfig {
        server_name: server_name.to_string(),
        transport,
        client_info: Implementation {
            name: "openalpaca-mcp".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            ..Default::default()
        },
        request_timeout,
        max_reconnect_attempts: defaults.max_reconnect_attempts,
        reconnect_backoff_ms: defaults.reconnect_backoff_ms,
    })
}

/// Resolve `bearer_env` / `api_key_env` from the process environment.
///
/// A missing variable is refused **up front** with the key's name, so the row
/// reads `Failed{NeedsConfig{missing: [VAR]}}` and the GUI can name what to
/// set. This is deliberately stricter than the reference design, which attempts
/// the connection and warns: for a daemon nobody is watching, the
/// classification that names the missing key is the better one (§4.2, X-31).
fn resolve_http_auth(
    server_name: &str,
    auth: Option<&HttpAuthConfig>,
) -> Result<Option<openalpaca_mcp::HttpAuth>, MissingEnv> {
    use openalpaca_mcp::HttpAuth;
    match auth {
        None => Ok(None),
        Some(HttpAuthConfig::Bearer { bearer }) => Ok(Some(HttpAuth::Bearer(bearer.clone()))),
        Some(HttpAuthConfig::BearerEnv { bearer_env }) => match std::env::var(bearer_env) {
            Ok(val) => Ok(Some(HttpAuth::Bearer(val))),
            Err(_) => Err(MissingEnv {
                var: bearer_env.clone(),
                message: format!(
                    "missing env var '{bearer_env}' for bearer_env on server '{server_name}'"
                ),
            }),
        },
        Some(HttpAuthConfig::ApiKey {
            api_key_header,
            api_key_env,
        }) => match std::env::var(api_key_env) {
            Ok(val) => Ok(Some(HttpAuth::ApiKey {
                header: api_key_header.clone(),
                value: val,
            })),
            Err(_) => Err(MissingEnv {
                var: api_key_env.clone(),
                message: format!(
                    "missing env var '{api_key_env}' for api_key_env on server '{server_name}'"
                ),
            }),
        },
    }
}

fn content_hash(contents: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(contents.as_bytes()))
}

/// `lock().unwrap_or_else(|p| p.into_inner())` without repeating it fifteen
/// times — the workspace's poison-recovery convention.
trait LockOrRecover<T> {
    fn lock_or_recover(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockOrRecover<T> for StdMutex<T> {
    fn lock_or_recover(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[cfg(test)]
mod tests;
