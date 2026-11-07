//! In-memory agent registry with template + instance model.
//!
//! Templates define agent capabilities and are loaded at startup.
//! Instances are ephemeral runtime objects spawned from templates on demand.
//! Singleton templates (e.g. lead_agent) enforce max 1 active instance.
//!
//! Templates are kept in three layers — bundled, file, plugin — and every
//! reader sees one resolved view of them (subagent design §2.4 L-1,
//! `tasks/subagent-brief-design.md`).

use super::subagent::{AgentStatus, SubAgent};
use super::template::{AgentSource, AgentTemplate};
use crate::bundled::ShippedCopy;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Outcome of a `destroy_instance()` call, allowing callers to emit
/// the correct lifecycle status ("destroyed" vs "idle").
#[derive(Debug, Clone, PartialEq)]
pub enum DestroyOutcome {
    /// Non-singleton instance was removed from registry entirely.
    Removed,
    /// Singleton instance was reset to Idle (still in registry for reuse).
    ResetToIdle,
    /// Instance was not found in the registry.
    NotFound,
}

/// The layer a template comes from (subagent design §2.4 L-1).
///
/// The resolved view takes each id from the file layer, else the bundled
/// one, else the plugin one: a person's file overrides the built-in
/// template of its id, and a plugin's template fills only an id no other
/// layer holds (`register_template` refuses it otherwise).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateLayer {
    /// The templates the binary carries.
    Bundled,
    /// The templates read from the config directory's `agents/`.
    File,
    /// The templates plugins contribute.
    Plugin,
}

/// Whether a resolved template can be spawned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateState {
    Ready,
    /// A file holds the id but cannot be used (L-4): it does not parse, or
    /// a hot reload found it naming an unknown capability. The id leaves
    /// every list, a spawn of it is refused naming the file, and no other
    /// layer's template of the id stands in. `reason` reads after the path:
    /// "does not parse (…)".
    Withheld {
        path: PathBuf,
        reason: String,
    },
    /// A file holds the id and says `disabled: true` (§2.4 L-4a, §7 O-1
    /// (b)): the id leaves every list, a spawn of it is refused naming the
    /// file, and no other layer's template of the id stands in — neither
    /// the bundled one nor a plugin's.
    Disabled {
        path: PathBuf,
    },
}

/// Why a definition of an id is not used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotUsedReason {
    /// Another file claims the same id and comes first by file name.
    Duplicate,
    /// The file does not parse, and a file that parses holds the id it
    /// names (L-4: a broken file withholds only an id nothing else holds).
    DoesNotParse,
    /// The file is byte-identical to a version OpenAlpaca shipped, so it is
    /// set aside — not read, not touched — and the bundled template of its
    /// id runs (subagent design §2.2 V-1).
    UneditedCopy(ShippedCopy),
    /// The bundled template of the id, under a file of the person's.
    Overridden,
    /// A plugin's template of the id, refused at registration because
    /// another layer holds the id (N-PLUG-6; §2.3 P-2).
    PluginRefused,
}

impl NotUsedReason {
    /// The word the template listing carries (R-1).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Duplicate => "duplicate",
            Self::DoesNotParse => "does_not_parse",
            Self::UneditedCopy(_) => "unedited_copy",
            Self::Overridden => "overridden",
            Self::PluginRefused => "plugin_refused",
        }
    }
}

/// A definition of a template's id that the resolved view does not use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotUsed {
    pub layer: TemplateLayer,
    pub path: Option<PathBuf>,
    pub reason: NotUsedReason,
    /// The plugin, for a `PluginRefused` entry.
    pub plugin: Option<String>,
}

/// Where a resolved template comes from, as `AgentRegistry::provenance`
/// answers it. `AgentSource` keeps its one meaning, where execution runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateProvenance {
    pub layer: TemplateLayer,
    /// The file a file-layer template was read from, when one is known.
    pub path: Option<PathBuf>,
    pub not_used: Vec<NotUsed>,
    pub state: TemplateState,
    pub warnings: Vec<String>,
}

impl TemplateProvenance {
    /// Whether a file of the person's stands over the bundled template of
    /// its id.
    pub fn overrides_bundled(&self) -> bool {
        self.not_used
            .iter()
            .any(|entry| entry.reason == NotUsedReason::Overridden)
    }
}

/// What the file layer holds under an id: a template, a file that cannot
/// be used (L-4), or a file that switches the id off (L-4a).
enum FileHeld {
    Template(Box<AgentTemplate>),
    Withheld { reason: String },
    Disabled,
}

/// An id of the file layer, with the file it was read from and the other
/// files of the same id it does not use.
struct FileTemplate {
    held: FileHeld,
    path: Option<PathBuf>,
    not_used: Vec<NotUsed>,
    /// What the rescan found worth saying about the file (W-1).
    warnings: Vec<String>,
}

impl FileTemplate {
    fn template(&self) -> Option<&AgentTemplate> {
        match &self.held {
            FileHeld::Template(template) => Some(template),
            FileHeld::Withheld { .. } | FileHeld::Disabled => None,
        }
    }
}

/// The file layer, built whole and handed to
/// `AgentRegistry::replace_file_layer`.
#[derive(Default)]
pub struct FileLayer {
    templates: HashMap<String, FileTemplate>,
    /// The unedited copies set aside, by the bundled id each would hide
    /// (V-1): listed on that id's row, whichever layer it resolves from.
    set_aside: HashMap<String, Vec<NotUsed>>,
}

impl FileLayer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a template read from `path`. Returns false, leaving the layer
    /// as it was, when the id is already in it: the first one added holds
    /// the id.
    pub fn insert(&mut self, template: AgentTemplate, path: Option<PathBuf>) -> bool {
        let id = template.frontmatter.id.clone();
        self.hold(id, path, FileHeld::Template(Box::new(template)))
    }

    /// Hold `id` for a file that cannot be used, `reason` saying why after
    /// its path ("does not parse (…)"). Returns false, as `insert` does,
    /// when the id is already in the layer.
    pub fn withhold(&mut self, id: String, path: PathBuf, reason: String) -> bool {
        self.hold(id, Some(path), FileHeld::Withheld { reason })
    }

    /// Hold `id` for a file that switches it off (L-4a). Returns false, as
    /// `insert` does, when the id is already in the layer.
    pub fn disable(&mut self, id: String, path: PathBuf) -> bool {
        self.hold(id, Some(path), FileHeld::Disabled)
    }

    /// Whether the layer holds `id`, usable, withheld or switched off.
    pub fn holds(&self, id: &str) -> bool {
        self.templates.contains_key(id)
    }

    /// The file that holds `id`, when one is known.
    pub fn path_of(&self, id: &str) -> Option<&Path> {
        self.templates
            .get(id)
            .and_then(|entry| entry.path.as_deref())
    }

    /// Record a file of `id` that the layer does not use. Ignored when the
    /// layer does not hold the id.
    pub fn note_not_used(&mut self, id: &str, path: PathBuf, reason: NotUsedReason) {
        if let Some(entry) = self.templates.get_mut(id) {
            entry.not_used.push(NotUsed {
                layer: TemplateLayer::File,
                path: Some(path),
                reason,
                plugin: None,
            });
        }
    }

    /// Record a file set aside as an unedited copy of a shipped version of
    /// the bundled template `id` (V-1). The layer does not hold `id` for it.
    pub fn set_aside(&mut self, id: &str, path: PathBuf, copy: ShippedCopy) {
        self.set_aside
            .entry(id.to_string())
            .or_default()
            .push(NotUsed {
                layer: TemplateLayer::File,
                path: Some(path),
                reason: NotUsedReason::UneditedCopy(copy),
                plugin: None,
            });
    }

    /// The files set aside, as `(id, path, copy)`, in no particular order.
    pub fn set_aside_files(&self) -> impl Iterator<Item = (&str, &Path, ShippedCopy)> {
        self.set_aside.iter().flat_map(|(id, entries)| {
            entries
                .iter()
                .filter_map(move |entry| match (&entry.path, entry.reason) {
                    (Some(path), NotUsedReason::UneditedCopy(copy)) => {
                        Some((id.as_str(), path.as_path(), copy))
                    }
                    _ => None,
                })
        })
    }

    /// Add a warning to the file that holds `id` (W-1). Ignored when the
    /// layer does not hold the id.
    pub fn warn(&mut self, id: &str, warning: String) {
        if let Some(entry) = self.templates.get_mut(id) {
            entry.warnings.push(warning);
        }
    }

    fn hold(&mut self, id: String, path: Option<PathBuf>, held: FileHeld) -> bool {
        match self.templates.entry(id) {
            Entry::Occupied(_) => false,
            Entry::Vacant(slot) => {
                slot.insert(FileTemplate {
                    held,
                    path,
                    not_used: Vec::new(),
                    warnings: Vec::new(),
                });
                true
            }
        }
    }
}

/// The three layers and the view resolved from them, under one mutex.
#[derive(Default)]
struct TemplateLayers {
    bundled: HashMap<String, AgentTemplate>,
    file: FileLayer,
    plugin: HashMap<String, AgentTemplate>,
    /// The plugins whose template of an id `register_template` refused
    /// because another layer held it, by id (L-1): listed on the holder's
    /// row until the plugin is withdrawn.
    refused: HashMap<String, Vec<String>>,
    /// What every reader sees: recomputed by `resolve` after each change
    /// to a layer, never written by itself.
    resolved: HashMap<String, AgentTemplate>,
}

impl TemplateLayers {
    /// Recompute the resolved view: each id from the file layer, else the
    /// bundled one, else the plugin one. An id the file layer withholds or
    /// switches off is in no layer's view: nothing stands in for the
    /// person's file (L-4, L-4a).
    fn resolve(&mut self) {
        self.resolved.clear();
        self.resolved.extend(
            self.plugin
                .iter()
                .map(|(id, template)| (id.clone(), template.clone())),
        );
        self.resolved.extend(
            self.bundled
                .iter()
                .map(|(id, template)| (id.clone(), template.clone())),
        );
        for (id, entry) in &self.file.templates {
            match entry.template() {
                Some(template) => {
                    self.resolved.insert(id.clone(), template.clone());
                }
                None => {
                    self.resolved.remove(id);
                }
            }
        }
    }

    /// Whether any layer holds `id`, a withheld or switching file's
    /// included.
    fn holds(&self, id: &str) -> bool {
        self.resolved.contains_key(id) || self.file.holds(id)
    }

    /// The layer the resolved template of `id` comes from.
    fn layer_of(&self, id: &str) -> Option<TemplateLayer> {
        if self.file.holds(id) {
            Some(TemplateLayer::File)
        } else if self.bundled.contains_key(id) {
            Some(TemplateLayer::Bundled)
        } else if self.plugin.contains_key(id) {
            Some(TemplateLayer::Plugin)
        } else {
            None
        }
    }

    /// The resolved templates in L-1's order: the file layer's, then the
    /// bundled layer's, then the plugins', each by id.
    fn by_layer(&self) -> Vec<AgentTemplate> {
        let rank = |id: &str| match self.layer_of(id) {
            Some(TemplateLayer::File) => 0,
            Some(TemplateLayer::Bundled) => 1,
            _ => 2,
        };
        let mut templates: Vec<&AgentTemplate> = self.resolved.values().collect();
        templates.sort_by(|a, b| {
            let (a, b) = (&a.frontmatter.id, &b.frontmatter.id);
            rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
        });
        templates.into_iter().cloned().collect()
    }
}

/// Whether two templates say the same thing: a resync of an idle instance
/// built from an equal template would change nothing but its version.
pub(crate) fn same_definition(a: &AgentTemplate, b: &AgentTemplate) -> bool {
    a.frontmatter == b.frontmatter && a.body == b.body && a.sections == b.sections
}

/// The idle instance a template's id answers to between runs (L-1a).
fn idle_instance(template: &AgentTemplate) -> SubAgent {
    let id = &template.frontmatter.id;
    let mut agent = template.to_subagent(id, "");
    agent.status = AgentStatus::Idle;
    agent.current_task = None;
    agent
}

/// Internal wrapper that pairs a SubAgent instance with its config version
/// for optimistic locking on config updates.
struct RegisteredAgent {
    agent: SubAgent,
    config_version: u64,
    is_singleton: bool,
}

/// Registry for tracking agent templates and runtime instances.
///
/// Templates are immutable blueprints, kept by layer and resolved on read.
/// Instances are spawned on demand and destroyed when tasks complete.
pub struct AgentRegistry {
    /// Agent templates by layer, each keyed by template_id (e.g.
    /// "code_agent"), and the resolved view every reader sees.
    templates: Mutex<TemplateLayers>,
    /// Active agent instances keyed by instance_id (e.g. "code_agent::a1b2c3d4").
    instances: Mutex<HashMap<String, RegisteredAgent>>,
    /// Templates withdrawn with a plugin: `template_id → plugin_id`
    /// (extension design §10 case 5(a)). Kept so `spawn_subagent` naming one
    /// answers *"provided by plugin 'x', which is disabled"* rather than the
    /// bare "template not found". The live map stays scrubbed exactly as today.
    template_tombstones: Mutex<HashMap<String, String>>,
    /// The idle instances `sync_template_instances` materialised, by
    /// instance id, each with the template it was last built from (L-1a).
    /// An instance not in it — a legacy `.toml` agent's — is never removed.
    synced: Mutex<HashMap<String, AgentTemplate>>,
    /// Held by `agent::file_layer::rescan_file_layer` from its read of the
    /// directory to its instance sync, so one rescan runs at a time (L-3).
    rescan: Mutex<()>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            templates: Mutex::new(TemplateLayers::default()),
            instances: Mutex::new(HashMap::new()),
            template_tombstones: Mutex::new(HashMap::new()),
            synced: Mutex::new(HashMap::new()),
            rescan: Mutex::new(()),
        }
    }

    // ── Lock helpers ─────────────────────────────────────────────────

    fn lock_templates(&self) -> std::sync::MutexGuard<'_, TemplateLayers> {
        match self.templates.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!("AgentRegistry templates mutex poisoned — recovering");
                poisoned.into_inner()
            }
        }
    }

    fn lock_instances(&self) -> std::sync::MutexGuard<'_, HashMap<String, RegisteredAgent>> {
        match self.instances.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!("AgentRegistry instances mutex poisoned — recovering");
                poisoned.into_inner()
            }
        }
    }

    // ── Template methods ─────────────────────────────────────────────

    /// Register an agent template. Returns false if any layer already holds
    /// the id, a withheld file or one that switches the id off included.
    ///
    /// A plugin's template (`AgentSource::Plugin`) goes into the plugin
    /// layer, any other into the file layer with no path.
    pub fn register_template(&self, template: AgentTemplate) -> bool {
        let mut templates = self.lock_templates();
        let id = template.frontmatter.id.clone();
        if templates.holds(&id) {
            if let AgentSource::Plugin { plugin_id, .. } = &template.source {
                let refused = templates.refused.entry(id).or_default();
                if !refused.contains(plugin_id) {
                    refused.push(plugin_id.clone());
                }
            }
            return false;
        }
        // A template that came back invalidates its own tombstone.
        self.lock_template_tombstones().remove(&id);
        if matches!(template.source, AgentSource::Plugin { .. }) {
            templates.plugin.insert(id, template);
        } else {
            templates.file.insert(template, None);
        }
        templates.resolve();
        true
    }

    /// Replace the file layer whole: an id the new layer does not hold
    /// leaves it, and the bundled template of that id, if any, is resolved
    /// again.
    pub fn replace_file_layer(&self, layer: FileLayer) {
        let mut templates = self.lock_templates();
        self.forget_tombstones(layer.templates.keys());
        templates.file = layer;
        templates.resolve();
    }

    /// Replace the bundled layer, then make the idle instances follow it
    /// (L-1a). Of two templates with one id the first is kept.
    pub fn set_bundled_layer(&self, bundled: Vec<AgentTemplate>) {
        let mut layer = HashMap::new();
        for template in bundled {
            layer
                .entry(template.frontmatter.id.clone())
                .or_insert(template);
        }
        {
            let mut templates = self.lock_templates();
            self.forget_tombstones(layer.keys());
            templates.bundled = layer;
            templates.resolve();
        }
        self.sync_template_instances();
    }

    /// Whether the bundled layer holds `template_id`, whatever stands over
    /// it.
    pub fn is_bundled(&self, template_id: &str) -> bool {
        self.lock_templates().bundled.contains_key(template_id)
    }

    /// How many templates the bundled layer holds.
    pub fn bundled_count(&self) -> usize {
        self.lock_templates().bundled.len()
    }

    /// The ids a file holds with no template under them — withheld (L-4)
    /// or switched off (L-4a) — sorted: in no list of templates, but on the
    /// template listing's rows (R-1).
    pub fn held_off_ids(&self) -> Vec<String> {
        let templates = self.lock_templates();
        let mut ids: Vec<String> = templates
            .file
            .templates
            .iter()
            .filter(|(_, entry)| entry.template().is_none())
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// The ids a file switches off (L-4a), sorted.
    pub fn disabled_ids(&self) -> Vec<String> {
        let templates = self.lock_templates();
        let mut ids: Vec<String> = templates
            .file
            .templates
            .iter()
            .filter(|(_, entry)| matches!(entry.held, FileHeld::Disabled))
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Where the template resolved under `template_id` comes from, or None
    /// when no layer holds the id.
    ///
    /// `not_used` lists, after the file layer's own entries (duplicates,
    /// broken files), the bundled template a file overrides, the unedited
    /// copies set aside for the id (V-1) and the plugins refused it (P-2).
    /// An id a file switches off (L-4a) lists no bundled template as
    /// overridden — the file runs nothing in its place — and lists a plugin's
    /// template of the id, registered before the file came, as refused.
    pub fn provenance(&self, template_id: &str) -> Option<TemplateProvenance> {
        let templates = self.lock_templates();
        let mut provenance = if let Some(entry) = templates.file.templates.get(template_id) {
            let (state, mut warnings) = match (&entry.held, &entry.path) {
                (FileHeld::Withheld { reason }, Some(path)) => (
                    TemplateState::Withheld {
                        path: path.clone(),
                        reason: reason.clone(),
                    },
                    vec![format!("{} {reason}", path.display())],
                ),
                (FileHeld::Disabled, Some(path)) => {
                    (TemplateState::Disabled { path: path.clone() }, Vec::new())
                }
                _ => (TemplateState::Ready, Vec::new()),
            };
            warnings.extend(entry.warnings.iter().cloned());
            let mut not_used = entry.not_used.clone();
            let disabled = matches!(entry.held, FileHeld::Disabled);
            if templates.bundled.contains_key(template_id) && !disabled {
                not_used.push(NotUsed {
                    layer: TemplateLayer::Bundled,
                    path: None,
                    reason: NotUsedReason::Overridden,
                    plugin: None,
                });
            }
            if disabled
                && let Some(AgentSource::Plugin { plugin_id, .. }) =
                    templates.plugin.get(template_id).map(|t| &t.source)
                && !templates
                    .refused
                    .get(template_id)
                    .is_some_and(|plugins| plugins.contains(plugin_id))
            {
                not_used.push(NotUsed {
                    layer: TemplateLayer::Plugin,
                    path: None,
                    reason: NotUsedReason::PluginRefused,
                    plugin: Some(plugin_id.clone()),
                });
            }
            TemplateProvenance {
                layer: TemplateLayer::File,
                path: entry.path.clone(),
                not_used,
                state,
                warnings,
            }
        } else {
            TemplateProvenance {
                layer: templates.layer_of(template_id)?,
                path: None,
                not_used: Vec::new(),
                state: TemplateState::Ready,
                warnings: Vec::new(),
            }
        };
        if let Some(set_aside) = templates.file.set_aside.get(template_id) {
            let mut set_aside = set_aside.clone();
            set_aside.sort_by(|a, b| a.path.cmp(&b.path));
            provenance.not_used.extend(set_aside);
        }
        for plugin in templates.refused.get(template_id).into_iter().flatten() {
            provenance.not_used.push(NotUsed {
                layer: TemplateLayer::Plugin,
                path: None,
                reason: NotUsedReason::PluginRefused,
                plugin: Some(plugin.clone()),
            });
        }
        Some(provenance)
    }

    /// Get a template by id.
    pub fn get_template(&self, template_id: &str) -> Option<AgentTemplate> {
        self.lock_templates().resolved.get(template_id).cloned()
    }

    /// List all resolved templates, sorted by id (L-1): the lead's
    /// `<agents>` block and both spawn descriptions read the same from start
    /// to start.
    pub fn list_templates(&self) -> Vec<AgentTemplate> {
        let mut templates: Vec<AgentTemplate> =
            self.lock_templates().resolved.values().cloned().collect();
        templates.sort_by(|a, b| a.frontmatter.id.cmp(&b.frontmatter.id));
        templates
    }

    /// List all resolved templates in the order a lead is chosen in (G-1):
    /// the person's files, then the bundled layer, then plugins, each by id.
    pub fn list_templates_by_layer(&self) -> Vec<AgentTemplate> {
        self.lock_templates().by_layer()
    }

    /// Find templates that have a given capability, in
    /// `list_templates_by_layer`'s order.
    pub fn find_templates_by_capability(&self, capability_name: &str) -> Vec<AgentTemplate> {
        self.lock_templates()
            .by_layer()
            .into_iter()
            .filter(|t| t.frontmatter.capabilities.iter().any(|s| s == capability_name))
            .collect()
    }

    /// Number of registered templates.
    pub fn template_count(&self) -> usize {
        self.lock_templates().resolved.len()
    }

    /// Remove a template by id from the plugin layer, else from the file
    /// layer. Returns true if one was removed. A bundled template is never
    /// removed.
    ///
    /// The plugin layer first: its one caller is the plugin manager's
    /// rollback of a template it has just registered, and a file of the
    /// person's over the same id — a rescan may have put one there
    /// meanwhile — is not the plugin's to take. The file layer is otherwise
    /// replaced whole by its rescan (L-3).
    pub fn remove_template(&self, template_id: &str) -> bool {
        let mut templates = self.lock_templates();
        let removed = templates.plugin.remove(template_id).is_some()
            || templates.file.templates.remove(template_id).is_some();
        if removed {
            templates.resolve();
        }
        removed
    }

    /// Remove a plugin's template and leave a tombstone behind
    /// (extension design §10 case 5(a)) — **only when the template under
    /// this id is that plugin's own** (`AgentSource::Plugin { plugin_id }`).
    /// Returns whether one was removed.
    ///
    /// N-PLUG-6: a plugin whose `agent/info` named a shipped template's id
    /// (`writing_agent`) was refused at registration, yet its unload removed
    /// the shipped template by id. A template another source holds is not
    /// this plugin's to take, and no tombstone is written for it: a
    /// tombstone records a withdrawal that happened, and attributing a live
    /// incumbent's id to the plugin would mislead a later miss. For the same
    /// reason a withdrawal leaves no tombstone when another layer still
    /// holds the id — a person's file over it (a spawn of that template
    /// failing for another reason must not name the plugin).
    pub fn remove_plugin_template(&self, template_id: &str, plugin_id: &str) -> bool {
        let (removed, still_held) = {
            let mut templates = self.lock_templates();
            let owned = templates.plugin.get(template_id).is_some_and(|template| {
                matches!(
                    &template.source,
                    AgentSource::Plugin { plugin_id: owner, .. } if owner == plugin_id
                )
            });
            let removed = owned && templates.plugin.remove(template_id).is_some();
            if removed {
                templates.resolve();
            }
            (removed, templates.holds(template_id))
        };
        if removed && !still_held {
            self.lock_template_tombstones()
                .insert(template_id.to_string(), plugin_id.to_string());
        } else {
            tracing::debug!(
                template = template_id,
                plugin = plugin_id,
                "the template under this id is not the plugin's; left in place"
            );
        }
        removed
    }

    /// Expire every tombstone this plugin left behind, returning how many were
    /// dropped.
    ///
    /// The uninstall path calls it (GAP-24). Until one existed nothing expired
    /// a tombstone that was not immediately superseded by the same template
    /// coming back, so a plugin whose directory had gone kept telling
    /// `spawn_subagent` that a template was *"withdrawn with plugin 'x'"* for a
    /// plugin that no longer existed.
    pub fn clear_plugin_tombstones(&self, plugin_id: &str) -> usize {
        self.forget_plugin_refusals(plugin_id);
        let mut tombstones = self.lock_template_tombstones();
        let before = tombstones.len();
        tombstones.retain(|_, owner| owner != plugin_id);
        before - tombstones.len()
    }

    /// Forget that this plugin's templates were refused (L-1's
    /// `plugin_refused`): the plugin manager calls it when it withdraws the
    /// plugin, so a disabled plugin is no longer listed on a holder's row.
    pub fn forget_plugin_refusals(&self, plugin_id: &str) {
        let mut templates = self.lock_templates();
        templates.refused.retain(|_, plugins| {
            plugins.retain(|plugin| plugin != plugin_id);
            !plugins.is_empty()
        });
    }

    /// The plugin a withdrawn template belonged to, if one is recorded.
    pub fn template_tombstone(&self, template_id: &str) -> Option<String> {
        self.lock_template_tombstones().get(template_id).cloned()
    }

    /// Drop the tombstones of ids a layer now holds: a template that came
    /// back invalidates its own tombstone, as in `register_template`.
    fn forget_tombstones<'a>(&self, ids: impl Iterator<Item = &'a String>) {
        let mut tombstones = self.lock_template_tombstones();
        for id in ids {
            tombstones.remove(id);
        }
    }

    fn lock_template_tombstones(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        match self.template_tombstones.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!("AgentRegistry template tombstones mutex poisoned — recovering");
                poisoned.into_inner()
            }
        }
    }

    // ── Instance methods ─────────────────────────────────────────────

    /// Spawn a new agent instance from a template.
    ///
    /// For **non-singleton** templates: creates a fresh instance with a unique ID
    /// (`{template_id}::{8-char-uuid}`) and marks it Busy.
    ///
    /// For **singleton** templates: if an idle instance already exists, claims it.
    /// If a busy instance exists, returns an error. If no instance exists, creates one
    /// with `id == template_id` for backward compatibility.
    pub fn spawn_instance(&self, template_id: &str, task_id: String) -> Result<SubAgent, String> {
        // Extract what we need from the template, then drop the templates lock
        // before acquiring instances, to avoid holding both locks simultaneously.
        let (is_singleton, template_clone) = {
            let templates = self.lock_templates();
            let Some(template) = templates.resolved.get(template_id) else {
                return Err(match templates.file.templates.get(template_id) {
                    Some(FileTemplate {
                        held: FileHeld::Disabled,
                        path: Some(path),
                        ..
                    }) => format!(
                        "template '{template_id}' is switched off by {} (disabled: true); delete \
                         that file, or remove the line, to use it again",
                        path.display()
                    ),
                    Some(FileTemplate {
                        held: FileHeld::Withheld { reason },
                        path: Some(path),
                        ..
                    }) => {
                        let under = if templates.bundled.contains_key(template_id) {
                            " to use the bundled template"
                        } else {
                            ""
                        };
                        format!(
                            "template '{template_id}' is withheld: {} {reason}; fix it, or remove it{under}",
                            path.display()
                        )
                    }
                    _ => format!("template '{}' not found", template_id),
                });
            };
            (template.frontmatter.singleton, template.clone())
        }; // templates lock dropped here

        let mut instances = self.lock_instances();

        if is_singleton {
            // Singleton: look for existing instance of this template
            if let Some(entry) = instances
                .values_mut()
                .find(|r| r.agent.template_id == template_id)
            {
                if !entry.agent.status.is_available() {
                    return Err(format!(
                        "singleton agent '{}' is busy (status: {})",
                        template_id, entry.agent.status
                    ));
                }
                // Claim the existing idle instance
                entry.agent.status = AgentStatus::Busy {
                    task_id: task_id.clone(),
                };
                entry.agent.current_task = Some(task_id);
                return Ok(entry.agent.clone());
            }
            // No instance yet — create with stable ID = template_id
            let agent = template_clone.to_subagent(template_id, &task_id);
            instances.insert(
                template_id.to_string(),
                RegisteredAgent {
                    agent: agent.clone(),
                    config_version: 0,
                    is_singleton: true,
                },
            );
            Ok(agent)
        } else {
            // Non-singleton: create a fresh instance with unique ID
            let instance_id = loop {
                let short_uuid = &uuid::Uuid::new_v4().to_string()[..8];
                let candidate = format!("{}::{}", template_id, short_uuid);
                if !instances.contains_key(&candidate) {
                    break candidate;
                }
            };
            let agent = template_clone.to_subagent(&instance_id, &task_id);
            instances.insert(
                instance_id,
                RegisteredAgent {
                    agent: agent.clone(),
                    config_version: 0,
                    is_singleton: false,
                },
            );
            Ok(agent)
        }
    }

    /// Destroy (remove) an agent instance.
    ///
    /// For singletons, sets the instance back to Idle instead of removing,
    /// so it can be re-claimed later. Returns a `DestroyOutcome` so callers
    /// can emit the correct lifecycle event status.
    ///
    /// Uses the cached `is_singleton` field on `RegisteredAgent` to avoid
    /// needing the templates lock.
    pub fn destroy_instance(&self, instance_id: &str) -> DestroyOutcome {
        let mut instances = self.lock_instances();
        if let Some(entry) = instances.get_mut(instance_id)
            && entry.is_singleton
        {
            // Singleton: reset to Idle instead of removing
            entry.agent.status = AgentStatus::Idle;
            entry.agent.current_task = None;
            return DestroyOutcome::ResetToIdle;
        }
        // Non-singleton: remove entirely
        if instances.remove(instance_id).is_some() {
            DestroyOutcome::Removed
        } else {
            DestroyOutcome::NotFound
        }
    }

    /// Get an instance by instance_id.
    pub fn get_instance(&self, instance_id: &str) -> Option<SubAgent> {
        self.lock_instances()
            .get(instance_id)
            .map(|r| r.agent.clone())
    }

    /// List all active instances.
    pub fn list_instances(&self) -> Vec<SubAgent> {
        self.lock_instances()
            .values()
            .map(|r| r.agent.clone())
            .collect()
    }

    /// Count active instances spawned from a given template.
    pub fn count_instances_of(&self, template_id: &str) -> usize {
        self.lock_instances()
            .values()
            .filter(|r| r.agent.template_id == template_id)
            .count()
    }

    /// Make the idle instances follow the resolved catalog (L-1a). Called
    /// after every resolved change — the file layer's rescan and
    /// `set_bundled_layer`.
    ///
    /// Each resolved file or bundled template has an idle instance under its
    /// id — the one the singleton lead is claimed from, `GET
    /// /v1/agents/{id}/config` reads and `?skill=` filters: an idle one built
    /// from another definition is rebuilt, a missing one registered, a busy
    /// one left alone and rebuilt at the next change, as hot reload always
    /// did. The idle instance of an id that left the catalog goes. Only the
    /// instances this method materialised are ever removed, so a legacy
    /// `.toml` agent stays; a plugin's template gets none, as before.
    pub fn sync_template_instances(&self) {
        let wanted: HashMap<String, AgentTemplate> = {
            let templates = self.lock_templates();
            templates
                .resolved
                .iter()
                .filter(|(id, _)| templates.layer_of(id) != Some(TemplateLayer::Plugin))
                .map(|(id, template)| (id.clone(), template.clone()))
                .collect()
        };
        let mut instances = self.lock_instances();
        let mut synced = self.lock_synced();

        for (id, template) in &wanted {
            match instances.get_mut(id) {
                None => {
                    instances.insert(
                        id.clone(),
                        RegisteredAgent {
                            agent: idle_instance(template),
                            config_version: 0,
                            is_singleton: false,
                        },
                    );
                    synced.insert(id.clone(), template.clone());
                }
                Some(entry) if entry.agent.status.is_available() => {
                    let current = synced
                        .get(id)
                        .is_some_and(|built| same_definition(built, template));
                    if !current {
                        entry.agent = idle_instance(template);
                        entry.config_version += 1;
                        synced.insert(id.clone(), template.clone());
                    }
                }
                Some(_) => {} // busy: the next change rebuilds it
            }
        }

        synced.retain(|id, _| {
            if wanted.contains_key(id) {
                return true;
            }
            match instances.get(id) {
                Some(entry) if !entry.agent.status.is_available() => true,
                Some(_) => {
                    instances.remove(id);
                    false
                }
                None => false,
            }
        });
    }

    /// The file layer's rescan lock: see `rescan`.
    pub(crate) fn lock_rescan(&self) -> std::sync::MutexGuard<'_, ()> {
        match self.rescan.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!("AgentRegistry rescan mutex poisoned — recovering");
                poisoned.into_inner()
            }
        }
    }

    fn lock_synced(&self) -> std::sync::MutexGuard<'_, HashMap<String, AgentTemplate>> {
        match self.synced.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!("AgentRegistry synced-instances mutex poisoned — recovering");
                poisoned.into_inner()
            }
        }
    }

    // ── Backward-compatible methods ──────────────────────────────────
    //
    // These bridge the old singleton-agent API to the new template+instance model.
    // They operate on the `instances` collection and are used by existing consumers
    // that haven't been migrated yet.

    /// Register a SubAgent directly (backward compat).
    ///
    /// Stores the agent as an instance entry. Used during startup when loading
    /// from TOML files that haven't been migrated to .md templates yet.
    pub fn register(&self, agent: SubAgent) -> bool {
        let mut instances = self.lock_instances();
        if instances.contains_key(&agent.id) {
            return false;
        }
        instances.insert(
            agent.id.clone(),
            RegisteredAgent {
                agent,
                config_version: 0,
                is_singleton: false,
            },
        );
        true
    }

    /// Get a SubAgent by id. Searches instances first.
    pub fn get(&self, agent_id: &str) -> Option<SubAgent> {
        self.lock_instances().get(agent_id).map(|r| r.agent.clone())
    }

    /// Get a SubAgent and its config_version by id.
    pub fn get_with_version(&self, agent_id: &str) -> Option<(SubAgent, u64)> {
        self.lock_instances()
            .get(agent_id)
            .map(|r| (r.agent.clone(), r.config_version))
    }

    /// Update the config of a registered agent with optimistic locking.
    pub fn update_config(
        &self,
        agent_id: &str,
        new_agent: SubAgent,
        expected_version: u64,
    ) -> Result<u64, String> {
        let mut instances = self.lock_instances();
        let entry = instances
            .get_mut(agent_id)
            .ok_or_else(|| "AGENT_NOT_FOUND".to_string())?;

        if entry.config_version != expected_version {
            return Err("CONFIG_CONFLICT".to_string());
        }

        entry.agent = new_agent;
        entry.config_version += 1;
        Ok(entry.config_version)
    }

    /// Update the status of an agent instance. Returns false if not found.
    pub fn update_status(&self, agent_id: &str, status: AgentStatus) -> bool {
        let mut instances = self.lock_instances();
        if let Some(entry) = instances.get_mut(agent_id) {
            entry.agent.current_task = match &status {
                AgentStatus::Busy { task_id } => Some(task_id.clone()),
                _ => None,
            };
            entry.agent.status = status;
            true
        } else {
            false
        }
    }

    /// Atomically claim an idle agent by marking it Busy.
    ///
    /// Searches the instances collection. Used by consumers that still reference
    /// agents by their old singleton IDs.
    pub fn try_claim(&self, agent_id: &str, task_id: String) -> Result<SubAgent, String> {
        let mut instances = self.lock_instances();
        let entry = instances
            .get_mut(agent_id)
            .ok_or_else(|| format!("agent '{}' not found", agent_id))?;
        if !entry.agent.status.is_available() {
            return Err(format!(
                "agent '{}' is not available (status: {})",
                agent_id, entry.agent.status
            ));
        }
        entry.agent.status = AgentStatus::Busy {
            task_id: task_id.clone(),
        };
        entry.agent.current_task = Some(task_id);
        Ok(entry.agent.clone())
    }

    /// Remove a SubAgent instance by id. Returns true if it existed.
    pub fn remove(&self, agent_id: &str) -> bool {
        self.lock_instances().remove(agent_id).is_some()
    }

    /// Number of active instances.
    pub fn count(&self) -> usize {
        self.lock_instances().len()
    }

    /// List idle instances. For the template model, this is mainly
    /// useful for singleton agents that are in Idle state.
    pub fn list_idle(&self) -> Vec<SubAgent> {
        self.lock_instances()
            .values()
            .filter(|r| r.agent.status.is_available())
            .map(|r| r.agent.clone())
            .collect()
    }

    /// Find instances that have a given capability.
    pub fn find_by_capability(&self, capability_name: &str) -> Vec<SubAgent> {
        self.lock_instances()
            .values()
            .filter(|r| r.agent.capabilities.iter().any(|s| s.name == capability_name))
            .map(|r| r.agent.clone())
            .collect()
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
