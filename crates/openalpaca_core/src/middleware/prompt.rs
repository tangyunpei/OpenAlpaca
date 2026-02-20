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
                "Act as the user's trusted local AI agent, respecting their privacy and autonomy".to_string(),
                "Provide structured output (JSON) when the task requires machine-readable results".to_string(),
            ],
            safety_rules: vec![
                "Confirm with the user before executing system commands or destructive actions".to_string(),
                "Protect the user's sensitive data and keep it within the local environment".to_string(),
            ],
            base_instructions: "You are OpenAlpaca, a locally-hosted AI agent that helps the user \
                manage tasks, retrieve information, and coordinate work through specialized \
                sub-agents and tools. You run entirely on the user's own machine."
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
        prompt.push_str("<system_instructions>\n");
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
            "Base Instructions: {}\n",
            system.base_instructions
        ));
        prompt.push_str("</system_instructions>\n\n");

        // 2. Agent Block (Mutable)
        prompt.push_str("<agent_role>\n");
        prompt.push_str(&format!("Role: {}\n", agent.role));
        prompt.push_str(&format!("Tone: {}\n", agent.tone));
        if !agent.domain_knowledge.is_empty() {
            prompt.push_str("Domain Knowledge:\n");
            for domain in &agent.domain_knowledge {
                prompt.push_str(&format!("- {}\n", domain));
            }
        }
        prompt.push_str("</agent_role>\n");

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
        "\n\n<available_tools>\n{}\n\
         \n\
         Use these tools to access files, fetch URLs, manage tasks, and complete the user's request.\n\
         Always use the provided tools rather than claiming you cannot perform an action.\n\
         If a tool call fails, report the error clearly and suggest an alternative approach.\n\
         </available_tools>",
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

        assert!(prompt.contains("<system_instructions>"));
        assert!(prompt.contains("</system_instructions>"));
        assert!(prompt.contains("Identity: OpenAlpaca"));
        assert!(prompt.contains("<agent_role>"));
        assert!(prompt.contains("</agent_role>"));
        assert!(prompt.contains("Role: Coder"));
        // User input should NOT be in the system prompt
        assert!(!prompt.contains("<user_input>"));
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
