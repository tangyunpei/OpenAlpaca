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
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

    let config: ToolConfigFile = toml::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;

    let mut tools = Vec::new();
    for tc in config.tools {
        // Validate tool name is non-empty
        if tc.name.trim().is_empty() {
            return Err(format!("Tool in {} has empty name", path.display()));
        }

        let backend = match tc.backend {
            ToolBackendConfig::Http {
                url,
                method,
                headers,
                timeout_secs,
            } => {
                // Validate URL format
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    return Err(format!(
                        "Tool '{}': URL must start with http:// or https://, got '{}'",
                        tc.name, url
                    ));
                }
                // Validate timeout range
                let t = timeout_secs.unwrap_or(30);
                if t == 0 || t > 300 {
                    return Err(format!(
                        "Tool '{}': timeout_secs {} is out of valid range [1-300]",
                        tc.name, t
                    ));
                }
                ToolBackend::Http {
                    method: method.unwrap_or_else(|| "GET".to_string()),
                    url,
                    headers: headers.unwrap_or_default(),
                    timeout_secs: t,
                }
            }
            ToolBackendConfig::Command {
                command,
                args_template,
                timeout_secs,
            } => {
                // Validate timeout range
                let t = timeout_secs.unwrap_or(30);
                if t == 0 || t > 300 {
                    return Err(format!(
                        "Tool '{}': timeout_secs {} is out of valid range [1-300]",
                        tc.name, t
                    ));
                }
                ToolBackend::Command {
                    command,
                    args_template,
                    timeout_secs: t,
                }
            }
        };

        tools.push(RegisteredTool {
            definition: ToolDefinition {
                name: tc.name,
                description: tc.description,
                parameters: tc.parameters,
                strict: None,
                input_examples: None,
            },
            backend,
            provides_capabilities: vec![],
        });
    }

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
mod tests;
