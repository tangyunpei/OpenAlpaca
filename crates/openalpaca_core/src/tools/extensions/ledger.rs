//! `ExtensionLedger` — the shared bookkeeping the whole ENABLE axis turns on.
//!
//! It lives inside [`ToolRegistry`](crate::tools::ToolRegistry) because that is
//! the one object every execution path already holds, and `Clone for
//! ToolRegistry` shares it by `Arc::clone` — which is what makes a deep
//! registry snapshot (a lead agent holds one for the whole run) read *live*
//! state at the gate (design §3.0 Fact 1).
//!
//! It is pure bookkeeping. It never holds a client, a process or a file path.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::sync::mpsc::UnboundedSender;

use super::describe::{Audience, Described};
use super::{
    ContributionKind, Disposition, ExtensionId, ExtensionKind, ExtensionState, FailureReason,
    Moment, WithdrawalCause,
};
use crate::bus::EventBus;
use crate::events::SystemEvent;
use crate::tools::registry::ToolContext;

/// Suppression window for the withheld announcement (design §7.4).
const WARN_DEDUP_WINDOW: Duration = Duration::from_secs(600);
/// LRU cap on the dedup set, swept lazily on insert.
const WARN_DEDUP_CAP: usize = 512;

// ============================================================================
// Call guard
// ============================================================================

/// RAII in-flight counter for one tool call or one out-of-process run.
///
/// `check` increments the counter **before** it reads the state, so a caller
/// that read `Enabled` an instant before the T0 CAS is already counted when
/// T3's drain looks; the other order would let that call slip past the drain
/// and be torn down under (design §3.2 T0/T3).
#[derive(Debug)]
pub struct CallGuard {
    counter: Option<Arc<AtomicUsize>>,
}

impl CallGuard {
    /// The guard an unrecorded extension gets — fail-open, counts nothing
    /// (design §6.2a).
    fn noop() -> Self {
        Self { counter: None }
    }

    fn held(counter: Arc<AtomicUsize>) -> Self {
        Self {
            counter: Some(counter),
        }
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        if let Some(counter) = &self.counter {
            counter.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

// ============================================================================
// Records
// ============================================================================

/// The outcome of a ledger CAS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    /// The CAS took. Carries the record's generation — **bumped** for
    /// `Enabling` (design §3.0 Fact 3 rule 1), unchanged for `Disabling`.
    Took(u64),
    /// The CAS did not take. Carries the state the record is actually in, or
    /// `None` when there is no record at all.
    Refused(Option<ExtensionState>),
}

/// A read-only snapshot of one ledger record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionRecord {
    pub id: ExtensionId,
    pub disposition: Disposition,
    pub state: ExtensionState,
    pub generation: u64,
    pub pending_cause: Option<WithdrawalCause>,
    /// When the record entered its current state (design §8 `since`).
    pub since: DateTime<Utc>,
    /// Retained `ContributionKind::Tool` names — attribution, not a cache of
    /// what the extension would offer (design §3.2 T1).
    pub tools: Vec<String>,
    /// Names the server itself dropped mid-session while staying enabled.
    pub withdrawn_by_server: Vec<String>,
    pub in_flight: usize,
}

#[derive(Debug)]
struct LedgerEntry {
    disposition: bool,
    state: ExtensionState,
    generation: u64,
    pending_cause: Option<WithdrawalCause>,
    since: DateTime<Utc>,
    /// Retained contribution names, keyed by class so a withdrawn resource URI
    /// can be attributed the way a tool name is (design §2.3).
    contributions: BTreeSet<(ContributionKind, String)>,
    /// lowercased name → name as recorded.
    server_withdrawn: BTreeMap<String, String>,
    in_flight: Arc<AtomicUsize>,
}

impl LedgerEntry {
    fn new(disposition: bool, state: ExtensionState) -> Self {
        Self {
            disposition,
            state,
            generation: 0,
            pending_cause: None,
            since: Utc::now(),
            contributions: BTreeSet::new(),
            server_withdrawn: BTreeMap::new(),
            in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn snapshot(&self, id: &ExtensionId) -> ExtensionRecord {
        ExtensionRecord {
            id: id.clone(),
            disposition: Disposition(self.disposition),
            state: self.state.clone(),
            generation: self.generation,
            pending_cause: self.pending_cause,
            since: self.since,
            tools: self
                .contributions
                .iter()
                .filter(|(kind, _)| *kind == ContributionKind::Tool)
                .map(|(_, name)| name.clone())
                .collect(),
            withdrawn_by_server: self.server_withdrawn.values().cloned().collect(),
            in_flight: self.in_flight.load(Ordering::SeqCst),
        }
    }
}

// ============================================================================
// The ledger
// ============================================================================

/// Per-extension state, retained attribution, tombstones, in-flight counters,
/// warn-dedup and the reaper senders (design §5).
#[derive(Default)]
pub struct ExtensionLedger {
    records: DashMap<ExtensionId, LedgerEntry>,
    /// `(kind, lowercased name) → owning extension`. Answers the gate's **miss**
    /// arm after T1 has removed the tool from the registry (design §3.0 Fact 2).
    owners: DashMap<(ContributionKind, String), ExtensionId>,
    /// `capability → {extensions that withdrew it}` — a **set**, because one
    /// capability can legitimately have several providers (design §7.2).
    tombstones: DashMap<String, BTreeSet<ExtensionId>>,
    /// `(ScopeKey, extension, moment) → first announcement`.
    warned: DashMap<(String, ExtensionId, Moment), Instant>,
    /// Installed once by `ToolRegistry::with_event_bus`. A ledger with no bus
    /// logs and returns.
    bus: Option<EventBus>,
    /// Per-kind crash-reaper senders. `OnceLock` so `ToolRegistry::new()` keeps
    /// its arg-free signature and the supervisors register after the registry
    /// exists (design §3.6).
    mcp_reaper: OnceLock<UnboundedSender<(ExtensionId, u64)>>,
    plugin_reaper: OnceLock<UnboundedSender<(ExtensionId, u64)>>,
}

impl ExtensionLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_event_bus(bus: EventBus) -> Self {
        Self {
            bus: Some(bus),
            ..Self::default()
        }
    }

    /// The bus, when one was installed. `None` until C4 wires
    /// `services/tools.rs` — a C1 ledger warns via `tracing` only.
    pub fn bus(&self) -> Option<&EventBus> {
        self.bus.as_ref()
    }

    /// Publish on the ledger's bus, if it has one. A no-op otherwise.
    pub fn publish(&self, event: SystemEvent) {
        if let Some(bus) = &self.bus {
            bus.publish(event);
        }
    }

    // ── The gate ─────────────────────────────────────────────────────────

    /// **THE GATE.** Answer whether this call may reach the extension's
    /// backend, and count it while it runs.
    ///
    /// The order of the three checks is fixed (design §6.2 #1): state →
    /// generation → server-withdrawn, so a stale handle to an extension that is
    /// *again* disabled gets the disabled wording, which is the more useful of
    /// the two.
    ///
    /// * `incarnation` — the generation stamped into the handle the caller
    ///   holds. `None` on the miss arm: a missing name has no handle.
    /// * `ctx` — `None` from `ToolRegistry::execute()`, which has none; the
    ///   dedup scope key then falls back to `"global"` (design §7.4).
    ///
    /// **An `ExtensionId` with no ledger entry resolves to `Allow`** — absence
    /// means *"no supervisor owns this yet"*, not *"disabled"* (design §6.2a).
    pub fn check(
        &self,
        ext: &ExtensionId,
        tool_name: &str,
        incarnation: Option<u64>,
        ctx: Option<&ToolContext>,
    ) -> Result<CallGuard, String> {
        let lowered = tool_name.to_lowercase();
        // Count first, read state second — under one shard guard, so the T0 CAS
        // cannot interleave between the two.
        let (guard, state, generation, pending_cause, server_withdrawn) = {
            let Some(entry) = self.records.get(ext) else {
                return Ok(CallGuard::noop());
            };
            entry.in_flight.fetch_add(1, Ordering::SeqCst);
            (
                CallGuard::held(Arc::clone(&entry.in_flight)),
                entry.state.clone(),
                entry.generation,
                entry.pending_cause,
                entry.server_withdrawn.contains_key(&lowered),
            )
        };

        let (described, state_word, stale) = if !state.is_enabled() {
            (
                state.describe(ext, pending_cause, Audience::Model),
                state.word(),
                false,
            )
        } else if incarnation.is_some_and(|g| g != generation) {
            (
                Described::stale(ext, tool_name, Audience::Model),
                state.word(),
                true,
            )
        } else if server_withdrawn {
            (
                Described::server_withdrawn(ext, tool_name, Audience::Model),
                state.word(),
                false,
            )
        } else {
            return Ok(guard);
        };

        // Blocked: give the count back before announcing.
        drop(guard);
        self.observe(ext, tool_name, state_word, stale, Moment::AttemptedUse, ctx, None);
        Err(described.render_model(Some(tool_name)))
    }

    /// Pre-flight + guard for an out-of-process run — a plugin skill's
    /// `skill/invoke` or a plugin agent's `spawn`/`step` loop, neither of which
    /// enters `ToolRegistry` for the run itself (design §3.2 T3(b)).
    ///
    /// Refuses a run against a non-`Enabled` plugin, and against a *previous
    /// load* of an enabled one, before the first RPC is sent.
    pub fn begin_run(&self, ext: &ExtensionId, generation: u64) -> Result<CallGuard, String> {
        let (guard, state, current_generation, pending_cause) = {
            let Some(entry) = self.records.get(ext) else {
                return Ok(CallGuard::noop());
            };
            entry.in_flight.fetch_add(1, Ordering::SeqCst);
            (
                CallGuard::held(Arc::clone(&entry.in_flight)),
                entry.state.clone(),
                entry.generation,
                entry.pending_cause,
            )
        };

        let (described, stale) = if !state.is_enabled() {
            (state.describe(ext, pending_cause, Audience::Model), false)
        } else if generation != current_generation {
            (Described::stale_run(ext, Audience::Model), true)
        } else {
            return Ok(guard);
        };

        drop(guard);
        self.observe(
            ext,
            &ext.name,
            state.word(),
            stale,
            Moment::AttemptedUse,
            None,
            None,
        );
        Err(described.render_model(None))
    }

    /// Own the exit of an out-of-process run: a run torn down at the drain
    /// deadline fails with the S4 refusal, never with a broken-pipe string
    /// (design §3.2 T3(b), layer 2).
    pub async fn run_scoped<T, F>(&self, ext: &ExtensionId, fut: F) -> Result<T, String>
    where
        F: std::future::Future<Output = Result<T, String>>,
    {
        match fut.await {
            Ok(value) => Ok(value),
            Err(raw) => Err(self.refusal_if_not_enabled(ext, None).unwrap_or(raw)),
        }
    }

    /// The S4 refusal for the extension's *current* state, or `None` when it
    /// reads `Enabled`. The hook the plugin bridges use to rewrite a
    /// `ChannelClosed`/`ProcessCrashed` — and `PluginLoopOutcome::Failed`,
    /// which is not a `Result` — into the §7.1 wording (C3).
    pub fn refusal_if_not_enabled(
        &self,
        ext: &ExtensionId,
        tool: Option<&str>,
    ) -> Option<String> {
        let entry = self.records.get(ext)?;
        if entry.state.is_enabled() {
            return None;
        }
        Some(
            entry
                .state
                .describe(ext, entry.pending_cause, Audience::Model)
                .render_model(tool),
        )
    }

    /// Which extension retains this tool name, whatever the registry holds.
    /// **Case-insensitive**, the way `check_agent_capability` lowercases
    /// (design §6.2 #1, X-23).
    pub fn owner_of(&self, tool_name: &str) -> Option<ExtensionId> {
        self.owner_of_kind(ContributionKind::Tool, tool_name)
    }

    pub fn owner_of_kind(&self, kind: ContributionKind, name: &str) -> Option<ExtensionId> {
        self.owners
            .get(&(kind, name.to_lowercase()))
            .map(|e| e.value().clone())
    }

    /// Is this name flagged as withdrawn by the extension's own server?
    /// Case-insensitive, like `owner_of`.
    pub fn is_server_withdrawn(&self, ext: &ExtensionId, name: &str) -> bool {
        self.records
            .get(ext)
            .is_some_and(|e| e.server_withdrawn.contains_key(&name.to_lowercase()))
    }

    // ── Transitions ──────────────────────────────────────────────────────

    /// The two CASes.
    ///
    /// `begin(ext, Enabling, None)` is **E0**: it increments the record's
    /// generation and returns it. Legal from *absent* (boot), `Disabled`,
    /// `Failed{*}`, `Unapproved{*}` (the approve path) and `Disabling` (the
    /// reload path, under the same mutex hold).
    ///
    /// `begin(ext, Disabling, cause)` is **T0**: it records the verb's
    /// [`WithdrawalCause`] as the record's `pending_cause`, which `describe`
    /// reads so a `reload`'s window is worded *reloading*, never *being turned
    /// off*. Legal from `Enabled` and `Failed{*}`.
    ///
    /// From the instant T0 takes, `check` returns `Blocked` everywhere, in
    /// every snapshot. Everything after is bookkeeping.
    pub fn begin(
        &self,
        ext: &ExtensionId,
        target: ExtensionState,
        cause: Option<WithdrawalCause>,
    ) -> Transition {
        match target {
            ExtensionState::Enabling => {
                use dashmap::mapref::entry::Entry;
                match self.records.entry(ext.clone()) {
                    Entry::Vacant(slot) => {
                        let mut entry = LedgerEntry::new(true, ExtensionState::Enabling);
                        entry.generation = 1;
                        slot.insert(entry);
                        Transition::Took(1)
                    }
                    Entry::Occupied(mut slot) => {
                        let entry = slot.get_mut();
                        let legal = matches!(
                            entry.state,
                            ExtensionState::Disabled
                                | ExtensionState::Failed { .. }
                                | ExtensionState::Unapproved { .. }
                                | ExtensionState::Disabling
                        );
                        if !legal {
                            return Transition::Refused(Some(entry.state.clone()));
                        }
                        entry.generation += 1;
                        entry.state = ExtensionState::Enabling;
                        entry.pending_cause = None;
                        entry.since = Utc::now();
                        Transition::Took(entry.generation)
                    }
                }
            }
            ExtensionState::Disabling => {
                let Some(mut entry) = self.records.get_mut(ext) else {
                    return Transition::Refused(None);
                };
                let legal = matches!(
                    entry.state,
                    ExtensionState::Enabled | ExtensionState::Failed { .. }
                );
                if !legal {
                    return Transition::Refused(Some(entry.state.clone()));
                }
                entry.state = ExtensionState::Disabling;
                entry.pending_cause = cause;
                entry.since = Utc::now();
                Transition::Took(entry.generation)
            }
            other => {
                tracing::error!(
                    extension = %ext,
                    target = other.word(),
                    "begin() takes only Enabling or Disabling"
                );
                Transition::Refused(self.records.get(ext).map(|e| e.state.clone()))
            }
        }
    }

    /// **T5 / E5** — leave a transient state. Succeeds only from `Enabling` or
    /// `Disabling`; clears `pending_cause` and this extension's warn-dedup
    /// entries, so a disable/enable/disable cycle re-announces rather than
    /// being swallowed (design §7.4).
    pub fn commit(&self, ext: &ExtensionId, to: ExtensionState) -> bool {
        let committed = {
            let Some(mut entry) = self.records.get_mut(ext) else {
                return false;
            };
            if !matches!(
                entry.state,
                ExtensionState::Enabling | ExtensionState::Disabling
            ) {
                return false;
            }
            entry.state = to;
            entry.pending_cause = None;
            entry.since = Utc::now();
            true
        };
        self.clear_warned(ext);
        committed
    }

    /// Create or replace a record outright — boot parking, T5-deny and T5-gone,
    /// which store their target state unconditionally under the supervisor's
    /// mutex and are not CASes from `Disabling` (design §3.2 W-deny).
    pub fn upsert(&self, ext: &ExtensionId, disposition: bool, state: ExtensionState) {
        use dashmap::mapref::entry::Entry;
        match self.records.entry(ext.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(LedgerEntry::new(disposition, state));
            }
            Entry::Occupied(mut slot) => {
                let entry = slot.get_mut();
                entry.disposition = disposition;
                entry.state = state;
                entry.pending_cause = None;
                entry.since = Utc::now();
            }
        }
        self.clear_warned(ext);
    }

    /// Update the state of an existing record, leaving the disposition alone.
    /// No-op when there is no record.
    pub fn store_state(&self, ext: &ExtensionId, state: ExtensionState) -> bool {
        let stored = {
            let Some(mut entry) = self.records.get_mut(ext) else {
                return false;
            };
            entry.state = state;
            entry.pending_cause = None;
            entry.since = Utc::now();
            true
        };
        self.clear_warned(ext);
        stored
    }

    /// The owner's persisted toggle, as last read or written by the supervisor.
    pub fn set_disposition(&self, ext: &ExtensionId, disposition: bool) -> bool {
        match self.records.get_mut(ext) {
            Some(mut entry) => {
                entry.disposition = disposition;
                true
            }
            None => false,
        }
    }

    /// T5-gone and `DELETE /v1/extensions/plugin/{id}` — the row disappears.
    pub fn drop_record(&self, ext: &ExtensionId) -> bool {
        let existed = self.records.remove(ext).is_some();
        self.owners.retain(|_, owner| &*owner != ext);
        self.restore(ext);
        existed
    }

    /// **`mark_failed`** — `Enabled → Failed{Crashed, ..}`, guarded twice
    /// (design §3.6).
    ///
    /// It is a no-op from any state other than `Enabled`, so a crash observed
    /// during `Disabling` does not fight the toggle; and a no-op unless
    /// `generation` is the record's current one, so a handle from a previous
    /// load cannot flip the incarnation that replaced it.
    ///
    /// On success it logs the transition itself and sends `(ExtensionId,
    /// generation)` down the kind's reaper channel. It publishes nothing: the
    /// `failed` event is the reaper's until C4 installs the ledger's bus.
    pub fn mark_failed(
        &self,
        ext: &ExtensionId,
        generation: u64,
        reason: FailureReason,
        detail: impl Into<String>,
    ) -> bool {
        let detail = detail.into();
        {
            let Some(mut entry) = self.records.get_mut(ext) else {
                tracing::debug!(extension = %ext, generation, "mark_failed on an unrecorded extension");
                return false;
            };
            if !entry.state.is_enabled() {
                tracing::debug!(
                    extension = %ext,
                    generation,
                    state = entry.state.word(),
                    "mark_failed is a no-op outside Enabled"
                );
                return false;
            }
            if entry.generation != generation {
                tracing::warn!(
                    extension = %ext,
                    generation,
                    current = entry.generation,
                    stale = true,
                    "mark_failed from a previous load ignored"
                );
                return false;
            }
            entry.state = ExtensionState::Failed {
                reason: reason.clone(),
                detail: detail.clone(),
                since: Utc::now(),
            };
            entry.pending_cause = None;
            entry.since = Utc::now();
        }

        tracing::warn!(
            extension = %ext,
            generation,
            reason = reason.word(),
            detail = %detail,
            "extension marked failed"
        );
        self.clear_warned(ext);

        if let Some(tx) = self.reaper(ext.kind)
            && tx.send((ext.clone(), generation)).is_err()
        {
            tracing::warn!(extension = %ext, "crash reaper channel closed");
        }
        true
    }

    /// Register a kind's crash-reaper channel. The slot is write-once; a second
    /// registration is refused and logged.
    pub fn on_crash(&self, kind: ExtensionKind, tx: UnboundedSender<(ExtensionId, u64)>) -> bool {
        let slot = match kind {
            ExtensionKind::Mcp => &self.mcp_reaper,
            ExtensionKind::Plugin => &self.plugin_reaper,
        };
        if slot.set(tx).is_err() {
            tracing::warn!(kind = kind.as_str(), "crash reaper already registered");
            return false;
        }
        true
    }

    fn reaper(&self, kind: ExtensionKind) -> Option<&UnboundedSender<(ExtensionId, u64)>> {
        match kind {
            ExtensionKind::Mcp => self.mcp_reaper.get(),
            ExtensionKind::Plugin => self.plugin_reaper.get(),
        }
    }

    // ── Retained attribution ─────────────────────────────────────────────

    /// **E5** — replace this extension's retained tool names wholesale.
    ///
    /// Never touches `server_withdrawn`: §3.7 step 7 writes the union
    /// `live ∪ server_withdrawn` and relies on the flags surviving the write.
    /// Only `restore` clears them.
    pub fn record_tools<I, S>(&self, ext: &ExtensionId, names: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.record_contributions(ext, ContributionKind::Tool, names);
    }

    /// The general form of [`record_tools`](Self::record_tools) — the same
    /// wholesale replacement for any contribution class (design §2.3).
    pub fn record_contributions<I, S>(&self, ext: &ExtensionId, kind: ContributionKind, names: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let names: Vec<String> = names.into_iter().map(Into::into).collect();

        // Drop the names this extension no longer claims from the owner index.
        let previous: Vec<String> = self
            .records
            .get(ext)
            .map(|e| {
                e.contributions
                    .iter()
                    .filter(|(k, _)| *k == kind)
                    .map(|(_, n)| n.clone())
                    .collect()
            })
            .unwrap_or_default();
        let keep: BTreeSet<String> = names.iter().map(|n| n.to_lowercase()).collect();
        for gone in previous {
            let lowered = gone.to_lowercase();
            if !keep.contains(&lowered) {
                self.owners
                    .remove_if(&(kind, lowered), |_, owner| owner == ext);
            }
        }

        // Displace a *dead* incumbent (design §10 case 13): a live one is
        // skipped by the supervisor before it ever calls this.
        for name in &names {
            let lowered = name.to_lowercase();
            if let Some(incumbent) = self.owner_of_kind(kind, &lowered)
                && &incumbent != ext
                && let Some(mut entry) = self.records.get_mut(&incumbent)
            {
                entry
                    .contributions
                    .retain(|(k, n)| !(*k == kind && n.to_lowercase() == lowered));
                entry.server_withdrawn.remove(&lowered);
            }
            self.owners.insert((kind, lowered), ext.clone());
        }

        if let Some(mut entry) = self.records.get_mut(ext) {
            entry.contributions.retain(|(k, _)| *k != kind);
            for name in names {
                entry.contributions.insert((kind, name));
            }
        }
    }

    /// The retained tool names T1 iterates. Retained through `Disabled` /
    /// `Failed` and replaced wholesale by the next E5.
    pub fn tool_names(&self, ext: &ExtensionId) -> Vec<String> {
        self.contribution_names(ext, ContributionKind::Tool)
    }

    pub fn contribution_names(&self, ext: &ExtensionId, kind: ContributionKind) -> Vec<String> {
        self.records
            .get(ext)
            .map(|e| {
                e.contributions
                    .iter()
                    .filter(|(k, _)| *k == kind)
                    .map(|(_, n)| n.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// §3.7 step 5 — the server itself dropped this name while staying enabled.
    pub fn flag_server_withdrawn(&self, ext: &ExtensionId, name: &str) {
        if let Some(mut entry) = self.records.get_mut(ext) {
            entry
                .server_withdrawn
                .insert(name.to_lowercase(), name.to_string());
        }
    }

    /// §3.7 step 6 — a re-added name is no longer withdrawn.
    pub fn clear_server_withdrawn(&self, ext: &ExtensionId, name: &str) {
        if let Some(mut entry) = self.records.get_mut(ext) {
            entry.server_withdrawn.remove(&name.to_lowercase());
        }
    }

    pub fn server_withdrawn(&self, ext: &ExtensionId) -> Vec<String> {
        self.records
            .get(ext)
            .map(|e| e.server_withdrawn.values().cloned().collect())
            .unwrap_or_default()
    }

    // ── Tombstones ───────────────────────────────────────────────────────

    /// **T1 step 1 / T2 step 1** — record that this extension used to provide
    /// these capabilities, so `resolve_capabilities` can tell *withheld* from
    /// *never existed* (design §7.2).
    pub fn withdraw<I, S>(&self, ext: &ExtensionId, capabilities: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for cap in capabilities {
            self.tombstones
                .entry(cap.into())
                .or_default()
                .insert(ext.clone());
        }
        self.clear_warned(ext);
    }

    /// **E5** — this extension serves again: drop it from every tombstone set
    /// and clear its `server_withdrawn` flags (a fresh load starts with none).
    pub fn restore(&self, ext: &ExtensionId) {
        self.drop_tombstones(ext, None);
        if let Some(mut entry) = self.records.get_mut(ext) {
            entry.server_withdrawn.clear();
        }
        self.clear_warned(ext);
    }

    /// **§3.7 step 6** — per-capability restore. A whole-extension `restore`
    /// would erase the tombstones step 5 just wrote for a tool removed in the
    /// same change.
    pub fn restore_caps<I, S>(&self, ext: &ExtensionId, capabilities: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let only: BTreeSet<String> = capabilities.into_iter().map(Into::into).collect();
        self.drop_tombstones(ext, Some(&only));
        self.clear_warned(ext);
    }

    fn drop_tombstones(&self, ext: &ExtensionId, only: Option<&BTreeSet<String>>) {
        let mut emptied = Vec::new();
        for mut set in self.tombstones.iter_mut() {
            if only.is_some_and(|caps| !caps.contains(set.key())) {
                continue;
            }
            set.value_mut().remove(ext);
            if set.value().is_empty() {
                emptied.push(set.key().clone());
            }
        }
        for key in emptied {
            self.tombstones.remove_if(&key, |_, set| set.is_empty());
        }
    }

    /// Every extension recorded as a provider of this capability.
    pub fn recorded_providers(&self, capability: &str) -> Vec<ExtensionId> {
        self.tombstones
            .get(capability)
            .map(|s| s.value().iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The recorded providers that are currently unable to serve `capability`:
    /// non-`Enabled`, **or** `Enabled` but holding the name in
    /// `server_withdrawn`. The `bool` says which — `true` for the second, whose
    /// attribution names the owner as *still enabled* (design §7.2).
    pub fn blocked_providers(&self, capability: &str) -> Vec<(ExtensionId, bool)> {
        self.recorded_providers(capability)
            .into_iter()
            .filter_map(|ext| {
                let flag = {
                    let entry = self.records.get(&ext)?;
                    if !entry.state.is_enabled() {
                        Some(false)
                    } else if entry
                        .server_withdrawn
                        .contains_key(&capability.to_lowercase())
                    {
                        Some(true)
                    } else {
                        None
                    }
                }?;
                Some((ext, flag))
            })
            .collect()
    }

    // ── Reads ────────────────────────────────────────────────────────────

    pub fn record(&self, ext: &ExtensionId) -> Option<ExtensionRecord> {
        self.records.get(ext).map(|e| e.snapshot(ext))
    }

    pub fn state(&self, ext: &ExtensionId) -> Option<ExtensionState> {
        self.records.get(ext).map(|e| e.state.clone())
    }

    pub fn generation(&self, ext: &ExtensionId) -> Option<u64> {
        self.records.get(ext).map(|e| e.generation)
    }

    /// What **T3's drain** waits on: tool calls *and* out-of-process runs.
    pub fn in_flight(&self, ext: &ExtensionId) -> usize {
        self.records
            .get(ext)
            .map(|e| e.in_flight.load(Ordering::SeqCst))
            .unwrap_or(0)
    }

    pub fn list(&self) -> Vec<ExtensionRecord> {
        let mut rows: Vec<ExtensionRecord> = self
            .records
            .iter()
            .map(|e| e.value().snapshot(e.key()))
            .collect();
        rows.sort_by(|a, b| (a.id.kind, &a.id.name).cmp(&(b.id.kind, &b.id.name)));
        rows
    }

    /// Every registered extension tool with no ledger record — the audit that
    /// keeps §6.2a's fail-open from becoming a bypass. The supervisors call it
    /// at the end of boot and log at `error` if it is non-empty.
    pub fn audit(&self, registry: &crate::tools::ToolRegistry) -> Vec<String> {
        let mut orphans: Vec<String> = registry
            .iter_registered_tools()
            .filter_map(|(name, tool)| tool.extension_id().map(|ext| (name, ext)))
            .filter(|(_, ext)| !self.records.contains_key(ext))
            .map(|(name, ext)| format!("{name} (no ledger record for {ext})"))
            .collect();
        orphans.sort();
        orphans
    }

    // ── Warn dedup (design §7.4) ─────────────────────────────────────────

    /// Announce a withholding, deduped per `(ScopeKey, extension, moment)` for
    /// ten minutes. The **error** is never suppressed — only the announcement.
    ///
    /// `scope_override` is `Moment::ScheduledSkip`'s skill id: a cron fire is a
    /// distinct unattended event and never dedupes (design §6.2 #13).
    pub fn note_withheld(
        &self,
        ext: &ExtensionId,
        subject: &str,
        moment: Moment,
        ctx: Option<&ToolContext>,
        scope_override: Option<&str>,
    ) {
        let state_word = self
            .records
            .get(ext)
            .map(|e| e.state.word())
            .unwrap_or("unrecorded");
        self.observe(ext, subject, state_word, false, moment, ctx, scope_override);
    }

    #[allow(clippy::too_many_arguments)]
    fn observe(
        &self,
        ext: &ExtensionId,
        subject: &str,
        state_word: &str,
        stale: bool,
        moment: Moment,
        ctx: Option<&ToolContext>,
        scope_override: Option<&str>,
    ) {
        let scope = scope_override
            .map(str::to_string)
            .unwrap_or_else(|| scope_key(ctx));
        let agent_id = ctx.and_then(|c| c.agent_id.clone());
        let task_id = ctx.and_then(|c| c.task_id.clone());

        if self.note_first(&scope, ext, moment) {
            tracing::warn!(
                extension = %ext,
                state = state_word,
                tool = subject,
                ?agent_id,
                ?task_id,
                stale,
                "extension capability withheld"
            );
        } else {
            tracing::debug!(
                extension = %ext,
                state = state_word,
                tool = subject,
                ?agent_id,
                ?task_id,
                stale,
                "extension capability withheld (deduped)"
            );
        }
    }

    /// First occurrence in scope? `ScheduledSkip` is exempt and always is.
    fn note_first(&self, scope: &str, ext: &ExtensionId, moment: Moment) -> bool {
        if moment == Moment::ScheduledSkip {
            return true;
        }
        let key = (scope.to_string(), ext.clone(), moment);
        let now = Instant::now();

        if let Some(seen) = self.warned.get(&key)
            && now.duration_since(*seen.value()) < WARN_DEDUP_WINDOW
        {
            return false;
        }

        // Lazy sweep, then LRU trim.
        self.warned
            .retain(|_, seen| now.duration_since(*seen) < WARN_DEDUP_WINDOW);
        while self.warned.len() >= WARN_DEDUP_CAP {
            let oldest = self
                .warned
                .iter()
                .min_by_key(|e| *e.value())
                .map(|e| e.key().clone());
            match oldest {
                Some(key) => {
                    self.warned.remove(&key);
                }
                None => break,
            }
        }
        self.warned.insert(key, now);
        true
    }

    fn clear_warned(&self, ext: &ExtensionId) {
        self.warned.retain(|(_, recorded, _), _| recorded != ext);
    }

    /// Live entries in the dedup set — one per announcement actually made in a
    /// scope. The observable C1 uses in place of C4's event.
    pub fn warned_count(&self) -> usize {
        self.warned.len()
    }
}

/// `ctx.task_id` → `ctx.request_id` → `ctx.agent_id` → `"global"` (design §7.4).
fn scope_key(ctx: Option<&ToolContext>) -> String {
    ctx.and_then(|c| {
        c.task_id
            .clone()
            .or_else(|| c.request_id.map(|r| r.to_string()))
            .or_else(|| c.agent_id.clone())
    })
    .unwrap_or_else(|| "global".to_string())
}
