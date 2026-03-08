use async_trait::async_trait;

use crate::protocol::error::ErrorShape;
use crate::rpc::{RpcContext, RpcHandler};

pub struct HealthHandler;

#[async_trait]
impl RpcHandler for HealthHandler {
    async fn handle(
        &self,
        _params: serde_json::Value,
        _ctx: &RpcContext,
    ) -> Result<serde_json::Value, ErrorShape> {
        Ok(serde_json::json!({ "ok": true }))
    }
}

pub struct StatusHandler;

#[async_trait]
impl RpcHandler for StatusHandler {
    async fn handle(
        &self,
        _params: serde_json::Value,
        ctx: &RpcContext,
    ) -> Result<serde_json::Value, ErrorShape> {
        let config = ctx.state.config.load();
        let port = config.gateway.as_ref().and_then(|g| g.port).unwrap_or(3777);
        let channels = ctx.state.channel_registry.read().await;
        let channel_count = channels.len();

        Ok(serde_json::json!({
            "ok": true,
            "port": port,
            "channels": channel_count,
            "version": env!("CARGO_PKG_VERSION"),
        }))
    }
}
