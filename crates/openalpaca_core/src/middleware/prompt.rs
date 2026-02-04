use serde::{Deserialize, Serialize};

/// Immutable constraints defined by the System (The "Soul").
/// These rules cannot be overridden by individual agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemPersona {
    pub name: String,
    pub core_values: Vec<String>,
    pub safety_rules: Vec<String>,
    pub base_instructions: String,
}

impl Default for SystemPersona {
    fn default() -> Self {
        Self {
            name: "OpenAlpaca".to_string(),
            core_values: vec![
                "Be helpful and harmless".to_string(),
                "Prefer JSON output when structure is needed".to_string(),
            ],
            safety_rules: vec![
                "Do not execute system commands without permission".to_string(),
                "Do not reveal sensitive user data".to_string(),
            ],
            base_instructions: "You are an intelligent agent running on the user's local machine."
                .to_string(),
        }
    }
}

/// Mutable style defined by the specific Agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPersona {
    pub role: String,
    pub tone: String,
    pub domain_knowledge: Vec<String>,
}

pub struct PromptAssembler;

impl PromptAssembler {
    /// Combines System Persona, Agent Persona, and User Context into a final prompt.
    /// This ensures the System Persona is always present and prioritized.
    pub fn assemble(system: &SystemPersona, agent: &AgentPersona, user_content: &str) -> String {
        let mut prompt = String::new();

        // 1. System Block (Immutable)
        prompt.push_str("### SYSTEM INSTRUCTIONS ###\n");
        prompt.push_str(&format!("Identity: {}\n", system.name));
        prompt.push_str("Core Values:\n");
        for value in &system.core_values {
            prompt.push_str(&format!("- {}\n", value));
        }
        prompt.push_str("Safety Rules:\n");
        for rule in &system.safety_rules {
            prompt.push_str(&format!("- {}\n", rule));
        }
        prompt.push_str(&format!(
            "Base Instructions: {}\n\n",
            system.base_instructions
        ));

        // 2. Agent Block (Mutable)
        prompt.push_str("### AGENT ROLE ###\n");
        prompt.push_str(&format!("Role: {}\n", agent.role));
        prompt.push_str(&format!("Tone: {}\n", agent.tone));
        if !agent.domain_knowledge.is_empty() {
            prompt.push_str("Domain Knowledge:\n");
            for domain in &agent.domain_knowledge {
                prompt.push_str(&format!("- {}\n", domain));
            }
        }
        prompt.push_str("\n");

        // 3. User Context (Dynamic)
        prompt.push_str("### USER INPUT ###\n");
        prompt.push_str(user_content);

        prompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assemble() {
        let system = SystemPersona::default();
        let agent = AgentPersona {
            role: "Coder".to_string(),
            tone: "Concise".to_string(),
            domain_knowledge: vec!["Rust".to_string(), "Systems Programming".to_string()],
        };
        let user_input = "Write a hello world in Rust";

        let prompt = PromptAssembler::assemble(&system, &agent, user_input);

        assert!(prompt.contains("### SYSTEM INSTRUCTIONS ###"));
        assert!(prompt.contains("Identity: OpenAlpaca"));
        assert!(prompt.contains("### AGENT ROLE ###"));
        assert!(prompt.contains("Role: Coder"));
        assert!(prompt.contains("### USER INPUT ###"));
        assert!(prompt.contains("Write a hello world in Rust"));
    }
}
