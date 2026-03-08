use async_trait::async_trait;

use crate::protocol::error::ErrorShape;
use crate::rpc::{RpcContext, RpcHandler};

pub struct ModelsListHandler;

#[async_trait]
impl RpcHandler for ModelsListHandler {
    async fn handle(
        &self,
        _params: serde_json::Value,
        _ctx: &RpcContext,
    ) -> Result<serde_json::Value, ErrorShape> {
        // Stub — full model catalog will be implemented in Phase 6
        Ok(serde_json::json!({
            "models": [],
            "message": "model catalog not yet implemented"
        }))
    }
}
