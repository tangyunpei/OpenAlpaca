//! Skill matching: maps required skills to available agents.

use crate::agent::registry::AgentRegistry;

/// A matched agent with the skills it can fulfill.
#[derive(Debug, Clone)]
pub struct SkillMatch {
    pub agent_id: String,
    pub agent_name: String,
    pub matched_skills: Vec<String>,
    pub role_description: String,
}

/// Matches required skills to idle agents using greedy set-cover.
pub struct SkillMatcher;

impl SkillMatcher {
    /// Find agents that can cover the required skills.
    ///
    /// Strategy: greedy set-cover — sort idle agents by number of matching
    /// skills (descending), pick agents until all skills are covered.
    /// Partial coverage is a warning, not an error.
    /// Empty skills or no idle agents → error.
    pub fn match_skills(
        &self,
        required: &[String],
        registry: &AgentRegistry,
    ) -> Result<Vec<SkillMatch>, String> {
        if required.is_empty() {
            return Err("No skills specified for matching".to_string());
        }

        let idle_agents = registry.list_idle();
        if idle_agents.is_empty() {
            return Err("No idle agents available".to_string());
        }

        let mut uncovered: Vec<String> = required.to_vec();
        let mut matches = Vec::new();

        while !uncovered.is_empty() {
            // Find the idle agent that covers the most uncovered skills
            let mut best_agent = None;
            let mut best_matched: Vec<String> = Vec::new();

            for agent in &idle_agents {
                // Skip agents we already selected
                if matches.iter().any(|m: &SkillMatch| m.agent_id == agent.id) {
                    continue;
                }

                let matched: Vec<String> = uncovered
                    .iter()
                    .filter(|skill| agent.skills.iter().any(|s| &s.name == *skill))
                    .cloned()
                    .collect();

                if matched.len() > best_matched.len() {
                    best_matched = matched;
                    best_agent = Some(agent.clone());
                }
            }

            match best_agent {
                Some(agent) if !best_matched.is_empty() => {
                    // Remove covered skills
                    uncovered.retain(|s| !best_matched.contains(s));

                    matches.push(SkillMatch {
                        agent_id: agent.id.clone(),
                        agent_name: agent.name.clone(),
                        matched_skills: best_matched,
                        role_description: agent
                            .description
                            .clone()
                            .unwrap_or_else(|| agent.name.clone()),
                    });
                }
                _ => {
                    // No more agents can cover remaining skills — partial coverage
                    break;
                }
            }
        }

        if matches.is_empty() {
            return Err("No agents match the required skills".to_string());
        }

        Ok(matches)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::subagent::{
        AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Skill, SubAgent,
    };

    fn make_agent(id: &str, name: &str, skills: Vec<&str>) -> SubAgent {
        SubAgent {
            id: id.to_string(),
            template_id: id.to_string(),
            name: name.to_string(),
            description: Some(format!("{} agent", name)),
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

    fn make_registry(agents: Vec<SubAgent>) -> AgentRegistry {
        let reg = AgentRegistry::new();
        for a in agents {
            reg.register(a);
        }
        reg
    }

    #[test]
    fn test_single_skill() {
        let reg = make_registry(vec![make_agent("a1", "Searcher", vec!["web_search"])]);
        let matcher = SkillMatcher;
        let result = matcher
            .match_skills(&["web_search".to_string()], &reg)
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].agent_id, "a1");
        assert_eq!(result[0].matched_skills, vec!["web_search"]);
    }

    #[test]
    fn test_multi_skills_single_agent() {
        let reg = make_registry(vec![make_agent(
            "a1",
            "MultiTool",
            vec!["web_search", "summarize", "text_generate"],
        )]);
        let matcher = SkillMatcher;
        let result = matcher
            .match_skills(
                &[
                    "web_search".to_string(),
                    "summarize".to_string(),
                    "text_generate".to_string(),
                ],
                &reg,
            )
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].matched_skills.len(), 3);
    }

    #[test]
    fn test_multi_agents() {
        let reg = make_registry(vec![
            make_agent("a1", "Searcher", vec!["web_search"]),
            make_agent("a2", "Writer", vec!["text_generate"]),
        ]);
        let matcher = SkillMatcher;
        let result = matcher
            .match_skills(
                &["web_search".to_string(), "text_generate".to_string()],
                &reg,
            )
            .unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_no_idle() {
        let reg = AgentRegistry::new();
        let agent = make_agent("a1", "Searcher", vec!["web_search"]);
        reg.register(agent);
        reg.update_status(
            "a1",
            AgentStatus::Busy {
                task_id: "t1".into(),
            },
        );

        let matcher = SkillMatcher;
        let result = matcher.match_skills(&["web_search".to_string()], &reg);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No idle agents"));
    }

    #[test]
    fn test_no_matching() {
        let reg = make_registry(vec![make_agent("a1", "Searcher", vec!["web_search"])]);
        let matcher = SkillMatcher;
        let result = matcher.match_skills(&["text_generate".to_string()], &reg);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No agents match"));
    }

    #[test]
    fn test_empty_skills() {
        let reg = make_registry(vec![make_agent("a1", "Searcher", vec!["web_search"])]);
        let matcher = SkillMatcher;
        let result = matcher.match_skills(&[], &reg);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No skills specified"));
    }

    #[test]
    fn test_greedy_prefers_more_skills() {
        // a1 has 1 skill, a2 has 2 of the required skills -> a2 should be picked first
        let reg = make_registry(vec![
            make_agent("a1", "Narrow", vec!["web_search"]),
            make_agent("a2", "Broad", vec!["web_search", "summarize"]),
        ]);
        let matcher = SkillMatcher;
        let result = matcher
            .match_skills(&["web_search".to_string(), "summarize".to_string()], &reg)
            .unwrap();
        // Greedy should pick a2 first (covers both), so only 1 agent needed
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].agent_id, "a2");
        assert_eq!(result[0].matched_skills.len(), 2);
    }
}
