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
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::PathBuf;
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
}

/// A definition of a template's id that the resolved view does not use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotUsed {
    pub layer: TemplateLayer,
    pub path: Option<PathBuf>,
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

/// A template of the file layer, with the file it was read from.
struct FileTemplate {
    template: AgentTemplate,
    path: Option<PathBuf>,
}

/// The file layer, built whole and handed to
/// `AgentRegistry::replace_file_layer`.
#[derive(Default)]
pub struct FileLayer {
    templates: HashMap<String, FileTemplate>,
}

impl FileLayer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a template read from `path`. Returns false, leaving the layer
    /// as it was, when a template of the same id is already in it: the
    /// first one added holds the id.
    pub fn insert(&mut self, template: AgentTemplate, path: Option<PathBuf>) -> bool {
        match self.templates.entry(template.frontmatter.id.clone()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(slot) => {
                slot.insert(FileTemplate { template, path });
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
    /// What every reader sees: recomputed by `resolve` after each change
    /// to a layer, never written by itself.
    resolved: HashMap<String, AgentTemplate>,
}

impl TemplateLayers {
    /// Recompute the resolved view: each id from the file layer, else the
    /// bundled one, else the plugin one. The one map is refilled, not
    /// replaced, so it keeps its hasher and a listing's order stays as
    /// steady across changes as the one map's was.
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
        self.resolved.extend(
            self.file
                .templates
                .iter()
                .map(|(id, entry)| (id.clone(), entry.template.clone())),
        );
    }
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
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            templates: Mutex::new(TemplateLayers::default()),
            instances: Mutex::new(HashMap::new()),
            template_tombstones: Mutex::new(HashMap::new()),
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

    /// Register an agent template. Returns false if a template with the
    /// same id already exists in any layer.
    ///
    /// A plugin's template (`AgentSource::Plugin`) goes into the plugin
    /// layer, any other into the file layer with no path.
    pub fn register_template(&self, template: AgentTemplate) -> bool {
        let mut templates = self.lock_templates();
        let id = template.frontmatter.id.clone();
        if templates.resolved.contains_key(&id) {
            return false;
        }
        // A template that came back invalidates its own tombstone.
        self.lock_template_tombstones().remove(&id);
        if matches!(template.source, AgentSource::Plugin { .. }) {
            templates.plugin.insert(id, template);
        } else {
            let entry = FileTemplate {
                template,
                path: None,
            };
            templates.file.templates.insert(id, entry);
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

    /// Replace the bundled layer. Of two templates with one id the first
    /// is kept.
    pub fn set_bundled_layer(&self, bundled: Vec<AgentTemplate>) {
        let mut layer = HashMap::new();
        for template in bundled {
            layer
                .entry(template.frontmatter.id.clone())
                .or_insert(template);
        }
        let mut templates = self.lock_templates();
        self.forget_tombstones(layer.keys());
        templates.bundled = layer;
        templates.resolve();
    }

    /// Where the template resolved under `template_id` comes from, or None
    /// when no layer holds the id.
    pub fn provenance(&self, template_id: &str) -> Option<TemplateProvenance> {
        let templates = self.lock_templates();
        let (layer, path) = if let Some(entry) = templates.file.templates.get(template_id) {
            (TemplateLayer::File, entry.path.clone())
        } else if templates.bundled.contains_key(template_id) {
            (TemplateLayer::Bundled, None)
        } else if templates.plugin.contains_key(template_id) {
            (TemplateLayer::Plugin, None)
        } else {
            return None;
        };
        Some(TemplateProvenance {
            layer,
            path,
            not_used: Vec::new(),
            state: TemplateState::Ready,
            warnings: Vec::new(),
        })
    }

    /// Get a template by id.
    pub fn get_template(&self, template_id: &str) -> Option<AgentTemplate> {
        self.lock_templates().resolved.get(template_id).cloned()
    }

    /// List all registered templates.
    pub fn list_templates(&self) -> Vec<AgentTemplate> {
        self.lock_templates().resolved.values().cloned().collect()
    }

    /// Find templates that have a given capability.
    pub fn find_templates_by_capability(&self, capability_name: &str) -> Vec<AgentTemplate> {
        self.lock_templates()
            .resolved
            .values()
            .filter(|t| t.frontmatter.capabilities.iter().any(|s| s == capability_name))
            .cloned()
            .collect()
    }

    /// Number of registered templates.
    pub fn template_count(&self) -> usize {
        self.lock_templates().resolved.len()
    }

    /// Remove a template by id from the file layer, else from the plugin
    /// layer. Returns true if one was removed. A bundled template is never
    /// removed.
    pub fn remove_template(&self, template_id: &str) -> bool {
        let mut templates = self.lock_templates();
        let removed = templates.file.templates.remove(template_id).is_some()
            || templates.plugin.remove(template_id).is_some();
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
    /// incumbent's id to the plugin would mislead a later miss.
    pub fn remove_plugin_template(&self, template_id: &str, plugin_id: &str) -> bool {
        let removed = {
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
            removed
        };
        if removed {
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
        let mut tombstones = self.lock_template_tombstones();
        let before = tombstones.len();
        tombstones.retain(|_, owner| owner != plugin_id);
        before - tombstones.len()
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
            let template = templates
                .resolved
                .get(template_id)
                .ok_or_else(|| format!("template '{}' not found", template_id))?;
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
