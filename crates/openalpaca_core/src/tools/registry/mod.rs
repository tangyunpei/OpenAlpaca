use async_trait::async_trait;
use dashmap::DashMap;
use openalpaca_llm::ToolDefinition;
use std::collections::HashMap;
use std::sync::Arc;

/// Per-invocation execution context passed to tools that need identity.
/// Lightweight — no Arc deps, no DB handles. Just identity strings.
#[derive(Debug, Clone, Default)]
pub struct ToolContext {
    pub agent_id: Option<String>,
    pub task_id: Option<String>,
    pub owner_id: Option<String>,
    pub workspace_id: Option<String>,
}

/// Backend that executes a tool's logic.
#[derive(Clone)]
pub enum ToolBackend {
    BuiltIn(Arc<dyn BuiltInTool>),
    Http {
        method: String,
        url: String,
        headers: HashMap<String, String>,
        timeout_secs: u64,
    },
    Command {
        command: String,
        args_template: Option<String>,
        timeout_secs: u64,
    },
    Plugin(Arc<dyn openalpaca_api::plugin_traits::PluginToolExecutor>),
}

/// Trait for built-in tool implementations.
#[async_trait]
pub trait BuiltInTool: Send + Sync {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String>;

    /// Execute with per-invocation context. Default delegates to execute().
    /// Override for tools that need identity (owner_id, task_id, etc.).
    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<String, String> {
        self.execute(arguments).await
    }
}

/// A tool registered in the registry: its LLM-facing definition + execution backend.
#[derive(Clone)]
pub struct RegisteredTool {
    pub definition: ToolDefinition,
    pub backend: ToolBackend,
    pub provides_capabilities: Vec<String>,
    /// When true, SandboxManager skips the per-tool timeout for this tool.
    /// Used for coordination tools that manage their own timeouts.
    pub exempt_from_timeout: bool,
}

/// Central registry mapping tool names to definitions and execution backends.
///
/// Backed by `DashMap` for lock-free concurrent reads and writes.
/// Shared as `Arc<ToolRegistry>` — tools can be registered and removed at runtime
/// (e.g. when plugins load/unload) without requiring `&mut self`.
pub struct ToolRegistry {
    tools: DashMap<String, RegisteredTool>,
    http_client: reqwest::Client,
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        let new_tools = DashMap::new();
        for entry in self.tools.iter() {
            new_tools.insert(entry.key().clone(), entry.value().clone());
        }
        Self {
            tools: new_tools,
            http_client: self.http_client.clone(),
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                attempt.error("too many redirects")
            } else if let Err(e) =
                crate::tools::url_validation::validate_url(attempt.url().as_str())
            {
                attempt.error(format!("redirect blocked by SSRF policy: {}", e))
            } else {
                attempt.follow()
            }
        });
        let http_client = reqwest::Client::builder()
            .redirect(redirect_policy)
            .build()
            .expect("Failed to create shared HTTP client");

        Self {
            tools: DashMap::new(),
            http_client,
        }
    }

    /// Register a tool. Safe to call at any time, including after wrapping in Arc.
    pub fn register(&self, tool: RegisteredTool) {
        self.tools.insert(tool.definition.name.clone(), tool);
    }

    /// Remove a tool by name. Returns true if the tool existed.
    pub fn remove(&self, name: &str) -> bool {
        self.tools.remove(name).is_some()
    }

    /// Look up a tool by name. Returns a clone because DashMap guards
    /// must not be held across await points.
    pub fn get(&self, name: &str) -> Option<RegisteredTool> {
        self.tools.get(name).map(|r| r.value().clone())
    }

    /// Execute a tool by name.
    ///
    /// Performs basic JSON Schema pre-validation (required fields and type check)
    /// before dispatching to the backend, catching obvious argument errors early.
    pub async fn execute(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String> {
        // Extract definition + backend and drop the DashMap guard before any .await
        let (definition, backend) = {
            let entry = self
                .tools
                .get(tool_name)
                .ok_or_else(|| format!("Unknown tool: '{}'", tool_name))?;
            (entry.definition.clone(), entry.backend.clone())
        };

        // Pre-validate arguments against the tool's JSON Schema definition.
        validate_tool_arguments(&definition, arguments)?;

        match &backend {
            ToolBackend::BuiltIn(handler) => handler.execute(arguments).await,
            ToolBackend::Http {
                method,
                url,
                headers,
                timeout_secs,
            } => {
                execute_http(&self.http_client, method, url, headers, *timeout_secs, arguments)
                    .await
            }
            ToolBackend::Command {
                command,
                args_template,
                timeout_secs,
            } => execute_command(command, args_template.as_deref(), *timeout_secs, arguments).await,
            ToolBackend::Plugin(executor) => executor.execute(tool_name, arguments).await,
        }
    }

    /// Execute a tool by name with per-invocation context.
    /// Routes to BuiltInTool::execute_with_context() for BuiltIn backends.
    /// For Http/Command/Plugin backends, context is ignored (they don't need identity).
    pub async fn execute_with_context(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        // Extract definition + backend and drop the DashMap guard before any .await
        let (definition, backend) = {
            let entry = self
                .tools
                .get(tool_name)
                .ok_or_else(|| format!("Tool '{}' not found in registry", tool_name))?;
            (entry.definition.clone(), entry.backend.clone())
        };

        // Pre-validate arguments (same validation as execute())
        validate_tool_arguments(&definition, arguments)?;

        match &backend {
            ToolBackend::BuiltIn(implementation) => {
                implementation.execute_with_context(arguments, ctx).await
            }
            ToolBackend::Http { .. } | ToolBackend::Command { .. } | ToolBackend::Plugin(_) => {
                self.execute(tool_name, arguments).await
            }
        }
    }

    /// List registered tool names.
    pub fn registered_tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|e| e.key().clone()).collect()
    }

    /// Number of registered tools.
    pub fn count(&self) -> usize {
        self.tools.len()
    }

    /// Check if a tool is exempt from the per-tool sandbox timeout.
    pub fn is_exempt_from_timeout(&self, tool_name: &str) -> bool {
        self.tools
            .get(tool_name)
            .map(|t| t.value().exempt_from_timeout)
            .unwrap_or(false)
    }

    /// Returns tool definitions for all tools whose `provides_capabilities`
    /// intersects with the requested capabilities. Empty capabilities returns empty.
    pub fn tools_for_capabilities(&self, capabilities: &[String]) -> Vec<ToolDefinition> {
        if capabilities.is_empty() {
            return vec![];
        }
        self.tools
            .iter()
            .filter(|entry| {
                entry.value().provides_capabilities
                    .iter()
                    .any(|cap| capabilities.contains(cap))
            })
            .map(|entry| entry.value().definition.clone())
            .collect()
    }

    /// Returns tool definitions for all tools whose `provides_capabilities`
    /// intersects with the requested capabilities, excluding tools that provide
    /// any denied capability.
    pub fn tools_for_capabilities_with_deny(
        &self,
        capabilities: &[String],
        denied: &[String],
    ) -> Vec<ToolDefinition> {
        if capabilities.is_empty() {
            return vec![];
        }
        self.tools
            .iter()
            .filter(|entry| {
                entry.value().provides_capabilities
                    .iter()
                    .any(|cap| capabilities.contains(cap))
            })
            .filter(|entry| {
                !entry.value().provides_capabilities
                    .iter()
                    .any(|cap| denied.contains(cap))
            })
            .map(|entry| entry.value().definition.clone())
            .collect()
    }

    /// Return the names of tools that use command backends (i.e., execute via shell).
    /// These should be treated like shell tools for injection sanitization.
    pub fn command_backend_tool_names(&self) -> Vec<String> {
        self.tools
            .iter()
            .filter_map(|entry| match &entry.value().backend {
                ToolBackend::Command { .. } => Some(entry.key().clone()),
                ToolBackend::BuiltIn(_) | ToolBackend::Http { .. } | ToolBackend::Plugin(_) => None,
            })
            .collect()
    }
}

/// Execute an HTTP backend tool call using the shared HTTP client.
async fn execute_http(
    client: &reqwest::Client,
    method: &str,
    url_template: &str,
    headers: &HashMap<String, String>,
    timeout_secs: u64,
    arguments: &serde_json::Value,
) -> Result<String, String> {
    // Replace {param_name} placeholders in URL with URL-encoded argument values
    let mut url = url_template.to_string();
    if let Some(obj) = arguments.as_object() {
        for (key, value) in obj {
            let replacement = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let encoded = urlencoding::encode(&replacement);
            url = url.replace(&format!("{{{}}}", key), &encoded);
        }
    }

    // Check for unsubstituted {param_name} placeholders.
    // These indicate missing required arguments and would produce malformed URLs.
    if let Some(start) = url.find('{')
        && let Some(end) = url[start..].find('}')
    {
        let placeholder = &url[start..start + end + 1];
        return Err(format!(
            "URL template has unsubstituted placeholder '{}' — \
             the required parameter was not provided in tool arguments",
            placeholder
        ));
    }

    // SSRF protection: validate the resolved URL before making the request.
    // The URL template may contain user-controlled placeholders that could
    // redirect requests to internal/private endpoints.
    crate::tools::url_validation::validate_url(&url)?;

    let timeout = std::time::Duration::from_secs(timeout_secs);

    let mut request = match method.to_uppercase().as_str() {
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        _ => client.get(&url),
    };

    for (k, v) in headers {
        request = request.header(k.as_str(), v.as_str());
    }

    request = request.timeout(timeout);

    let response = request
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    let status = response.status();

    // Stream body with size cap to prevent memory exhaustion.
    // Read at most MAX_HTTP_RESPONSE_BYTES before stopping.
    const MAX_HTTP_RESPONSE_BYTES: usize = 1_048_576; // 1 MB
    let body = {
        use futures_util::StreamExt;
        let mut body_bytes = Vec::with_capacity(8192);
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("Failed to read response body: {}", e))?;
            body_bytes.extend_from_slice(&chunk);
            if body_bytes.len() >= MAX_HTTP_RESPONSE_BYTES {
                body_bytes.truncate(MAX_HTTP_RESPONSE_BYTES);
                break;
            }
        }
        String::from_utf8_lossy(&body_bytes).into_owned()
    };

    if status.is_success() {
        // Truncate to 8KB
        Ok(body.chars().take(8192).collect())
    } else {
        Err(format!(
            "HTTP {} — {}",
            status,
            body.chars().take(1024).collect::<String>()
        ))
    }
}

/// Execute a command backend tool call.
async fn execute_command(
    command: &str,
    args_template: Option<&str>,
    timeout_secs: u64,
    arguments: &serde_json::Value,
) -> Result<String, String> {
    let mut full_args = args_template.unwrap_or("").to_string();
    if let Some(obj) = arguments.as_object() {
        for (key, value) in obj {
            let replacement = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let escaped = shell_escape::escape(replacement.into());
            full_args = full_args.replace(&format!("{{{}}}", key), &escaped);
        }
    }

    // Check for unsubstituted {param_name} placeholders.
    // These indicate missing required arguments and would produce malformed commands.
    if let Some(start) = full_args.find('{')
        && let Some(end) = full_args[start..].find('}')
    {
        let placeholder = &full_args[start..start + end + 1];
        return Err(format!(
            "Command template has unsubstituted placeholder '{}' — \
             the required parameter was not provided in tool arguments",
            placeholder
        ));
    }

    let timeout = std::time::Duration::from_secs(timeout_secs);
    let full_command = format!("{} {}", command, full_args);

    let output = tokio::time::timeout(timeout, {
        crate::tools::platform::shell_command(&full_command).output()
    })
    .await
    .map_err(|_| format!("Command timed out after {}s", timeout_secs))?
    .map_err(|e| format!("Failed to execute command: {}", e))?;

    // Cap output size to prevent memory pressure from commands producing
    // unbounded stdout/stderr.
    const MAX_OUTPUT_BYTES: usize = 512 * 1024; // 512 KB
    let stdout_raw = &output.stdout;
    let stderr_raw = &output.stderr;
    let stdout = String::from_utf8_lossy(&stdout_raw[..stdout_raw.len().min(MAX_OUTPUT_BYTES)]);
    let stderr = String::from_utf8_lossy(&stderr_raw[..stderr_raw.len().min(MAX_OUTPUT_BYTES)]);

    if output.status.success() {
        Ok(stdout.to_string())
    } else {
        Err(format!(
            "Command failed (exit {}): {}{}",
            output.status.code().unwrap_or(-1),
            stdout,
            stderr
        ))
    }
}

/// Basic JSON Schema pre-validation for tool arguments.
///
/// Checks:
/// - Arguments must be a JSON object (if the schema specifies `type: "object"`)
/// - All fields listed in `required` must be present
/// - Field types match the schema's `properties.<name>.type` when specified
///
/// This is not a full JSON Schema validator — it catches the most common
/// argument errors early (missing required fields, wrong root type) to provide
/// clear error messages instead of cryptic tool failures.
fn validate_tool_arguments(
    definition: &ToolDefinition,
    arguments: &serde_json::Value,
) -> Result<(), String> {
    let schema = &definition.parameters;

    // Check root type is object (if schema says so)
    if schema.get("type").and_then(|t| t.as_str()) == Some("object") && !arguments.is_object() {
        return Err(format!(
            "Tool '{}': arguments must be a JSON object, got {}",
            definition.name,
            json_type_name(arguments)
        ));
    }

    // Check required fields
    if let Some(required) = schema.get("required").and_then(|r| r.as_array())
        && let Some(obj) = arguments.as_object()
    {
        for req in required {
            if let Some(field_name) = req.as_str()
                && !obj.contains_key(field_name)
            {
                return Err(format!(
                    "Tool '{}': missing required parameter '{}'",
                    definition.name, field_name
                ));
            }
        }
    }

    // Check field types against schema properties
    if let (Some(properties), Some(obj)) = (
        schema.get("properties").and_then(|p| p.as_object()),
        arguments.as_object(),
    ) {
        for (key, value) in obj {
            if let Some(prop_schema) = properties.get(key)
                && let Some(expected_type) = prop_schema.get("type").and_then(|t| t.as_str())
                && !json_value_matches_type(value, expected_type)
            {
                return Err(format!(
                    "Tool '{}': parameter '{}' should be {}, got {}",
                    definition.name,
                    key,
                    expected_type,
                    json_type_name(value)
                ));
            }
        }
    }

    Ok(())
}

/// Return a human-readable type name for a JSON value.
fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Check if a JSON value matches the expected JSON Schema type.
fn json_value_matches_type(value: &serde_json::Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => true, // Unknown type — don't block
    }
}

#[cfg(test)]
mod tests;
