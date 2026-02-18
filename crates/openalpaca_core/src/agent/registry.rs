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

    /// Find templates that have a given skill.
    pub fn find_templates_by_skill(&self, skill_name: &str) -> Vec<AgentTemplate> {
        self.lock_templates()
            .values()
            .filter(|t| t.frontmatter.skills.iter().any(|s| s == skill_name))
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
    pub fn spawn_instance(
        &self,
        template_id: &str,
        task_id: String,
    ) -> Result<SubAgent, String> {
        let templates = self.lock_templates();
        let template = templates
            .get(template_id)
            .ok_or_else(|| format!("template '{}' not found", template_id))?;

        let mut instances = self.lock_instances();

        if template.frontmatter.singleton {
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
            let agent = template.to_subagent(template_id, &task_id);
            instances.insert(
                template_id.to_string(),
                RegisteredAgent {
                    agent: agent.clone(),
                    config_version: 0,
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
            let agent = template.to_subagent(&instance_id, &task_id);
            instances.insert(
                instance_id,
                RegisteredAgent {
                    agent: agent.clone(),
                    config_version: 0,
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
    pub fn destroy_instance(&self, instance_id: &str) -> DestroyOutcome {
        let mut instances = self.lock_instances();
        if let Some(entry) = instances.get_mut(instance_id) {
            if self.is_singleton_instance(&entry.agent.template_id) {
                // Singleton: reset to Idle instead of removing
                entry.agent.status = AgentStatus::Idle;
                entry.agent.current_task = None;
                return DestroyOutcome::ResetToIdle;
            }
        }
        // Non-singleton: remove entirely
        if instances.remove(instance_id).is_some() {
            DestroyOutcome::Removed
        } else {
            DestroyOutcome::NotFound
        }
    }

    /// Check if a template_id refers to a singleton template.
    fn is_singleton_instance(&self, template_id: &str) -> bool {
        self.lock_templates()
            .get(template_id)
            .map(|t| t.frontmatter.singleton)
            .unwrap_or(false)
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
            },
        );
        true
    }

    /// Get a SubAgent by id. Searches instances first.
    pub fn get(&self, agent_id: &str) -> Option<SubAgent> {
        self.lock_instances()
            .get(agent_id)
            .map(|r| r.agent.clone())
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

    /// List all instances (backward compat for `list_all()`).
    pub fn list_all(&self) -> Vec<SubAgent> {
        self.list_instances()
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

    /// Find instances that have a given skill (backward compat).
    pub fn find_by_skill(&self, skill_name: &str) -> Vec<SubAgent> {
        self.lock_instances()
            .values()
            .filter(|r| r.agent.skills.iter().any(|s| s.name == skill_name))
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
mod tests {
    use super::*;
    use crate::agent::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, Skill};
    use crate::agent::template::{parse_agent_markdown, AgentTemplate, AgentTemplateFrontmatter};

    fn make_agent(id: &str, skills: Vec<&str>) -> SubAgent {
        SubAgent {
            id: id.to_string(),
            template_id: id.to_string(),
            name: format!("Agent {}", id),
            description: None,
            icon: None,
            status: AgentStatus::Idle,
            current_task: None,
            skills: skills
                .into_iter()
                .map(|s| Skill {
                    name: s.to_string(),
                    category: "test".to_string(),
                    proficiency: 1.0,
                })
                .collect(),
            preset: AgentPreset::default(),
            constraints: AgentConstraints::default(),
            llm_config: AgentLlmConfig::default(),
        }
    }

    fn make_template(id: &str, skills: Vec<&str>, singleton: bool) -> AgentTemplate {
        AgentTemplate {
            frontmatter: AgentTemplateFrontmatter {
                id: id.to_string(),
                name: format!("Template {}", id),
                description: format!("{} template", id),
                icon: None,
                singleton,
                skills: skills.into_iter().map(|s| s.to_string()).collect(),
                denied_skills: vec![],
                temperature: 0.5,
                verbosity: "normal".to_string(),
                model: None,
                fallback_models: vec![],
                max_tool_calls: None,
                timeout_seconds: None,
                max_cost_per_task: None,
                require_confirmation_for: vec![],
            },
            body: String::new(),
            sections: std::collections::HashMap::new(),
        }
    }

    // ── Legacy (backward compat) tests ─────────────────────────────

    #[test]
    fn test_register_and_get() {
        let reg = AgentRegistry::new();
        assert!(reg.register(make_agent("a1", vec!["search"])));
        assert!(!reg.register(make_agent("a1", vec!["search"]))); // duplicate
        assert_eq!(reg.count(), 1);

        let agent = reg.get("a1").unwrap();
        assert_eq!(agent.name, "Agent a1");
    }

    #[test]
    fn test_get_nonexistent() {
        let reg = AgentRegistry::new();
        assert!(reg.get("nope").is_none());
    }

    #[test]
    fn test_remove() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec![]));
        assert!(reg.remove("a1"));
        assert!(!reg.remove("a1"));
        assert_eq!(reg.count(), 0);
    }

    #[test]
    fn test_update_status() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec![]));

        assert!(reg.update_status(
            "a1",
            AgentStatus::Busy {
                task_id: "t1".into()
            }
        ));
        let agent = reg.get("a1").unwrap();
        assert_eq!(agent.status.as_str(), "busy");
        assert_eq!(agent.current_task.as_deref(), Some("t1"));

        assert!(reg.update_status("a1", AgentStatus::Idle));
        let agent = reg.get("a1").unwrap();
        assert!(agent.status.is_available());
        assert!(agent.current_task.is_none());

        assert!(!reg.update_status("nope", AgentStatus::Idle));
    }

    #[test]
    fn test_list_all() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec![]));
        reg.register(make_agent("a2", vec![]));

        let all = reg.list_all();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_list_idle() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec![]));
        reg.register(make_agent("a2", vec![]));

        reg.update_status(
            "a1",
            AgentStatus::Busy {
                task_id: "t1".into(),
            },
        );

        let idle = reg.list_idle();
        assert_eq!(idle.len(), 1);
        assert_eq!(idle[0].id, "a2");
    }

    #[test]
    fn test_find_by_skill() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec!["search", "summarize"]));
        reg.register(make_agent("a2", vec!["write"]));
        reg.register(make_agent("a3", vec!["search"]));

        let searchers = reg.find_by_skill("search");
        assert_eq!(searchers.len(), 2);

        let writers = reg.find_by_skill("write");
        assert_eq!(writers.len(), 1);

        let none = reg.find_by_skill("nonexistent");
        assert!(none.is_empty());
    }

    #[test]
    fn test_get_with_version() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec!["search"]));

        let (agent, version) = reg.get_with_version("a1").unwrap();
        assert_eq!(agent.id, "a1");
        assert_eq!(version, 0);

        assert!(reg.get_with_version("nope").is_none());
    }

    #[test]
    fn test_update_config_success() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec!["search"]));

        let mut updated = make_agent("a1", vec!["search", "summarize"]);
        updated.name = "Updated Agent".to_string();

        let new_version = reg.update_config("a1", updated, 0).unwrap();
        assert_eq!(new_version, 1);

        let (agent, version) = reg.get_with_version("a1").unwrap();
        assert_eq!(agent.name, "Updated Agent");
        assert_eq!(agent.skills.len(), 2);
        assert_eq!(version, 1);
    }

    #[test]
    fn test_update_config_conflict() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec!["search"]));

        // First update succeeds
        let updated = make_agent("a1", vec!["search", "summarize"]);
        reg.update_config("a1", updated, 0).unwrap();

        // Second update with stale version fails
        let updated2 = make_agent("a1", vec!["write"]);
        let err = reg.update_config("a1", updated2, 0).unwrap_err();
        assert_eq!(err, "CONFIG_CONFLICT");
    }

    #[test]
    fn test_update_config_not_found() {
        let reg = AgentRegistry::new();
        let agent = make_agent("a1", vec![]);
        let err = reg.update_config("a1", agent, 0).unwrap_err();
        assert_eq!(err, "AGENT_NOT_FOUND");
    }

    #[test]
    fn test_update_config_sequential_versions() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec![]));

        let v1 = reg
            .update_config("a1", make_agent("a1", vec!["s1"]), 0)
            .unwrap();
        assert_eq!(v1, 1);

        let v2 = reg
            .update_config("a1", make_agent("a1", vec!["s2"]), 1)
            .unwrap();
        assert_eq!(v2, 2);

        let v3 = reg
            .update_config("a1", make_agent("a1", vec!["s3"]), 2)
            .unwrap();
        assert_eq!(v3, 3);
    }

    #[test]
    fn test_try_claim_success() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec!["search"]));

        let agent = reg.try_claim("a1", "task-1".to_string()).unwrap();
        assert_eq!(agent.id, "a1");
        assert_eq!(agent.status.as_str(), "busy");
        assert_eq!(agent.current_task.as_deref(), Some("task-1"));

        // Registry state is also Busy
        let fetched = reg.get("a1").unwrap();
        assert_eq!(fetched.status.as_str(), "busy");
        assert_eq!(fetched.current_task.as_deref(), Some("task-1"));
    }

    #[test]
    fn test_try_claim_already_busy() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec!["search"]));

        // First claim succeeds
        reg.try_claim("a1", "task-1".to_string()).unwrap();

        // Second claim fails
        let err = reg.try_claim("a1", "task-2".to_string()).unwrap_err();
        assert!(err.contains("not available"));

        // Original task_id preserved
        let fetched = reg.get("a1").unwrap();
        assert_eq!(fetched.current_task.as_deref(), Some("task-1"));
    }

    #[test]
    fn test_try_claim_not_found() {
        let reg = AgentRegistry::new();
        let err = reg.try_claim("nonexistent", "task-1".to_string()).unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn test_try_claim_concurrent() {
        use std::sync::Arc;
        use std::thread;

        let reg = Arc::new(AgentRegistry::new());
        reg.register(make_agent("a1", vec!["search"]));

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let reg = reg.clone();
                thread::spawn(move || reg.try_claim("a1", format!("task-{}", i)))
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wins = results.iter().filter(|r| r.is_ok()).count();
        let losses = results.iter().filter(|r| r.is_err()).count();

        assert_eq!(wins, 1, "Exactly one thread should win the claim");
        assert_eq!(losses, 9);
    }

    // ── Template + Instance tests ──────────────────────────────────

    #[test]
    fn test_register_template() {
        let reg = AgentRegistry::new();
        let t = make_template("code_agent", vec!["file_read", "file_write"], false);
        assert!(reg.register_template(t.clone()));
        assert!(!reg.register_template(t)); // duplicate
        assert_eq!(reg.template_count(), 1);

        let fetched = reg.get_template("code_agent").unwrap();
        assert_eq!(fetched.frontmatter.id, "code_agent");
    }

    #[test]
    fn test_list_templates() {
        let reg = AgentRegistry::new();
        reg.register_template(make_template("a", vec![], false));
        reg.register_template(make_template("b", vec![], false));
        assert_eq!(reg.list_templates().len(), 2);
    }

    #[test]
    fn test_find_templates_by_skill() {
        let reg = AgentRegistry::new();
        reg.register_template(make_template("a", vec!["search"], false));
        reg.register_template(make_template("b", vec!["write"], false));
        reg.register_template(make_template("c", vec!["search", "write"], false));

        let searchers = reg.find_templates_by_skill("search");
        assert_eq!(searchers.len(), 2);

        let writers = reg.find_templates_by_skill("write");
        assert_eq!(writers.len(), 2);

        let none = reg.find_templates_by_skill("nonexistent");
        assert!(none.is_empty());
    }

    #[test]
    fn test_spawn_instance_non_singleton() {
        let reg = AgentRegistry::new();
        reg.register_template(make_template("code_agent", vec!["file_read"], false));

        let inst1 = reg.spawn_instance("code_agent", "task-1".into()).unwrap();
        assert!(inst1.id.starts_with("code_agent::"));
        assert_eq!(inst1.template_id, "code_agent");
        assert_eq!(inst1.status.as_str(), "busy");
        assert_eq!(inst1.current_task.as_deref(), Some("task-1"));

        // Can spawn multiple instances from the same template
        let inst2 = reg.spawn_instance("code_agent", "task-2".into()).unwrap();
        assert!(inst2.id.starts_with("code_agent::"));
        assert_ne!(inst1.id, inst2.id);
        assert_eq!(reg.count(), 2);
    }

    #[test]
    fn test_spawn_instance_singleton() {
        let reg = AgentRegistry::new();
        reg.register_template(make_template("lead_agent", vec!["orchestrate"], true));

        // First spawn creates the singleton instance
        let inst1 = reg.spawn_instance("lead_agent", "task-1".into()).unwrap();
        assert_eq!(inst1.id, "lead_agent"); // stable ID
        assert_eq!(inst1.status.as_str(), "busy");

        // Second spawn fails (singleton is busy)
        let err = reg.spawn_instance("lead_agent", "task-2".into()).unwrap_err();
        assert!(err.contains("busy"));

        // Release singleton
        reg.destroy_instance("lead_agent");
        let fetched = reg.get_instance("lead_agent").unwrap();
        assert!(fetched.status.is_available());

        // Re-claim succeeds
        let inst2 = reg.spawn_instance("lead_agent", "task-3".into()).unwrap();
        assert_eq!(inst2.id, "lead_agent");
        assert_eq!(inst2.current_task.as_deref(), Some("task-3"));
    }

    #[test]
    fn test_spawn_instance_template_not_found() {
        let reg = AgentRegistry::new();
        let err = reg.spawn_instance("nope", "task-1".into()).unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn test_destroy_instance_non_singleton() {
        let reg = AgentRegistry::new();
        reg.register_template(make_template("code_agent", vec![], false));

        let inst = reg.spawn_instance("code_agent", "task-1".into()).unwrap();
        assert_eq!(reg.count(), 1);

        // Destroying non-singleton removes it entirely
        assert_eq!(reg.destroy_instance(&inst.id), DestroyOutcome::Removed);
        assert_eq!(reg.count(), 0);
        assert!(reg.get_instance(&inst.id).is_none());
    }

    #[test]
    fn test_destroy_instance_singleton_resets_to_idle() {
        let reg = AgentRegistry::new();
        reg.register_template(make_template("lead", vec![], true));

        let inst = reg.spawn_instance("lead", "task-1".into()).unwrap();
        assert_eq!(inst.id, "lead");

        // Destroying singleton resets to Idle instead of removing
        reg.destroy_instance("lead");
        let fetched = reg.get_instance("lead").unwrap();
        assert!(fetched.status.is_available());
        assert!(fetched.current_task.is_none());
        assert_eq!(reg.count(), 1); // still in registry
    }

    #[test]
    fn test_count_instances_of() {
        let reg = AgentRegistry::new();
        reg.register_template(make_template("code_agent", vec![], false));
        reg.register_template(make_template("research_agent", vec![], false));

        reg.spawn_instance("code_agent", "t1".into()).unwrap();
        reg.spawn_instance("code_agent", "t2".into()).unwrap();
        reg.spawn_instance("research_agent", "t3".into()).unwrap();

        assert_eq!(reg.count_instances_of("code_agent"), 2);
        assert_eq!(reg.count_instances_of("research_agent"), 1);
        assert_eq!(reg.count_instances_of("nonexistent"), 0);
    }

    #[test]
    fn test_spawn_instance_concurrent() {
        use std::sync::Arc;
        use std::thread;

        let reg = Arc::new(AgentRegistry::new());
        reg.register_template(make_template("lead", vec![], true));

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let reg = reg.clone();
                thread::spawn(move || reg.spawn_instance("lead", format!("task-{}", i)))
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wins = results.iter().filter(|r| r.is_ok()).count();
        let losses = results.iter().filter(|r| r.is_err()).count();

        assert_eq!(wins, 1, "Exactly one thread should win the singleton claim");
        assert_eq!(losses, 9);
    }

    #[test]
    fn test_spawn_non_singleton_concurrent() {
        use std::sync::Arc;
        use std::thread;

        let reg = Arc::new(AgentRegistry::new());
        reg.register_template(make_template("worker", vec![], false));

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let reg = reg.clone();
                thread::spawn(move || reg.spawn_instance("worker", format!("task-{}", i)))
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wins = results.iter().filter(|r| r.is_ok()).count();

        // All 10 should succeed for non-singleton
        assert_eq!(wins, 10, "All threads should spawn non-singleton instances");
        assert_eq!(reg.count(), 10);
    }

    #[test]
    fn test_template_with_markdown() {
        let md = r#"---
id: "test_agent"
name: "Test Agent"
description: "A test agent"
singleton: false
skills:
  - "file_read"
temperature: 0.3
---

## Persona

You are a test agent.
"#;
        let template = parse_agent_markdown(md).unwrap();
        let reg = AgentRegistry::new();
        reg.register_template(template);

        let inst = reg.spawn_instance("test_agent", "task-1".into()).unwrap();
        assert!(inst.id.starts_with("test_agent::"));
        assert_eq!(inst.preset.persona, "You are a test agent.");
        assert_eq!(inst.preset.temperature, 0.3);
        assert_eq!(inst.skills.len(), 1);
        assert_eq!(inst.skills[0].name, "file_read");
    }
}
