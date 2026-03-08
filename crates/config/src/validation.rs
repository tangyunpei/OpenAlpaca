use crate::types::OpenAlpacaConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub path: String,
    pub message: String,
    pub severity: Severity,
}

impl std::fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tag = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "[{}] {}: {}", tag, self.path, self.message)
    }
}

pub trait Validate {
    fn validate(&self) -> Vec<ValidationIssue>;
}

impl Validate for OpenAlpacaConfig {
    fn validate(&self) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();

        // --- Gateway port == 0 ---
        if let Some(ref gateway) = self.gateway
            && let Some(0) = gateway.port
        {
            issues.push(ValidationIssue {
                path: "gateway.port".into(),
                message: "port must be 1-65535, got 0".into(),
                severity: Severity::Error,
            });
        }

        // --- Gateway privileged port ---
        if let Some(ref gateway) = self.gateway
            && let Some(p) = gateway.port
            && p > 0
            && p < 1024
        {
            issues.push(ValidationIssue {
                path: "gateway.port".into(),
                message: format!("port {p} is privileged (< 1024) and may require root"),
                severity: Severity::Warning,
            });
        }

        // --- Empty agent id ---
        if let Some(ref agents) = self.agents
            && let Some(ref list) = agents.list
        {
            for (i, agent) in list.iter().enumerate() {
                if agent.id.is_empty() {
                    issues.push(ValidationIssue {
                        path: format!("agents.list[{i}].id"),
                        message: "agent id cannot be empty".into(),
                        severity: Severity::Error,
                    });
                }
            }
        }

        // --- Duplicate agent IDs ---
        if let Some(ref agents) = self.agents
            && let Some(ref list) = agents.list
        {
            let mut seen_ids = std::collections::HashSet::new();
            for (i, agent) in list.iter().enumerate() {
                if !agent.id.is_empty() && !seen_ids.insert(&agent.id) {
                    issues.push(ValidationIssue {
                        path: format!("agents.list[{i}].id"),
                        message: format!("duplicate agent id: \"{}\"", agent.id),
                        severity: Severity::Error,
                    });
                }
            }
        }

        // --- Empty binding agent ---
        if let Some(ref bindings) = self.bindings {
            for (i, binding) in bindings.iter().enumerate() {
                if binding.agent.is_empty() {
                    issues.push(ValidationIssue {
                        path: format!("bindings[{i}].agent"),
                        message: "binding agent cannot be empty".into(),
                        severity: Severity::Error,
                    });
                }
            }
        }

        // --- Binding references valid agent ---
        if let Some(ref bindings) = self.bindings
            && let Some(ref agents) = self.agents
            && let Some(ref list) = agents.list
        {
            let agent_ids: std::collections::HashSet<&str> =
                list.iter().map(|a| a.id.as_str()).collect();
            for (i, binding) in bindings.iter().enumerate() {
                if !binding.agent.is_empty() && !agent_ids.contains(binding.agent.as_str()) {
                    issues.push(ValidationIssue {
                        path: format!("bindings[{i}].agent"),
                        message: format!(
                            "agent \"{}\" not found in agents.list",
                            binding.agent
                        ),
                        severity: Severity::Warning,
                    });
                }
            }
        }

        // --- Enabled channel requires token ---
        if let Some(ref channels) = self.channels {
            if let Some(ref tg) = channels.telegram
                && tg.default.enabled == Some(true)
                && tg.default.bot_token.is_none()
            {
                issues.push(ValidationIssue {
                    path: "channels.telegram.bot_token".into(),
                    message: "telegram is enabled but bot_token is not set".into(),
                    severity: Severity::Error,
                });
            }
            if let Some(ref dc) = channels.discord
                && dc.default.enabled == Some(true)
                && dc.default.bot_token.is_none()
            {
                issues.push(ValidationIssue {
                    path: "channels.discord.bot_token".into(),
                    message: "discord is enabled but bot_token is not set".into(),
                    severity: Severity::Error,
                });
            }
            if let Some(ref sl) = channels.slack
                && sl.default.enabled == Some(true)
                && sl.default.bot_token.is_none()
            {
                issues.push(ValidationIssue {
                    path: "channels.slack.bot_token".into(),
                    message: "slack is enabled but bot_token is not set".into(),
                    severity: Severity::Error,
                });
            }
        }

        // --- Session main_key override ---
        if let Some(ref session) = self.session
            && let Some(ref key) = session.main_key
            && key != "main"
        {
            issues.push(ValidationIssue {
                path: "session.main_key".into(),
                message: format!(
                    "main_key is set to \"{key}\" instead of the default \"main\""
                ),
                severity: Severity::Warning,
            });
        }

        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_config() {
        let config = OpenAlpacaConfig::default();
        let issues = config.validate();
        assert!(issues.is_empty());
    }

    #[test]
    fn test_invalid_gateway_port() {
        let yaml = "gateway:\n  port: 0";
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let issues = config.validate();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].path.contains("gateway.port"));
        assert_eq!(issues[0].severity, Severity::Error);
    }

    #[test]
    fn test_empty_agent_id() {
        let yaml = r#"
agents:
  list:
    - id: ""
      model: "gpt-4"
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let issues = config.validate();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].path.contains("agents.list[0].id"));
        assert_eq!(issues[0].severity, Severity::Error);
    }

    #[test]
    fn test_empty_binding_agent() {
        let yaml = r#"
bindings:
  - agent: ""
    channel: "telegram"
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let issues = config.validate();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].path.contains("bindings[0].agent"));
        assert_eq!(issues[0].severity, Severity::Error);
    }

    #[test]
    fn test_duplicate_agent_ids() {
        let yaml = r#"
agents:
  list:
    - id: "assistant"
      model: "gpt-4"
    - id: "assistant"
      model: "gpt-3.5"
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let issues = config.validate();
        let dup: Vec<_> = issues
            .iter()
            .filter(|i| i.message.contains("duplicate"))
            .collect();
        assert_eq!(dup.len(), 1);
        assert!(dup[0].path.contains("agents.list[1].id"));
        assert_eq!(dup[0].severity, Severity::Error);
    }

    #[test]
    fn test_enabled_channel_missing_token() {
        let yaml = r#"
channels:
  telegram:
    enabled: true
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let issues = config.validate();
        let token_issues: Vec<_> = issues
            .iter()
            .filter(|i| i.message.contains("bot_token"))
            .collect();
        assert_eq!(token_issues.len(), 1);
        assert!(token_issues[0].path.contains("channels.telegram"));
        assert_eq!(token_issues[0].severity, Severity::Error);
    }

    #[test]
    fn test_binding_references_unknown_agent() {
        let yaml = r#"
agents:
  list:
    - id: "assistant"
      model: "gpt-4"
bindings:
  - agent: "nonexistent"
    channel: "telegram"
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let issues = config.validate();
        let unknown: Vec<_> = issues
            .iter()
            .filter(|i| i.message.contains("not found"))
            .collect();
        assert_eq!(unknown.len(), 1);
        assert!(unknown[0].path.contains("bindings[0].agent"));
        assert_eq!(unknown[0].severity, Severity::Warning);
    }

    #[test]
    fn test_session_main_key_override_warned() {
        let yaml = r#"
session:
  main_key: "custom"
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let issues = config.validate();
        let key_issues: Vec<_> = issues
            .iter()
            .filter(|i| i.path.contains("session.main_key"))
            .collect();
        assert_eq!(key_issues.len(), 1);
        assert_eq!(key_issues[0].severity, Severity::Warning);
    }

    #[test]
    fn test_privileged_port_warning() {
        let yaml = "gateway:\n  port: 80";
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let issues = config.validate();
        let port_issues: Vec<_> = issues
            .iter()
            .filter(|i| i.path.contains("gateway.port"))
            .collect();
        assert_eq!(port_issues.len(), 1);
        assert_eq!(port_issues[0].severity, Severity::Warning);
        // Must NOT be an error — it's a valid port, just privileged
        assert!(port_issues[0].message.contains("privileged"));
    }
}
