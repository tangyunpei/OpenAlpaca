use std::time::Duration;

use serde_json::Value;

/// Trait for executing tools via a plugin subprocess.
/// Defined in openalpaca_api to avoid circular dependencies:
/// openalpaca_core uses it in ToolBackend::Plugin, openalpaca_plugins implements it.
///
/// Its methods return `impl Future + Send`, so an implementor writes plain
/// `async fn`; a holder of a type-erased executor holds
/// [`DynPluginToolExecutor`] (`Arc<DynPluginToolExecutor<'static>>`), built
/// with `DynPluginToolExecutor::new_arc` or `from_arc`.
#[dynosaur::dynosaur(pub DynPluginToolExecutor = dyn(box) PluginToolExecutor)]
pub trait PluginToolExecutor: Send + Sync {
    /// Execute a tool by name with JSON arguments.
    ///
    /// `tool_name` is the name the tool is registered under,
    /// `<plugin>::<tool>` (the registry's `Plugin` arm passes its own); the
    /// implementor sends its plugin the bare `<tool>` the plugin listed.
    fn execute(
        &self,
        tool_name: &str,
        arguments: &Value,
    ) -> impl Future<Output = Result<String, String>> + Send;

    /// [`Self::execute`] for a call the sandbox cuts at `sandbox_cut`
    /// (PLUGIN-WAIT): the registry's `Plugin` arm calls this where the call's
    /// context carries the sandbox's figure, and `execute` where it carries
    /// none. An implementor that waits for its plugin's answer waits past
    /// `sandbox_cut`, so the sandbox's clock, with its own message, is the
    /// one that ends a slow call.
    ///
    /// Default: [`Self::execute`], so no existing implementor changes.
    fn execute_within(
        &self,
        tool_name: &str,
        arguments: &Value,
        sandbox_cut: Duration,
    ) -> impl Future<Output = Result<String, String>> + Send {
        async move {
            let _ = sandbox_cut;
            self.execute(tool_name, arguments).await
        }
    }

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
///
/// Its method returns `impl Future + Send`, so an implementor writes plain
/// `async fn`; a plugin skill receives the callback as
/// [`DynToolCallbackExecutor`] (`&DynToolCallbackExecutor<'_>`), built with
/// `DynToolCallbackExecutor::from_ref`.
#[dynosaur::dynosaur(pub DynToolCallbackExecutor = dyn(box) ToolCallbackExecutor)]
pub trait ToolCallbackExecutor: Send + Sync {
    fn execute_tool(
        &self,
        tool_name: &str,
        arguments: &Value,
    ) -> impl Future<Output = Result<String, String>> + Send;
}

/// Trait for executing skills via a plugin subprocess.
/// The skill/invoke RPC may request tool callbacks, forming a loop until
/// the skill returns a final result with no tool_calls.
///
/// Its method returns `impl Future + Send`, so an implementor writes plain
/// `async fn`; the skill catalog holds a plugin's executor as
/// [`DynPluginSkillExecutor`] (`Arc<DynPluginSkillExecutor<'static>>`),
/// built with `DynPluginSkillExecutor::from_arc`.
#[dynosaur::dynosaur(pub DynPluginSkillExecutor = dyn(box) PluginSkillExecutor)]
pub trait PluginSkillExecutor: Send + Sync {
    /// Invoke the skill with a query.
    /// The executor handles the tool callback loop internally.
    fn invoke(
        &self,
        query: &str,
        context: &Value,
        tool_executor: &DynToolCallbackExecutor<'_>,
    ) -> impl Future<Output = Result<String, String>> + Send;

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
///
/// Its methods return `impl Future + Send`, so an implementor writes plain
/// `async fn`; a plugin's agent template holds its executor as
/// [`DynPluginAgentExecutor`] (`Arc<DynPluginAgentExecutor<'static>>`),
/// built with `DynPluginAgentExecutor::new_arc` or `from_arc`.
#[dynosaur::dynosaur(pub DynPluginAgentExecutor = dyn(box) PluginAgentExecutor)]
pub trait PluginAgentExecutor: Send + Sync {
    /// Spawn an agent instance for a task.
    /// Returns true if the agent accepted the task.
    fn spawn(
        &self,
        instance_id: &str,
        task_id: &str,
        instructions: &str,
        context: &Value,
    ) -> impl Future<Output = Result<bool, String>> + Send;

    /// Poll for progress or send tool results back.
    /// Returns (status, output, tool_calls).
    /// status: "working" | "tool_request" | "complete" | "failed"
    /// tool_calls: empty unless status is "tool_request"
    fn step(
        &self,
        instance_id: &str,
        tool_results: Option<&Value>,
    ) -> impl Future<Output = Result<(String, String, Vec<Value>), String>> + Send;

    /// Stop a running agent instance.
    fn stop(&self, instance_id: &str) -> impl Future<Output = Result<(), String>> + Send;

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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::Value;

    use super::{
        DynPluginAgentExecutor, DynPluginSkillExecutor, DynPluginToolExecutor,
        DynToolCallbackExecutor, PluginToolExecutor, ToolCallbackExecutor,
    };

    /// Compiles only for a `Send` value: applied to an unpolled future.
    fn is_send<T: Send>(_: &T) {}

    /// A type-erased executor crosses tasks: the registry holds it in
    /// `ToolBackend::Plugin` and calls it from spawned work.
    ///
    /// The supertraits alone make the held form `Send + Sync`, so the test
    /// also holds each method's declared `+ Send`: the generic body sees
    /// only the trait's declaration, and naming it at the Dyn compiles it.
    #[test]
    fn a_held_plugin_tool_executor_is_send_and_sync() {
        fn send_and_sync<T: Send + Sync + ?Sized>() {}
        send_and_sync::<Arc<DynPluginToolExecutor<'static>>>();

        fn futures_are_send<E: PluginToolExecutor + ?Sized>(e: &E, args: &Value) {
            is_send(&e.execute("t", args));
            is_send(&e.execute_within("t", args, Duration::ZERO));
        }
        let _ = futures_are_send::<DynPluginToolExecutor<'static>>;
    }

    /// A type-erased tool callback rides inside a plugin skill's boxed,
    /// `Send` invoke future, so the held form is `Send + Sync`; its method's
    /// declared `+ Send` is held as for `PluginToolExecutor`.
    #[test]
    fn a_held_tool_callback_executor_is_send_and_sync() {
        fn send_and_sync<T: Send + Sync + ?Sized>() {}
        send_and_sync::<Arc<DynToolCallbackExecutor<'static>>>();

        fn futures_are_send<E: ToolCallbackExecutor + ?Sized>(e: &E, args: &Value) {
            is_send(&e.execute_tool("t", args));
        }
        let _ = futures_are_send::<DynToolCallbackExecutor<'static>>;
    }

    /// A type-erased skill executor is held in the skill catalog's
    /// `SkillSource::Plugin` and invoked from spawned turns.
    #[test]
    fn a_held_plugin_skill_executor_is_send_and_sync() {
        fn send_and_sync<T: Send + Sync + ?Sized>() {}
        send_and_sync::<Arc<DynPluginSkillExecutor<'static>>>();
    }

    /// A type-erased agent executor is held in a plugin template's
    /// `AgentSource::Plugin` and driven from spawned subagent work.
    #[test]
    fn a_held_plugin_agent_executor_is_send_and_sync() {
        fn send_and_sync<T: Send + Sync + ?Sized>() {}
        send_and_sync::<Arc<DynPluginAgentExecutor<'static>>>();
    }
}
