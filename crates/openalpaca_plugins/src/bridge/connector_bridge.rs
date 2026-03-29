use async_trait::async_trait;
use openalpaca_connectors::{Connector, ConnectorError};
use serde_json::Value;
use tracing::{debug, warn};

use crate::stdio_channel::StdioChannel;

/// Bridges a plugin process to the [`Connector`] trait.
///
/// Proxies `connector/start`, `connector/stop`, and `connector/send`
/// JSON-RPC calls to the plugin over [`StdioChannel`].
pub struct PluginConnector {
    plugin_id: String,
    platform: String,
    channel: StdioChannel,
}

impl PluginConnector {
    pub fn new(plugin_id: String, platform: String, channel: StdioChannel) -> Self {
        Self {
            plugin_id,
            platform,
            channel,
        }
    }

    /// Send a message through this connector plugin.
    pub async fn send_message(
        &self,
        chat_id: &str,
        text: &str,
        reply_to: Option<&str>,
        files: &[String],
    ) -> Result<Value, String> {
        let params = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "reply_to": reply_to,
            "files": files,
        });

        self.channel
            .call("connector/send", params)
            .await
            .map_err(|e| format!("plugin {}::connector/send: {}", self.plugin_id, e))
    }
}

#[async_trait]
impl Connector for PluginConnector {
    fn name(&self) -> &str {
        &self.platform
    }

    async fn run(&self) -> Result<(), ConnectorError> {
        debug!(
            plugin_id = %self.plugin_id,
            platform = %self.platform,
            "starting plugin connector"
        );

        let params = serde_json::json!({
            "plugin_id": self.plugin_id,
            "platform": self.platform,
        });

        self.channel
            .call("connector/start", params)
            .await
            .map_err(|e| {
                warn!(
                    plugin_id = %self.plugin_id,
                    error = %e,
                    "plugin connector start failed"
                );
                ConnectorError::InitFailed(format!(
                    "plugin {} connector/start: {}",
                    self.plugin_id, e
                ))
            })?;

        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ConnectorError> {
        debug!(
            plugin_id = %self.plugin_id,
            platform = %self.platform,
            "shutting down plugin connector"
        );

        let params = serde_json::json!({
            "plugin_id": self.plugin_id,
            "platform": self.platform,
        });

        self.channel
            .call("connector/stop", params)
            .await
            .map_err(|e| {
                warn!(
                    plugin_id = %self.plugin_id,
                    error = %e,
                    "plugin connector stop failed"
                );
                ConnectorError::Internal(format!(
                    "plugin {} connector/stop: {}",
                    self.plugin_id, e
                ))
            })?;

        Ok(())
    }
}
