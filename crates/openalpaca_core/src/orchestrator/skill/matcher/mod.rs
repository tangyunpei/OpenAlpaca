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
    /// Find agents that can cover the required capabilities.
    ///
    /// Strategy: greedy set-cover — sort idle agents by number of matching
    /// capabilities (descending), pick agents until all capabilities are covered.
    /// Partial coverage is a warning, not an error.
    /// Empty capabilities or no idle agents → error.
    pub fn match_skills(
        &self,
        required: &[String],
        registry: &AgentRegistry,
    ) -> Result<Vec<SkillMatch>, String> {
        if required.is_empty() {
            return Err("No capabilities specified for matching".to_string());
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
                    .filter(|skill| agent.capabilities.iter().any(|s| &s.name == *skill))
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
            return Err("No agents match the required capabilities".to_string());
        }

        Ok(matches)
    }
}

#[cfg(test)]
mod tests;
