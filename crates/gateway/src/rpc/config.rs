use async_trait::async_trait;

use crate::protocol::error::ErrorShape;
use crate::rpc::{RpcContext, RpcHandler};

pub struct ConfigGetHandler;

#[async_trait]
impl RpcHandler for ConfigGetHandler {
    async fn handle(
        &self,
        params: serde_json::Value,
        ctx: &RpcContext,
    ) -> Result<serde_json::Value, ErrorShape> {
        let config = ctx.state.config.load();
        let key = params
            .get("key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        match key {
            Some(path) => match openalpaca_config::io::get_value_at_path(&config, &path) {
                Some(value) => {
                    let json = serde_json::to_value(value)
                        .map_err(|e| ErrorShape::internal(e.to_string()))?;
                    Ok(json)
                }
                None => Err(ErrorShape::not_found(format!("key not found: {path}"))),
            },
            None => {
                let json = serde_json::to_value(config.as_ref())
                    .map_err(|e| ErrorShape::internal(e.to_string()))?;
                Ok(json)
            }
        }
    }
}

pub struct ConfigSetHandler;

#[async_trait]
impl RpcHandler for ConfigSetHandler {
    async fn handle(
        &self,
        params: serde_json::Value,
        _ctx: &RpcContext,
    ) -> Result<serde_json::Value, ErrorShape> {
        let key = params
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ErrorShape::invalid_params("missing 'key' parameter"))?;
        let _value = params
            .get("value")
            .ok_or_else(|| ErrorShape::invalid_params("missing 'value' parameter"))?;

        // Config set via RPC is deferred — for now return acknowledgment
        Ok(serde_json::json!({
            "ok": true,
            "key": key,
            "message": "config.set via RPC is not yet implemented"
        }))
    }
}
