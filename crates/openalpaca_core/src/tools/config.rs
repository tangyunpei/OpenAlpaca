use super::registry::{RegisteredTool, ToolBackend};
use openalpaca_llm::ToolDefinition;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Deserialize)]
pub struct ToolConfigFile {
    #[serde(default)]
    pub tools: Vec<ToolConfig>,
}

#[derive(Deserialize)]
pub struct ToolConfig {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub backend: ToolBackendConfig,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
pub enum ToolBackendConfig {
    #[serde(rename = "http")]
    Http {
        url: String,
        method: Option<String>,
        headers: Option<HashMap<String, String>>,
        timeout_secs: Option<u64>,
    },
    #[serde(rename = "command")]
    Command {
        command: String,
        args_template: Option<String>,
        timeout_secs: Option<u64>,
    },
}

/// Load tools from a single TOML file.
pub fn load_tools_from_file(path: &Path) -> Result<Vec<RegisteredTool>, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

    let config: ToolConfigFile =
        toml::from_str(&content).map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;

    let tools = config
        .tools
        .into_iter()
        .map(|tc| {
            let backend = match tc.backend {
                ToolBackendConfig::Http {
                    url,
                    method,
                    headers,
                    timeout_secs,
                } => ToolBackend::Http {
                    method: method.unwrap_or_else(|| "GET".to_string()),
                    url,
                    headers: headers.unwrap_or_default(),
                    timeout_secs: timeout_secs.unwrap_or(30),
                },
                ToolBackendConfig::Command {
                    command,
                    args_template,
                    timeout_secs,
                } => ToolBackend::Command {
                    command,
                    args_template,
                    timeout_secs: timeout_secs.unwrap_or(30),
                },
            };

            RegisteredTool {
                definition: ToolDefinition {
                    name: tc.name,
                    description: tc.description,
                    parameters: tc.parameters,
                },
                backend,
            }
        })
        .collect();

    Ok(tools)
}

/// Scan a directory for `*.toml` files and load tools from each.
/// Logs warnings for files that fail to parse.
pub fn load_tools_from_dir(dir: &Path) -> Vec<RegisteredTool> {
    let mut all_tools = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return all_tools,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "toml") {
            match load_tools_from_file(&path) {
                Ok(tools) => all_tools.extend(tools),
                Err(e) => {
                    tracing::warn!("Failed to load tools from {}: {}", path.display(), e);
                }
            }
        }
    }

    all_tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_toml_config() {
        let toml_str = r#"
[[tools]]
name = "weather_lookup"
description = "Get weather"

[tools.parameters]
type = "object"
required = ["location"]

[tools.parameters.properties.location]
type = "string"
description = "City name"

[tools.backend]
type = "http"
url = "https://api.example.com/weather?q={location}"
method = "GET"
timeout_secs = 10
"#;

        let config: ToolConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.tools.len(), 1);
        assert_eq!(config.tools[0].name, "weather_lookup");
        assert_eq!(config.tools[0].description, "Get weather");
        match &config.tools[0].backend {
            ToolBackendConfig::Http { url, method, timeout_secs, .. } => {
                assert!(url.contains("example.com"));
                assert_eq!(method.as_deref(), Some("GET"));
                assert_eq!(*timeout_secs, Some(10));
            }
            _ => panic!("Expected Http backend"),
        }
    }

    #[test]
    fn test_parse_command_backend() {
        let toml_str = r#"
[[tools]]
name = "git_log"
description = "Show git log"

[tools.parameters]
type = "object"

[tools.backend]
type = "command"
command = "git"
args_template = "log --oneline -n {count}"
timeout_secs = 15
"#;

        let config: ToolConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.tools.len(), 1);
        match &config.tools[0].backend {
            ToolBackendConfig::Command { command, args_template, timeout_secs } => {
                assert_eq!(command, "git");
                assert_eq!(args_template.as_deref(), Some("log --oneline -n {count}"));
                assert_eq!(*timeout_secs, Some(15));
            }
            _ => panic!("Expected Command backend"),
        }
    }

    #[test]
    fn test_load_from_dir_nonexistent() {
        let tools = load_tools_from_dir(Path::new("/nonexistent/path"));
        assert!(tools.is_empty());
    }

    #[test]
    fn test_load_from_dir_with_file() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
[[tools]]
name = "test_tool"
description = "A test tool"

[tools.parameters]
type = "object"

[tools.backend]
type = "command"
command = "echo"
args_template = "hello"
"#;
        std::fs::write(dir.path().join("test.toml"), toml_content).unwrap();

        let tools = load_tools_from_dir(dir.path());
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].definition.name, "test_tool");
    }
}
