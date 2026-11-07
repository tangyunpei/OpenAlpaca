use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use openalpaca_api::plugin_traits::PluginToolExecutor;
use openalpaca_core::tools::extensions::ExtensionLedger;
use serde_json::Value;

use crate::bridge::LoadBinding;
use crate::stdio_channel::StdioChannel;

/// How far past the sandbox's cut a tool call waits for its plugin's answer
/// (PLUGIN-WAIT). The sandbox arms its clock first, and a wait at the same
/// figure would race it — the two firing in one tick, the plugin channel's
/// "timeout waiting for plugin response" could win — so this one ends
/// later, and the sandbox's cut, with its own message and record, is what a
/// slow plugin tool meets. It never ends a call itself: the cut drops the
/// wait first.
const PAST_THE_SANDBOX: Duration = Duration::from_secs(5);

/// Proxies tool calls to a plugin process via [`StdioChannel`].
///
/// Sends a `tools/call` JSON-RPC request with the tool's name as the plugin
/// listed it and the arguments, then parses the MCP response format,
/// concatenating all text content items. The registry hands over its own
/// name, `<plugin>::<tool>`; the plugin never sees that prefix. A call the
/// sandbox cuts ([`PluginToolExecutor::execute_within`]) waits for the
/// answer past that cut; one no sandbox cuts waits the channel's default,
/// as every other request to the plugin does.
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

    /// Send `tools/call` and read its answer, waiting `wait` for it — the
    /// channel's default when `None`.
    async fn call(
        &self,
        tool_name: &str,
        arguments: &Value,
        wait: Option<Duration>,
    ) -> Result<String, String> {
        // The name the plugin listed: the registry's, less this plugin's own
        // `<plugin>::` (the prefix `discover_tools` gave it). A name without
        // it is already bare.
        let listed = tool_name
            .strip_prefix(self.plugin_id.as_str())
            .and_then(|rest| rest.strip_prefix("::"))
            .unwrap_or(tool_name);
        let params = serde_json::json!({
            "name": listed,
            "arguments": arguments,
        });

        let answer = match wait {
            Some(wait) => {
                self.channel
                    .call_with_timeout("tools/call", params, wait)
                    .await
            }
            None => self.channel.call("tools/call", params).await,
        };
        match answer {
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
            // `describe_failure` puts the plugin's name before the subject,
            // so the subject is the listed name; a refusal keeps the
            // registry's, the name the model called.
            Err(e) => Err(self.load.describe_failure(listed, Some(tool_name), &e)),
        }
    }
}

#[async_trait]
impl PluginToolExecutor for PluginToolProxy {
    async fn execute(&self, tool_name: &str, arguments: &Value) -> Result<String, String> {
        self.call(tool_name, arguments, None).await
    }

    async fn execute_within(
        &self,
        tool_name: &str,
        arguments: &Value,
        sandbox_cut: Duration,
    ) -> Result<String, String> {
        self.call(tool_name, arguments, Some(sandbox_cut + PAST_THE_SANDBOX))
            .await
    }

    fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    fn generation(&self) -> u64 {
        self.load.generation()
    }
}
