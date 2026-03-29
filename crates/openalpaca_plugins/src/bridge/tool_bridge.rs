use async_trait::async_trait;
use openalpaca_api::plugin_traits::PluginToolExecutor;
use serde_json::Value;

use crate::stdio_channel::StdioChannel;

/// Proxies tool calls to a plugin process via [`StdioChannel`].
///
/// Sends a `tools/call` JSON-RPC request with the bare tool name and arguments,
/// then parses the MCP response format, concatenating all text content items.
pub struct PluginToolProxy {
    plugin_id: String,
    channel: StdioChannel,
}

impl PluginToolProxy {
    pub fn new(plugin_id: String, channel: StdioChannel) -> Self {
        Self { plugin_id, channel }
    }
}

#[async_trait]
impl PluginToolExecutor for PluginToolProxy {
    async fn execute(&self, tool_name: &str, arguments: &Value) -> Result<String, String> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
        });

        match self.channel.call("tools/call", params).await {
            Ok(result) => {
                // MCP tools/call returns { content: [{ type: "text", text: "..." }] }
                if let Some(content) = result.get("content") {
                    if let Some(arr) = content.as_array() {
                        let texts: Vec<&str> = arr
                            .iter()
                            .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                            .collect();
                        if !texts.is_empty() {
                            return Ok(texts.join("\n"));
                        }
                    }
                }
                // Fallback: return stringified result
                Ok(result.to_string())
            }
            Err(e) => Err(format!("plugin {}::{}: {}", self.plugin_id, tool_name, e)),
        }
    }

    fn plugin_id(&self) -> &str {
        &self.plugin_id
    }
}
