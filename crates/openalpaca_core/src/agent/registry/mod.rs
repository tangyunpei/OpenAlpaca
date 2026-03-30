//! In-memory agent registry with template + instance model.
//!
//! Templates define agent capabilities and are loaded at startup.
//! Instances are ephemeral runtime objects spawned from templates on demand.
//! Singleton templates (e.g. lead_agent) enforce max 1 active instance.

use super::subagent::{AgentStatus, SubAgent};
use super::template::AgentTemplate;
use std::collections::HashMap;
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

/// Internal wrapper that pairs a SubAgent instance with its config version
/// for optimistic locking on config updates.
struct RegisteredAgent {
    agent: SubAgent,
    config_version: u64,
    is_singleton: bool,
}

/// Registry for tracking agent templates and runtime instances.
///
/// Templates are immutable blueprints loaded at startup.
/// Instances are spawned on demand and destroyed when tasks complete.
pub struct AgentRegistry {
    /// Agent templates keyed by template_id (e.g. "code_agent").
    templates: Mutex<HashMap<String, AgentTemplate>>,
    /// Active agent instances keyed by instance_id (e.g. "code_agent::a1b2c3d4").
    instances: Mutex<HashMap<String, RegisteredAgent>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            templates: Mutex::new(HashMap::new()),
            instances: Mutex::new(HashMap::new()),
        }
    }

    // ── Lock helpers ─────────────────────────────────────────────────

    fn lock_templates(&self) -> std::sync::MutexGuard<'_, HashMap<String, AgentTemplate>> {
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
    /// same id already exists.
    pub fn register_template(&self, template: AgentTemplate) -> bool {
        let mut templates = self.lock_templates();
        let id = template.frontmatter.id.clone();
        if templates.contains_key(&id) {
            return false;
        }
        templates.insert(id, template);
        true
    }

    /// Get a template by id.
    pub fn get_template(&self, template_id: &str) -> Option<AgentTemplate> {
        self.lock_templates().get(template_id).cloned()
    }

    /// List all registered templates.
    pub fn list_templates(&self) -> Vec<AgentTemplate> {
        self.lock_templates().values().cloned().collect()
    }

    /// Find templates that have a given capability.
    pub fn find_templates_by_capability(&self, capability_name: &str) -> Vec<AgentTemplate> {
        self.lock_templates()
            .values()
            .filter(|t| t.frontmatter.capabilities.iter().any(|s| s == capability_name))
            .cloned()
            .collect()
    }

    /// Number of registered templates.
    pub fn template_count(&self) -> usize {
        self.lock_templates().len()
    }

    /// Remove a template by id. Returns true if it existed.
    pub fn remove_template(&self, template_id: &str) -> bool {
        self.lock_templates().remove(template_id).is_some()
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
