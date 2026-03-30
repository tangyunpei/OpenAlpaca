use async_trait::async_trait;
use openalpaca_api::plugin_traits::PluginAgentExecutor;
use serde_json::Value;
use tracing::{debug, warn};

use crate::stdio_channel::StdioChannel;

/// Proxies agent lifecycle RPCs to a plugin process via [`StdioChannel`].
///
/// Bridges the [`PluginAgentExecutor`] trait to `agent/spawn`, `agent/step`,
/// and `agent/stop` JSON-RPC methods on the plugin's stdio channel.
pub struct PluginAgentBridge {
    plugin_id: String,
    agent_id: String,
    channel: StdioChannel,
}

impl PluginAgentBridge {
    pub fn new(plugin_id: String, agent_id: String, channel: StdioChannel) -> Self {
        Self {
            plugin_id,
            agent_id,
            channel,
        }
    }
}

#[async_trait]
impl PluginAgentExecutor for PluginAgentBridge {
    async fn spawn(
        &self,
        instance_id: &str,
        task_id: &str,
        instructions: &str,
        context: &Value,
    ) -> Result<bool, String> {
        let params = serde_json::json!({
            "instance_id": instance_id,
            "task_id": task_id,
            "instructions": instructions,
            "context": context,
        });

        debug!(
            plugin = %self.plugin_id,
            agent = %self.agent_id,
            instance_id,
            task_id,
            "sending agent/spawn"
        );

        let response = self
            .channel
            .call("agent/spawn", params)
            .await
            .map_err(|e| format!("plugin {}::agent/spawn: {}", self.plugin_id, e))?;

        let accepted = response
            .get("accepted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok(accepted)
    }

    async fn step(
        &self,
        instance_id: &str,
        tool_results: Option<&Value>,
    ) -> Result<(String, String, Vec<Value>), String> {
        let params = serde_json::json!({
            "instance_id": instance_id,
            "tool_results": tool_results,
        });

        debug!(
            plugin = %self.plugin_id,
            agent = %self.agent_id,
            instance_id,
            has_tool_results = tool_results.is_some(),
            "sending agent/step"
        );

        let response = self
            .channel
            .call("agent/step", params)
            .await
            .map_err(|e| format!("plugin {}::agent/step: {}", self.plugin_id, e))?;

        let status = response
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("failed")
            .to_string();

        let output = response
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let tool_calls = response
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        if status == "failed" && output.is_empty() {
            warn!(
                plugin = %self.plugin_id,
                agent = %self.agent_id,
                instance_id,
                "agent/step returned failed status with no output"
            );
        }

        Ok((status, output, tool_calls))
    }

    async fn stop(&self, instance_id: &str) -> Result<(), String> {
        let params = serde_json::json!({
            "instance_id": instance_id,
        });

        debug!(
            plugin = %self.plugin_id,
            agent = %self.agent_id,
            instance_id,
            "sending agent/stop"
        );

        self.channel
            .call("agent/stop", params)
            .await
            .map_err(|e| format!("plugin {}::agent/stop: {}", self.plugin_id, e))?;

        Ok(())
    }

    fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    fn agent_id(&self) -> &str {
        &self.agent_id
    }
}
