use async_trait::async_trait;

use crate::protocol::error::ErrorShape;
use crate::rpc::{RpcContext, RpcHandler};

pub struct ChannelsStatusHandler;

#[async_trait]
impl RpcHandler for ChannelsStatusHandler {
    async fn handle(
        &self,
        _params: serde_json::Value,
        ctx: &RpcContext,
    ) -> Result<serde_json::Value, ErrorShape> {
        let registry = ctx.state.channel_registry.read().await;
        let plugins = registry.list();

        let channels: Vec<serde_json::Value> = plugins
            .iter()
            .map(|plugin| {
                serde_json::json!({
                    "id": plugin.id().0,
                    "label": plugin.meta().label,
                    "capabilities": {
                        "chat_types": plugin.capabilities().chat_types.iter()
                            .map(|ct| ct.to_string())
                            .collect::<Vec<_>>(),
                        "media": plugin.capabilities().media,
                        "reactions": plugin.capabilities().reactions,
                        "threads": plugin.capabilities().threads,
                    }
                })
            })
            .collect();

        Ok(serde_json::json!({ "channels": channels }))
    }
}
