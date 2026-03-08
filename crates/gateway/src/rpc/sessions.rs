use async_trait::async_trait;

use crate::protocol::error::ErrorShape;
use crate::rpc::{RpcContext, RpcHandler};

pub struct SessionsListHandler;

#[async_trait]
impl RpcHandler for SessionsListHandler {
    async fn handle(
        &self,
        _params: serde_json::Value,
        ctx: &RpcContext,
    ) -> Result<serde_json::Value, ErrorShape> {
        let entries = ctx
            .state
            .session_store
            .list()
            .await
            .map_err(|e| ErrorShape::internal(e.to_string()))?;
        let json =
            serde_json::to_value(&entries).map_err(|e| ErrorShape::internal(e.to_string()))?;
        Ok(serde_json::json!({ "sessions": json }))
    }
}

pub struct SessionsDeleteHandler;

#[async_trait]
impl RpcHandler for SessionsDeleteHandler {
    async fn handle(
        &self,
        params: serde_json::Value,
        ctx: &RpcContext,
    ) -> Result<serde_json::Value, ErrorShape> {
        let key = params
            .get("session_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ErrorShape::invalid_params("missing 'session_key' parameter"))?;

        ctx.state
            .session_store
            .remove(key)
            .await
            .map_err(|e| ErrorShape::internal(e.to_string()))?;

        Ok(serde_json::json!({ "ok": true, "deleted": key }))
    }
}
