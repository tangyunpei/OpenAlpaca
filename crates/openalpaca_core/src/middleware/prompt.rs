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
///
/// When `sendable_channels` is provided, the block includes explicit send guidance
/// instructing the LLM to use the `send_message` tool with `recipient="default"`.
pub fn format_connector_guidance(
    statuses: &[(String, String)],
    sendable_channels: Option<&[String]>,
) -> String {
    let active: Vec<&(String, String)> = statuses.iter().filter(|(_, s)| s == "active").collect();

    // Channels that are sendable but not active (e.g. iMessage connector crashed
    // but AppleScript sending still works independently).
    let send_only: Vec<&str> = sendable_channels
        .unwrap_or(&[])
        .iter()
        .filter(|ch| !active.iter().any(|(name, _)| name == ch.as_str()))
        .map(|s| s.as_str())
        .collect();

    if active.is_empty() && send_only.is_empty() {
        return String::new();
    }

    let mut block = String::from(
        "<connector_status>\nConnected communication channels:\n",
    );
    for (name, _) in &active {
        let label = match name.as_str() {
            "imessage" => "iMessage (macOS — sends via AppleScript)",
            "telegram" => "Telegram",
            _ => name.as_str(),
        };
        block.push_str(&format!("- {} [active]\n", label));
    }
    for ch in &send_only {
        let label = match *ch {
            "imessage" => "iMessage (macOS — sends via AppleScript)",
            "telegram" => "Telegram",
            _ => ch,
        };
        block.push_str(&format!("- {} [send-capable]\n", label));
    }
    block.push_str(
        "\nWhen a message arrives from one of these channels, your reply is automatically \
         delivered back through the same channel.",
    );

    // Enhanced guidance when sendable channels are known
    let has_sendable = sendable_channels
        .map(|sc| !sc.is_empty())
        .unwrap_or(false);
    if has_sendable {
        block.push_str(
            "\n\nIMPORTANT — Sending messages:\n\
             - To proactively send a message: use the `send_message` tool.\n\
             - Prefer recipient=\"default\" — sends to the user's most recent conversation.\n\
             - Only ask for a specific recipient if the tool returns an error or the user explicitly provides one.",
        );
    } else {
        block.push_str(
            "\nTo proactively send a message to a contact via these channels, use the `send_message` tool.",
        );
    }

    block.push_str("\n</connector_status>");
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

    // --- format_connector_guidance tests ---

    #[test]
    fn test_connector_guidance_no_active_channels() {
        let statuses = vec![("telegram".to_string(), "disabled".to_string())];
        let result = format_connector_guidance(&statuses, None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_connector_guidance_active_without_sendable() {
        let statuses = vec![("telegram".to_string(), "active".to_string())];
        let result = format_connector_guidance(&statuses, None);
        assert!(result.contains("Telegram [active]"));
        assert!(result.contains("send_message"));
        // Without sendable_channels, should NOT contain the enhanced guidance
        assert!(!result.contains("IMPORTANT"));
    }

    #[test]
    fn test_connector_guidance_active_with_sendable() {
        let statuses = vec![("telegram".to_string(), "active".to_string())];
        let sendable = vec!["telegram".to_string()];
        let result = format_connector_guidance(&statuses, Some(&sendable));
        assert!(result.contains("Telegram [active]"));
        assert!(result.contains("IMPORTANT"));
        assert!(result.contains("send_message"));
        assert!(result.contains("recipient=\"default\""));
    }

    #[test]
    fn test_connector_guidance_empty_sendable() {
        let statuses = vec![("telegram".to_string(), "active".to_string())];
        let sendable: Vec<String> = vec![];
        let result = format_connector_guidance(&statuses, Some(&sendable));
        // Empty sendable should behave like None
        assert!(!result.contains("IMPORTANT"));
    }

    #[test]
    fn test_connector_guidance_send_only_channel() {
        // iMessage connector crashed (status=error), but AppleScript sending still works.
        // It should still appear as [send-capable].
        let statuses = vec![
            ("telegram".to_string(), "active".to_string()),
            ("imessage".to_string(), "error".to_string()),
        ];
        let sendable = vec!["telegram".to_string(), "imessage".to_string()];
        let result = format_connector_guidance(&statuses, Some(&sendable));
        assert!(result.contains("Telegram [active]"));
        assert!(result.contains("iMessage (macOS — sends via AppleScript) [send-capable]"));
        assert!(result.contains("IMPORTANT"));
    }

    #[test]
    fn test_connector_guidance_send_only_no_active() {
        // All connectors crashed, but sending still works — should NOT be empty.
        let statuses = vec![("imessage".to_string(), "error".to_string())];
        let sendable = vec!["imessage".to_string()];
        let result = format_connector_guidance(&statuses, Some(&sendable));
        assert!(!result.is_empty());
        assert!(result.contains("iMessage (macOS — sends via AppleScript) [send-capable]"));
    }

    #[test]
    fn test_connector_guidance_no_active_no_sendable() {
        // No active, no sendable — truly empty.
        let statuses = vec![("telegram".to_string(), "disabled".to_string())];
        let result = format_connector_guidance(&statuses, None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_connector_guidance_imessage_label_mentions_applescript() {
        let statuses = vec![("imessage".to_string(), "active".to_string())];
        let sendable = vec!["imessage".to_string()];
        let result = format_connector_guidance(&statuses, Some(&sendable));
        assert!(result.contains("AppleScript"));
    }
}
