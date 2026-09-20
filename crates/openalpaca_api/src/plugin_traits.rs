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

    /// Which load of the plugin this handle belongs to (extension design
    /// §3.0 Fact 3). The number rides inside the proxy, so no enum change is
    /// needed on `ToolBackend::Plugin`; the extension gate reads it back
    /// through `RegisteredTool::incarnation()` and refuses a handle from a
    /// previous load as `Stale`.
    ///
    /// Default `0` so no existing implementor changes.
    fn generation(&self) -> u64 {
        0
    }
}

/// Callback interface for executing tool calls requested by a plugin skill.
/// The daemon implements this to provide tool access to skills during invocation.
#[async_trait]
pub trait ToolCallbackExecutor: Send + Sync {
    async fn execute_tool(&self, tool_name: &str, arguments: &Value) -> Result<String, String>;
}

/// Trait for executing skills via a plugin subprocess.
/// The skill/invoke RPC may request tool callbacks, forming a loop until
/// the skill returns a final result with no tool_calls.
#[async_trait]
pub trait PluginSkillExecutor: Send + Sync {
    /// Invoke the skill with a query.
    /// The executor handles the tool callback loop internally.
    async fn invoke(
        &self,
        query: &str,
        context: &Value,
        tool_executor: &dyn ToolCallbackExecutor,
    ) -> Result<String, String>;

    fn plugin_id(&self) -> &str;
    fn skill_id(&self) -> &str;

    /// Which load of the plugin this bridge belongs to. The run-guard at
    /// `invoke_plugin_skill` passes it to `ledger.begin_run`, so a run is never
    /// started against a previous load (extension design §3.2 T3(b)).
    ///
    /// Default `0` so no existing implementor changes.
    fn generation(&self) -> u64 {
        0
    }
}

/// Trait for executing agent tasks via a plugin subprocess.
/// The agent manages its own LLM calls and reasoning loop.
/// The daemon dispatches tasks and proxies tool requests.
#[async_trait]
pub trait PluginAgentExecutor: Send + Sync {
    /// Spawn an agent instance for a task.
    /// Returns true if the agent accepted the task.
    async fn spawn(
        &self,
        instance_id: &str,
        task_id: &str,
        instructions: &str,
        context: &Value,
    ) -> Result<bool, String>;

    /// Poll for progress or send tool results back.
    /// Returns (status, output, tool_calls).
    /// status: "working" | "tool_request" | "complete" | "failed"
    /// tool_calls: empty unless status is "tool_request"
    async fn step(
        &self,
        instance_id: &str,
        tool_results: Option<&Value>,
    ) -> Result<(String, String, Vec<Value>), String>;

    /// Stop a running agent instance.
    async fn stop(&self, instance_id: &str) -> Result<(), String>;

    fn plugin_id(&self) -> &str;
    fn agent_id(&self) -> &str;

    /// Which load of the plugin this bridge belongs to. An in-flight subagent
    /// holds a cloned executor, so a lead that spawns from that template after
    /// a disable → re-enable is refused at the run-guard's pre-flight rather
    /// than at its first RPC (extension design §3.2 T3(b)).
    ///
    /// Default `0` so no existing implementor changes.
    fn generation(&self) -> u64 {
        0
    }
}
