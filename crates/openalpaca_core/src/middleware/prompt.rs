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
    /// Combines System Persona and Agent Persona into a system prompt.
    /// User input is NOT included — it is sent as a separate user message
    /// to avoid duplication in the LLM context.
    pub fn assemble(system: &SystemPersona, agent: &AgentPersona) -> String {
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

        prompt
    }
}

pub fn format_tool_guidance(tools: &[openalpaca_llm::ToolDefinition]) -> String {
    if tools.is_empty() {
        return String::new();
    }
    let tool_list: String = tools.iter()
        .map(|t| format!("- {}: {}", t.name, t.description))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "\n\nYou have access to the following tools:\n{}\n\n\
         Use these tools when they help complete the task. \
         Do NOT say you cannot access files or the internet \u{2014} use the provided tools instead.",
        tool_list
    )
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

        let prompt = PromptAssembler::assemble(&system, &agent);

        assert!(prompt.contains("### SYSTEM INSTRUCTIONS ###"));
        assert!(prompt.contains("Identity: OpenAlpaca"));
        assert!(prompt.contains("### AGENT ROLE ###"));
        assert!(prompt.contains("Role: Coder"));
        // User input should NOT be in the system prompt (Bug B fix)
        assert!(!prompt.contains("### USER INPUT ###"));
    }

    #[test]
    fn test_format_tool_guidance_empty() {
        let result = format_tool_guidance(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_tool_guidance_single() {
        let tools = vec![openalpaca_llm::ToolDefinition {
            name: "web_fetch".to_string(),
            description: "Fetch a URL".to_string(),
            parameters: serde_json::json!({}),
        }];
        let result = format_tool_guidance(&tools);
        assert!(result.contains("web_fetch"));
        assert!(result.contains("Fetch a URL"));
    }

    #[test]
    fn test_format_tool_guidance_multiple() {
        let tools = vec![
            openalpaca_llm::ToolDefinition {
                name: "web_fetch".to_string(),
                description: "Fetch a URL".to_string(),
                parameters: serde_json::json!({}),
            },
            openalpaca_llm::ToolDefinition {
                name: "file_read".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({}),
            },
        ];
        let result = format_tool_guidance(&tools);
        assert!(result.contains("web_fetch"));
        assert!(result.contains("file_read"));
        assert!(result.contains("Fetch a URL"));
        assert!(result.contains("Read a file"));
    }
}
