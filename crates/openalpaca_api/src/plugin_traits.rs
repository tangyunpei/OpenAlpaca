use async_trait::async_trait;
use serde_json::Value;

/// Trait for executing tools via a plugin subprocess.
/// Defined in openalpaca_api to avoid circular dependencies:
/// openalpaca_core uses it in ToolBackend::Plugin, openalpaca_plugins implements it.
#[async_trait]
pub trait PluginToolExecutor: Send + Sync {
    /// Execute a tool by name with JSON arguments.
    /// The tool_name is the bare name (without plugin namespace prefix).
    async fn execute(&self, tool_name: &str, arguments: &Value) -> Result<String, String>;

    /// The plugin this executor belongs to.
    fn plugin_id(&self) -> &str;
}
