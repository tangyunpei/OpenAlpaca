use async_trait::async_trait;
use openalpaca_api::plugin_traits::{PluginSkillExecutor, ToolCallbackExecutor};
use serde_json::Value;
use tracing::{debug, warn};

use crate::stdio_channel::StdioChannel;

/// Maximum tool-callback iterations before aborting the skill invocation.
const MAX_TOOL_LOOP_ITERATIONS: usize = 20;

/// Proxies skill invocations to a plugin process via [`StdioChannel`].
///
/// Sends `skill/invoke` JSON-RPC requests and handles the tool callback loop:
/// if the plugin responds with `tool_calls`, each call is executed via the
/// provided [`ToolCallbackExecutor`], and results are sent back with
/// `skill/invoke_continue`. The loop repeats until the plugin returns an
/// empty `tool_calls` array (or no `tool_calls` field), up to
/// [`MAX_TOOL_LOOP_ITERATIONS`].
pub struct PluginSkillBridge {
    plugin_id: String,
    skill_id: String,
    channel: StdioChannel,
}

impl PluginSkillBridge {
    pub fn new(plugin_id: String, skill_id: String, channel: StdioChannel) -> Self {
        Self {
            plugin_id,
            skill_id,
            channel,
        }
    }
}

#[async_trait]
impl PluginSkillExecutor for PluginSkillBridge {
    async fn invoke(
        &self,
        query: &str,
        context: &Value,
        tool_executor: &dyn ToolCallbackExecutor,
    ) -> Result<String, String> {
        // Gather available tool names from context (if provided)
        let available_tools = context
            .get("available_tools")
            .cloned()
            .unwrap_or(Value::Array(Vec::new()));

        let params = serde_json::json!({
            "query": query,
            "context": context,
            "available_tools": available_tools,
        });

        debug!(
            plugin = %self.plugin_id,
            skill = %self.skill_id,
            "sending skill/invoke"
        );

        let mut response = self
            .channel
            .call("skill/invoke", params)
            .await
            .map_err(|e| format!("plugin {}::skill/invoke: {}", self.plugin_id, e))?;

        // Tool callback loop
        for iteration in 0..MAX_TOOL_LOOP_ITERATIONS {
            let tool_calls = match response.get("tool_calls").and_then(|v| v.as_array()) {
                Some(calls) if !calls.is_empty() => calls.clone(),
                _ => break, // No tool calls — we're done
            };

            debug!(
                plugin = %self.plugin_id,
                skill = %self.skill_id,
                iteration,
                tool_count = tool_calls.len(),
                "executing tool callbacks"
            );

            // Execute each tool call
            let mut tool_results = Vec::new();
            for tc in &tool_calls {
                let tool_name = tc
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default();
                let arguments = tc
                    .get("arguments")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default()));
                let call_id = tc
                    .get("id")
                    .and_then(|id| id.as_str())
                    .unwrap_or_default()
                    .to_string();

                let result = match tool_executor.execute_tool(tool_name, &arguments).await {
                    Ok(output) => serde_json::json!({
                        "id": call_id,
                        "name": tool_name,
                        "result": output,
                        "is_error": false,
                    }),
                    Err(err) => serde_json::json!({
                        "id": call_id,
                        "name": tool_name,
                        "result": err,
                        "is_error": true,
                    }),
                };
                tool_results.push(result);
            }

            // Send results back to the plugin
            let continue_params = serde_json::json!({
                "tool_results": tool_results,
            });

            response = self
                .channel
                .call("skill/invoke_continue", continue_params)
                .await
                .map_err(|e| {
                    format!(
                        "plugin {}::skill/invoke_continue (iteration {}): {}",
                        self.plugin_id, iteration, e
                    )
                })?;
        }

        // Extract the final result
        if let Some(result) = response.get("result").and_then(|r| r.as_str()) {
            Ok(result.to_string())
        } else if let Some(result) = response.get("result") {
            Ok(result.to_string())
        } else {
            // Fallback: return the entire response as a string
            warn!(
                plugin = %self.plugin_id,
                skill = %self.skill_id,
                "skill/invoke response missing 'result' field"
            );
            Ok(response.to_string())
        }
    }

    fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    fn skill_id(&self) -> &str {
        &self.skill_id
    }
}
