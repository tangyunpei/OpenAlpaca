//! In-memory agent registry for tracking active SubAgents

use super::subagent::{AgentStatus, SubAgent};
use std::collections::HashMap;
use std::sync::Mutex;

/// Internal wrapper that pairs a SubAgent with its config version
/// for optimistic locking on config updates.
struct RegisteredAgent {
    agent: SubAgent,
    config_version: u64,
}

/// Registry for tracking SubAgents in memory.
pub struct AgentRegistry {
    agents: Mutex<HashMap<String, RegisteredAgent>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: Mutex::new(HashMap::new()),
        }
    }

    /// Acquire the agents lock, recovering from poisoning if necessary.
    fn lock_agents(&self) -> std::sync::MutexGuard<'_, HashMap<String, RegisteredAgent>> {
        match self.agents.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!("AgentRegistry mutex poisoned — recovering");
                poisoned.into_inner()
            }
        }
    }

    /// Register a SubAgent. Returns false if the id already exists.
    pub fn register(&self, agent: SubAgent) -> bool {
        let mut agents = self.lock_agents();
        if agents.contains_key(&agent.id) {
            return false;
        }
        agents.insert(
            agent.id.clone(),
            RegisteredAgent {
                agent,
                config_version: 0,
            },
        );
        true
    }

    /// Get a SubAgent by id.
    pub fn get(&self, agent_id: &str) -> Option<SubAgent> {
        self.lock_agents()
            .get(agent_id)
            .map(|r| r.agent.clone())
    }

    /// Get a SubAgent and its config_version by id.
    pub fn get_with_version(&self, agent_id: &str) -> Option<(SubAgent, u64)> {
        self.lock_agents()
            .get(agent_id)
            .map(|r| (r.agent.clone(), r.config_version))
    }

    /// Update the config of a SubAgent with optimistic locking.
    /// Returns the new config_version on success, or an error string on version mismatch.
    pub fn update_config(
        &self,
        agent_id: &str,
        new_agent: SubAgent,
        expected_version: u64,
    ) -> Result<u64, String> {
        let mut agents = self.lock_agents();
        let entry = agents
            .get_mut(agent_id)
            .ok_or_else(|| "AGENT_NOT_FOUND".to_string())?;

        if entry.config_version != expected_version {
            return Err("CONFIG_CONFLICT".to_string());
        }

        entry.agent = new_agent;
        entry.config_version += 1;
        Ok(entry.config_version)
    }

    /// Update the status of a SubAgent. Returns false if not found.
    pub fn update_status(&self, agent_id: &str, status: AgentStatus) -> bool {
        let mut agents = self.lock_agents();
        if let Some(entry) = agents.get_mut(agent_id) {
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

    /// Remove a SubAgent by id. Returns true if it existed.
    pub fn remove(&self, agent_id: &str) -> bool {
        self.lock_agents().remove(agent_id).is_some()
    }

    /// Number of registered agents.
    pub fn count(&self) -> usize {
        self.lock_agents().len()
    }

    /// List all registered agents.
    pub fn list_all(&self) -> Vec<SubAgent> {
        self.lock_agents()
            .values()
            .map(|r| r.agent.clone())
            .collect()
    }

    /// List agents that are idle (available).
    pub fn list_idle(&self) -> Vec<SubAgent> {
        self.lock_agents()
            .values()
            .filter(|r| r.agent.status.is_available())
            .map(|r| r.agent.clone())
            .collect()
    }

    /// Find agents that have a given skill.
    pub fn find_by_skill(&self, skill_name: &str) -> Vec<SubAgent> {
        self.lock_agents()
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

    fn make_agent(id: &str, skills: Vec<&str>) -> SubAgent {
        SubAgent {
            id: id.to_string(),
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
}
