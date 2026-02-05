//! In-memory agent registry for tracking active SubAgents

use super::subagent::{AgentStatus, SubAgent};
use std::collections::HashMap;
use std::sync::Mutex;

/// Registry for tracking SubAgents in memory.
pub struct AgentRegistry {
    agents: Mutex<HashMap<String, SubAgent>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: Mutex::new(HashMap::new()),
        }
    }

    /// Register a SubAgent. Returns false if the id already exists.
    pub fn register(&self, agent: SubAgent) -> bool {
        let mut agents = self.agents.lock().unwrap();
        if agents.contains_key(&agent.id) {
            return false;
        }
        agents.insert(agent.id.clone(), agent);
        true
    }

    /// Get a SubAgent by id.
    pub fn get(&self, agent_id: &str) -> Option<SubAgent> {
        self.agents.lock().unwrap().get(agent_id).cloned()
    }

    /// Update the status of a SubAgent. Returns false if not found.
    pub fn update_status(&self, agent_id: &str, status: AgentStatus) -> bool {
        let mut agents = self.agents.lock().unwrap();
        if let Some(agent) = agents.get_mut(agent_id) {
            agent.current_task = match &status {
                AgentStatus::Busy { task_id } => Some(task_id.clone()),
                _ => None,
            };
            agent.status = status;
            true
        } else {
            false
        }
    }

    /// Remove a SubAgent by id. Returns true if it existed.
    pub fn remove(&self, agent_id: &str) -> bool {
        self.agents.lock().unwrap().remove(agent_id).is_some()
    }

    /// Number of registered agents.
    pub fn count(&self) -> usize {
        self.agents.lock().unwrap().len()
    }

    /// List all registered agents.
    pub fn list_all(&self) -> Vec<SubAgent> {
        self.agents.lock().unwrap().values().cloned().collect()
    }

    /// List agents that are idle (available).
    pub fn list_idle(&self) -> Vec<SubAgent> {
        self.agents
            .lock()
            .unwrap()
            .values()
            .filter(|a| a.status.is_available())
            .cloned()
            .collect()
    }

    /// Find agents that have a given skill.
    pub fn find_by_skill(&self, skill_name: &str) -> Vec<SubAgent> {
        self.agents
            .lock()
            .unwrap()
            .values()
            .filter(|a| a.skills.iter().any(|s| s.name == skill_name))
            .cloned()
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
}
