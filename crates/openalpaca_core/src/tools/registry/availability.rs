//! **The one predicate** — is a skill's declared requirement still served?
//!
//! Design §10 case 3, §6.2 #10/#11/#12/#13, §7.2. A skill is *unsatisfiable*
//! iff **at least one** of its requirements is **wholly withheld**:
//!
//! * `requires_capabilities` arm — a capability whose index lookup is empty
//!   *and* whose tombstone set is non-empty (§7.2 `withheld`). A capability
//!   another provider still serves is `partially_withheld`: reported, never
//!   gating.
//! * legacy `tools.allow` arm (a skill whose `requires_capabilities` is empty)
//!   — read at **name** level through `owner_of`, because plugin tool names are
//!   not capabilities (`manager.rs:839`) and only `owner_of` works for both
//!   kinds. Unsatisfiable iff **every** allowed name is owned by a non-`Enabled`
//!   extension or is server-withdrawn under an `Enabled` one; a name with no
//!   ledger owner — a builtin, a typo — counts as satisfiable, which is what
//!   preserves today's silent degrade for unattributed misses on upgrade.
//!
//! The same value answers invocation (§6.2 #10, #11), router candidacy and the
//! catalog listings (#12), the cron skip (#13) and the `/slash` tier (§7.5), so
//! a skill the catalog hides is never one `/slash` or `invoke_skill` would run.

use super::ToolRegistry;
use crate::middleware::skill::SkillFrontmatter;
use crate::tools::extensions::{Audience, Described, ExtensionId};

#[cfg(test)]
#[path = "availability_tests.rs"]
mod tests;

/// Which arm of the frontmatter a requirement came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequirementKind {
    /// `requires_capabilities`.
    Capability,
    /// The legacy `tools.allow` name list.
    Tool,
}

impl RequirementKind {
    pub fn plural(&self) -> &'static str {
        match self {
            Self::Capability => "capabilities",
            Self::Tool => "tools",
        }
    }
}

/// One recorded provider that cannot currently serve a requirement, with the
/// §7.1 row that says why. Rendered from the one wording table (X-18), so the
/// refusal, the GUI row and the gate's tool result cannot disagree.
#[derive(Debug, Clone)]
pub struct WithheldProvider {
    pub extension: ExtensionId,
    /// `true` when the extension is still `Enabled` and the *server* withdrew
    /// the name (§3.7): the attribution names the owner as **still enabled**,
    /// never as disabled.
    pub server_withdrawn: bool,
    pub described: Described,
}

/// One requirement and the providers that cannot serve it. `providers` is
/// empty only in the degenerate case of a tombstoned capability whose recorded
/// providers have all since become `Enabled` again without re-registering it —
/// still unsatisfiable, just unattributable.
#[derive(Debug, Clone)]
pub struct WithheldSubject {
    pub subject: String,
    pub providers: Vec<WithheldProvider>,
}

impl WithheldSubject {
    fn lines(&self, out: &mut String) {
        if self.providers.is_empty() {
            out.push_str(&format!(
                "'{}': no installed extension currently provides it.\n",
                self.subject
            ));
            return;
        }
        for provider in &self.providers {
            out.push_str(&format!(
                "'{}': {}\n",
                self.subject,
                provider.described.render_model(None)
            ));
        }
    }
}

/// What the one predicate found for one skill.
#[derive(Debug, Clone)]
pub struct SkillRequirements {
    pub kind: RequirementKind,
    /// Wholly withheld — **this is the gate**. Non-empty ⇒ refuse, drop from
    /// router candidacy and from `<available_skills>`, skip the cron fire.
    pub withheld: Vec<WithheldSubject>,
    /// A provider went but another still serves it. Reported in chat when the
    /// skill was explicitly invoked; **never** gating (§7.2, §10 case 3).
    pub partial: Vec<WithheldSubject>,
}

impl SkillRequirements {
    fn none(kind: RequirementKind) -> Self {
        Self {
            kind,
            withheld: Vec::new(),
            partial: Vec::new(),
        }
    }

    /// `CapabilityOracle::is_satisfiable` — *no* requirement is wholly
    /// withheld. `partial` never enters this.
    pub fn is_satisfiable(&self) -> bool {
        self.withheld.is_empty()
    }

    /// The S4 refusal: names the skill, the requirement, the extension and the
    /// remedy (§7.5), rendered from the §7.1 table.
    pub fn refusal(&self, skill: &str) -> String {
        let mut out = format!(
            "Skill '{skill}' cannot run — required {} are unavailable.\n",
            self.kind.plural()
        );
        for subject in &self.withheld {
            subject.lines(&mut out);
        }
        out.trim_end().to_string()
    }

    /// The chat-visible prefix a *partially* withheld skill's result carries,
    /// because the user explicitly invoked it (§10 case 3). `None` when
    /// nothing was lost.
    pub fn chat_prefix(&self) -> Option<String> {
        if self.partial.is_empty() {
            return None;
        }
        let mut out = format!(
            "Note: this skill ran without some of the {} it declared.\n",
            self.kind.plural()
        );
        for subject in &self.partial {
            subject.lines(&mut out);
        }
        out.push('\n');
        Some(out)
    }

    /// `(extension, subject)` for every wholly-withheld requirement — what the
    /// cron skip announces with `Moment::ScheduledSkip` (§6.2 #13).
    pub fn attributions(&self) -> Vec<(&ExtensionId, &str)> {
        self.withheld
            .iter()
            .flat_map(|s| {
                s.providers
                    .iter()
                    .map(move |p| (&p.extension, s.subject.as_str()))
            })
            .collect()
    }
}

/// The availability question the [`crate::orchestrator::skill_catalog::SkillCatalog`]
/// asks without holding a registry (design §6.2 #12).
///
/// Implemented by [`ToolRegistry`], never by the ledger: *withheld* is "the
/// `capability_index` lookup is empty **and** the tombstone set is non-empty",
/// and the ledger holds only the second half — a third, never-disabled provider
/// could still serve the capability, which only the index knows.
pub trait CapabilityOracle: Send + Sync {
    fn is_satisfiable(&self, frontmatter: &SkillFrontmatter) -> bool;
}

impl CapabilityOracle for ToolRegistry {
    fn is_satisfiable(&self, frontmatter: &SkillFrontmatter) -> bool {
        self.skill_requirements(frontmatter).is_satisfiable()
    }
}

impl ToolRegistry {
    /// The one predicate, with the attribution every consumer of it renders.
    ///
    /// Dispatches on the frontmatter exactly as the resolvers do: the
    /// capability arm when `requires_capabilities` is non-empty, otherwise the
    /// legacy `tools.allow` name arm, otherwise nothing to lose.
    pub fn skill_requirements(&self, frontmatter: &SkillFrontmatter) -> SkillRequirements {
        if !frontmatter.requires_capabilities.is_empty() {
            return self.capability_requirements(&frontmatter.requires_capabilities);
        }
        if !frontmatter.tools.allow.is_empty() {
            return self.name_requirements(&frontmatter.tools.allow);
        }
        SkillRequirements::none(RequirementKind::Capability)
    }

    /// The capability arm. Classifies exactly as `resolve_capabilities` does —
    /// *withheld* when the index lookup is empty and the tombstone set is not,
    /// *partial* when both are non-empty — without building the definitions,
    /// because the router asks this once per catalog entry per message.
    fn capability_requirements(&self, capabilities: &[String]) -> SkillRequirements {
        let mut out = SkillRequirements::none(RequirementKind::Capability);
        for capability in capabilities {
            let resolved_any = self
                .capability_index
                .get(capability)
                .is_some_and(|names| !names.value().is_empty());
            if self.extensions.recorded_providers(capability).is_empty() {
                // Never provided by anything the ledger knows: `unknown`, a
                // `debug!` in `resolve_capabilities` and nothing here — a typo
                // and a withdrawal are indistinguishable today (§7.2).
                continue;
            }
            let providers: Vec<WithheldProvider> = self
                .extensions
                .blocked_providers(capability)
                .into_iter()
                .filter_map(|(extension, server_withdrawn)| {
                    self.withheld_provider(capability, extension, server_withdrawn)
                })
                .collect();
            let subject = WithheldSubject {
                subject: capability.clone(),
                providers,
            };
            if resolved_any {
                out.partial.push(subject);
            } else {
                out.withheld.push(subject);
            }
        }
        out
    }

    /// The legacy `tools.allow` arm, read at name level through `owner_of`.
    /// Total loss — the gate — is **every** allowed name withheld; one live
    /// name (a builtin, a typo, a tool of a still-enabled extension) keeps the
    /// skill satisfiable and demotes the rest to `partial`.
    fn name_requirements(&self, names: &[String]) -> SkillRequirements {
        let mut out = SkillRequirements::none(RequirementKind::Tool);
        let mut blocked = Vec::new();
        let mut any_live = false;
        for name in names {
            match self.withheld_name(name) {
                Some(provider) => blocked.push(WithheldSubject {
                    subject: name.clone(),
                    providers: vec![provider],
                }),
                None => any_live = true,
            }
        }
        if blocked.is_empty() {
            return out;
        }
        if any_live {
            out.partial = blocked;
        } else {
            out.withheld = blocked;
        }
        out
    }

    /// Is this **name** withheld, and by whom? `None` for a name with no ledger
    /// owner (fail-open, §6.2a) and for one whose owner is `Enabled` and still
    /// offers it. The single classification the legacy arm and
    /// `announce_withheld_names` share.
    pub(super) fn withheld_name(&self, name: &str) -> Option<WithheldProvider> {
        let extension = self.extensions.owner_of(name)?;
        let state = self.extensions.state(&extension)?;
        if !state.is_enabled() {
            return self.withheld_provider(name, extension, false);
        }
        if self.extensions.is_server_withdrawn(&extension, name) {
            return self.withheld_provider(name, extension, true);
        }
        None
    }

    fn withheld_provider(
        &self,
        subject: &str,
        extension: ExtensionId,
        server_withdrawn: bool,
    ) -> Option<WithheldProvider> {
        let described = if server_withdrawn {
            Described::server_withdrawn(&extension, subject, Audience::Model)
        } else {
            self.extensions
                .describe_state(&extension, Audience::Model)?
        };
        Some(WithheldProvider {
            extension,
            server_withdrawn,
            described,
        })
    }

    /// The attributed refusal for a **withdrawn plugin contribution** — a skill
    /// or an agent template a disabled plugin used to provide (§10 case 5(a)).
    /// The tombstone knows which plugin; the ledger knows what state it is in.
    pub fn withdrawn_contribution_refusal(
        &self,
        noun: &str,
        name: &str,
        plugin_id: &str,
    ) -> String {
        let extension = ExtensionId::plugin(plugin_id.to_string());
        match self.extensions.describe_state(&extension, Audience::Model) {
            Some(described) => format!(
                "{noun} '{name}' is provided by plugin '{plugin_id}': {}",
                described.render_model(None)
            ),
            None => format!(
                "{noun} '{name}' is provided by plugin '{plugin_id}', which is no longer loaded."
            ),
        }
    }
}
