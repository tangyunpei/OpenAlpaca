use std::sync::Arc;

use async_trait::async_trait;
use openalpaca_api::plugin_traits::PluginToolExecutor;
use openalpaca_core::tools::extensions::ExtensionLedger;
use serde_json::Value;

use crate::bridge::LoadBinding;
use crate::stdio_channel::StdioChannel;

/// Proxies tool calls to a plugin process via [`StdioChannel`].
///
/// Sends a `tools/call` JSON-RPC request with the bare tool name and arguments,
/// then parses the MCP response format, concatenating all text content items.
///
/// It carries the **load's generation** (extension design §3.0 Fact 3): a deep
/// registry snapshot taken before a disable → re-enable still holds this proxy
/// over a channel whose process is dead, and without the number its
/// `ChannelClosed` would `mark_failed` the *healthy* incarnation that replaced
/// it. With it, the ledger no-ops that call and the run is told its copy is
/// stale.
pub struct PluginToolProxy {
    plugin_id: String,
    channel: StdioChannel,
    load: LoadBinding,
}

impl PluginToolProxy {
    pub fn new(
        plugin_id: String,
        channel: StdioChannel,
        generation: u64,
        ledger: Arc<ExtensionLedger>,
    ) -> Self {
        let load = LoadBinding::new(&plugin_id, generation, ledger);
        Self {
            plugin_id,
            channel,
            load,
        }
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
                if let Some(arr) = result.get("content").and_then(|c| c.as_array()) {
                    let texts: Vec<&str> = arr
                        .iter()
                        .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                        .collect();
                    if !texts.is_empty() {
                        return Ok(texts.join("\n"));
                    }
                }
                // Fallback: return stringified result
                Ok(result.to_string())
            }
            Err(e) => Err(self.load.describe_failure(tool_name, Some(tool_name), &e)),
        }
    }

    fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    fn generation(&self) -> u64 {
        self.load.generation()
    }
}
