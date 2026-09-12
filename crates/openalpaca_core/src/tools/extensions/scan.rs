//! **T1 step 3** — the dependent scan (design §3.2 T1, §7.3).
//!
//! S4 moment 3: the withdrawal the owner is looking at. It runs *inside* the
//! unpublish sequence, so every path that runs T1 fires it — the route, the
//! watcher's `reconcile_all`, `deny`, `reload`, the §3.6 crash reaper and
//! §3.7's server-driven list change — and it is deliberately **never deduped**:
//! one transition, one announcement.
//!
//! It lives in `openalpaca_core` because both supervisors need it and neither
//! is upstream of the other: `McpSupervisor` is in `apps/openalpacad`,
//! `PluginManager` in `openalpaca_plugins`.

use std::collections::BTreeSet;

use crate::agent::AgentRegistry;
use crate::events::SystemEvent;
use crate::orchestrator::skill::catalog::SkillCatalog;
use crate::tools::ToolRegistry;

use super::{ExtensionId, ExtensionState, WithdrawalCause};

/// What T1 step 1 — and, for a plugin, T2 step 1 — just tombstoned.
///
/// Not the `capability_index`: by the time the scan runs, T1 has removed the
/// withdrawn keys and the empty ones are gone, so the index has nothing left to
/// attribute. The index is consulted only to classify each *hit* as total or
/// partial (design §7.3).
#[derive(Debug, Clone, Default)]
pub struct WithdrawnSet {
    /// The tombstoned capabilities — T1 step 1's per-tool `provides_capabilities`
    /// plus T2 step 1's virtual capabilities.
    pub capabilities: BTreeSet<String>,
    /// The withdrawn tool **names**, which the legacy `tools.allow` reading
    /// matches on (design §6.2 #10, #12).
    pub tools: Vec<String>,
}

impl WithdrawnSet {
    /// Step 3 fires only on a non-empty set, so a second, idempotent pass over
    /// an extension whose tools are already gone announces nothing.
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty() && self.tools.is_empty()
    }

    pub fn add_capabilities<I, S>(&mut self, caps: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.capabilities.extend(caps.into_iter().map(Into::into));
    }

    pub fn add_tool(&mut self, name: impl Into<String>) {
        self.tools.push(name.into());
    }
}

/// The handles the scan reads. Both are `Option` because `PluginManager` holds
/// them that way and a test harness may hold neither.
pub struct DependentScan<'a> {
    pub registry: &'a ToolRegistry,
    pub agents: Option<&'a AgentRegistry>,
    pub skills: Option<&'a SkillCatalog>,
    /// The daemon's default lane, `{local_user_id}:gui` — where the cron notice
    /// is written (design §7.3 step 1).
    pub notice_lane: &'a str,
}

/// What the scan found, returned so a caller can assert on it without a bus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanOutcome {
    pub affected_templates: Vec<String>,
    pub affected_skills: Vec<String>,
    pub affected_cron_skills: Vec<String>,
}

/// A classified-but-not-yet-announced scan — what `reload` carries from T1 to
/// its outcome (§3.4.1).
#[derive(Debug, Clone, Default)]
pub struct PendingScan {
    pub withdrawn: WithdrawnSet,
    pub outcome: ScanOutcome,
}

impl DependentScan<'_> {
    /// Classify **and** announce in one step — every path but `reload`.
    pub fn run(
        &self,
        ext: &ExtensionId,
        state: &ExtensionState,
        cause: WithdrawalCause,
        withdrawn: &WithdrawnSet,
        suppress_cron_notice: bool,
    ) -> ScanOutcome {
        let pending = self.classify(withdrawn);
        self.announce(ext, state, cause, &pending, suppress_cron_notice);
        pending.outcome
    }

    /// **Classify only.** Which templates and skills stopped resolving, read
    /// against the index T1 has *just* emptied.
    ///
    /// Split from [`announce`](Self::announce) for one caller: `reload` must
    /// classify at T1 — after the load has re-registered everything, nothing
    /// reads as lost — but publish only once the outcome is known, so a reload
    /// that ends `Enabled` can drop its cron notice (§3.4.1, the design's
    /// option (a)).
    pub fn classify(&self, withdrawn: &WithdrawnSet) -> PendingScan {
        if withdrawn.is_empty() {
            return PendingScan::default();
        }
        let mut outcome = ScanOutcome {
            affected_templates: self.affected_templates(withdrawn),
            ..ScanOutcome::default()
        };
        let (skills, cron) = self.affected_skills(withdrawn);
        outcome.affected_skills = skills;
        outcome.affected_cron_skills = cron;
        PendingScan {
            withdrawn: withdrawn.clone(),
            outcome,
        }
    }

    /// **Announce.** The one un-deduplicated `warn!` and the one
    /// [`SystemEvent::ExtensionCapabilityWithdrawn`].
    ///
    /// `state` is the record's state at the transition — `Disabling` on the
    /// route/watcher/deny paths, `Failed{Crashed,..}` from the reaper and the
    /// residue exits, `Enabled` from §3.7, the *outcome* on a deferred reload —
    /// and `cause` is what the wording is keyed on, never the state.
    ///
    /// `suppress_cron_notice` empties `affected_cron_skills` **on the event**:
    /// it is how §3.4.1's *"a reload that ends `Enabled` fires no cron notice"*
    /// is implemented, since the dispatcher's rule stays §7.3 step 2's verbatim
    /// — post when `affected_cron_skills` is non-empty.
    pub fn announce(
        &self,
        ext: &ExtensionId,
        state: &ExtensionState,
        cause: WithdrawalCause,
        pending: &PendingScan,
        suppress_cron_notice: bool,
    ) {
        // Nothing was withdrawn: a second, idempotent pass announces nothing —
        // one transition, one announcement (§7.3).
        if pending.withdrawn.is_empty() {
            return;
        }
        let (withdrawn, outcome) = (&pending.withdrawn, &pending.outcome);

        if outcome.affected_templates.is_empty() && outcome.affected_skills.is_empty() {
            tracing::debug!(
                extension = %ext,
                capabilities = withdrawn.capabilities.len(),
                tools = withdrawn.tools.len(),
                "extension capabilities withdrawn with no dependents"
            );
        } else {
            // Never deduped: one transition, one announcement (design §7.3).
            tracing::warn!(
                extension = %ext,
                cause = ?cause,
                state = state.word(),
                templates = ?outcome.affected_templates,
                skills = ?outcome.affected_skills,
                cron_skills = ?outcome.affected_cron_skills,
                "{ext}: {} — dependent templates and skills no longer resolve",
                cause.wording(ext, detail_of(state)),
            );
        }

        let announced_cron = if suppress_cron_notice {
            Vec::new()
        } else {
            outcome.affected_cron_skills.clone()
        };

        self.registry
            .extensions()
            .publish(SystemEvent::ExtensionCapabilityWithdrawn {
                extension: ext.clone(),
                state: state.clone(),
                cause,
                capabilities: withdrawn.capabilities.iter().cloned().collect(),
                tools: withdrawn.tools.clone(),
                affected_templates: outcome.affected_templates.clone(),
                affected_skills: outcome.affected_skills.clone(),
                affected_cron_skills: announced_cron,
                notice_lane: self.notice_lane.to_string(),
                timestamp: chrono::Utc::now(),
            });
    }

    /// Templates that just stopped resolving: one of their declared
    /// `capabilities` is in the withdrawn set **and** has no surviving provider.
    /// A capability another extension still serves is partial loss, which the
    /// next surface assembly announces (§7.2) — it did not stop resolving.
    fn affected_templates(&self, withdrawn: &WithdrawnSet) -> Vec<String> {
        let Some(agents) = self.agents else {
            return Vec::new();
        };
        let mut hits: Vec<String> = agents
            .list_templates()
            .into_iter()
            .filter(|t| {
                t.frontmatter
                    .capabilities
                    .iter()
                    .any(|cap| self.total_loss(withdrawn, cap))
            })
            .map(|t| t.frontmatter.id)
            .collect();
        hits.sort();
        hits.dedup();
        hits
    }

    /// Skills that became unsatisfiable, and the cron-scheduled subset.
    ///
    /// Both frontmatter branches, one predicate (design §6.2 #10, #12): the
    /// `requires_capabilities` branch is "at least one required capability
    /// wholly withheld"; the legacy `tools.allow` branch is the same rule read
    /// at name level — unsatisfiable iff **every** allowed name is withdrawn.
    fn affected_skills(&self, withdrawn: &WithdrawnSet) -> (Vec<String>, Vec<String>) {
        let Some(catalog) = self.skills else {
            return (Vec::new(), Vec::new());
        };
        let mut affected = Vec::new();
        let mut cron = Vec::new();
        for (id, entry) in catalog.entries_snapshot() {
            let fm = &entry.frontmatter;
            let unsatisfiable = if !fm.requires_capabilities.is_empty() {
                fm.requires_capabilities
                    .iter()
                    .any(|cap| self.total_loss(withdrawn, cap))
            } else if !fm.tools.allow.is_empty() {
                // A hit at all — otherwise a skill whose allow-list is entirely
                // builtins would count as "every name withdrawn" over an empty
                // intersection.
                let touched = fm
                    .tools
                    .allow
                    .iter()
                    .any(|name| contains_name(&withdrawn.tools, name));
                touched && fm.tools.allow.iter().all(|name| self.name_lost(name))
            } else {
                false
            };
            if !unsatisfiable {
                continue;
            }
            if fm.invoke.cron.is_some() {
                cron.push(id.clone());
            }
            affected.push(id);
        }
        affected.sort();
        cron.sort();
        (affected, cron)
    }

    /// A withdrawn capability with no surviving provider.
    fn total_loss(&self, withdrawn: &WithdrawnSet, capability: &str) -> bool {
        withdrawn.capabilities.contains(capability)
            && !self
                .registry
                .resolve_capabilities(&[capability.to_string()], &[])
                .withheld
                .is_empty()
    }

    /// Is this allowed tool name gone, in the §6.2 #12 sense? Owned by a
    /// non-`Enabled` extension, or server-withdrawn under an `Enabled` one.
    /// A name with no ledger owner — a builtin, a typo — counts as satisfiable,
    /// preserving today's silent degrade for unattributed misses.
    fn name_lost(&self, name: &str) -> bool {
        let ledger = self.registry.extensions();
        match ledger.owner_of(name) {
            None => false,
            Some(ext) => match ledger.state(&ext) {
                None => false,
                Some(state) if !state.is_enabled() => true,
                Some(_) => ledger.is_server_withdrawn(&ext, name),
            },
        }
    }
}

fn contains_name(names: &[String], needle: &str) -> bool {
    names.iter().any(|n| n.eq_ignore_ascii_case(needle))
}

/// The `<detail>` `WithdrawalCause::Crash` interpolates into its wording.
fn detail_of(state: &ExtensionState) -> &str {
    match state {
        ExtensionState::Failed { detail, .. } => detail,
        _ => "no detail",
    }
}
