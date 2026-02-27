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
                "Act as the user's trusted local AI agent, respecting their privacy and autonomy"
                    .to_string(),
                "Provide structured output (JSON) when the task requires machine-readable results"
                    .to_string(),
            ],
            safety_rules: vec![
                "Confirm with the user before executing system commands or destructive actions"
                    .to_string(),
                "Protect the user's sensitive data and keep it within the local environment"
                    .to_string(),
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
    let tool_list: String = tools
        .iter()
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

/// Format a `<connector_status>` prompt block from a list of (name, status) pairs.
///
/// Used by `query_handler.rs`, `pipeline.rs`, `lead_agent/mod.rs`, and `dag_executor/mod.rs`
/// to inject connector awareness into system prompts without duplicating the XML formatting.
pub fn format_connector_guidance(statuses: &[(String, String)]) -> String {
    let active: Vec<&(String, String)> = statuses.iter().filter(|(_, s)| s == "active").collect();
    if active.is_empty() {
        return String::new();
    }

    let mut block = String::from(
        "<connector_status>\nConnected communication channels:\n",
    );
    for (name, _) in &active {
        let label = match name.as_str() {
            "imessage" => "iMessage (macOS Messages app)",
            "telegram" => "Telegram",
            _ => name.as_str(),
        };
        block.push_str(&format!("- {} [active]\n", label));
    }
    block.push_str(
        "\nWhen a message arrives from one of these channels, your reply is automatically \
         delivered back through the same channel.\n\
         To proactively send a message to a contact via these channels, use the `send_message` tool.\n\
         </connector_status>",
    );
    block
}

/// Format a `<message_source>` prompt block for the current message's origin channel.
///
/// Returns an empty string for "internal" sources or unknown sources.
pub fn format_message_source(source: &str) -> String {
    let label = match source {
        "imessage" => "iMessage",
        "telegram" => "Telegram",
        "gui" => "Desktop GUI",
        "cli" => "CLI",
        _ => return String::new(),
    };
    format!(
        "<message_source>\n\
         This message arrived via: {label}. Your reply will be sent back via {label} automatically.\n\
         </message_source>"
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
