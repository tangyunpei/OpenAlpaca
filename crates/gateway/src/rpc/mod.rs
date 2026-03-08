pub mod channels;
pub mod config;
pub mod health;
pub mod models;
pub mod sessions;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::protocol::error::ErrorShape;
use crate::state::GatewayState;

/// Context for RPC handler execution.
pub struct RpcContext {
    pub state: Arc<GatewayState>,
}

/// Trait for RPC method handlers.
#[async_trait]
pub trait RpcHandler: Send + Sync {
    async fn handle(
        &self,
        params: serde_json::Value,
        ctx: &RpcContext,
    ) -> Result<serde_json::Value, ErrorShape>;
}

/// Registry of RPC handlers, keyed by method name.
pub struct RpcRegistry {
    handlers: HashMap<String, Box<dyn RpcHandler>>,
}

impl RpcRegistry {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    pub fn register(&mut self, method: &str, handler: Box<dyn RpcHandler>) {
        self.handlers.insert(method.to_string(), handler);
    }

    pub async fn dispatch(
        &self,
        method: &str,
        params: serde_json::Value,
        ctx: &RpcContext,
    ) -> Result<serde_json::Value, ErrorShape> {
        match self.handlers.get(method) {
            Some(handler) => handler.handle(params, ctx).await,
            None => Err(ErrorShape::method_not_found(method)),
        }
    }

    pub fn list_methods(&self) -> Vec<String> {
        let mut methods: Vec<_> = self.handlers.keys().cloned().collect();
        methods.sort();
        methods
    }
}

impl Default for RpcRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the default RPC registry with all Tier-1 methods.
pub fn build_default_registry() -> RpcRegistry {
    let mut registry = RpcRegistry::new();
    registry.register("health", Box::new(health::HealthHandler));
    registry.register("status", Box::new(health::StatusHandler));
    registry.register("config.get", Box::new(config::ConfigGetHandler));
    registry.register("config.set", Box::new(config::ConfigSetHandler));
    registry.register("sessions.list", Box::new(sessions::SessionsListHandler));
    registry.register("sessions.delete", Box::new(sessions::SessionsDeleteHandler));
    registry.register("channels.status", Box::new(channels::ChannelsStatusHandler));
    registry.register("models.list", Box::new(models::ModelsListHandler));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoHandler;

    #[async_trait]
    impl RpcHandler for EchoHandler {
        async fn handle(
            &self,
            params: serde_json::Value,
            _ctx: &RpcContext,
        ) -> Result<serde_json::Value, ErrorShape> {
            Ok(params)
        }
    }

    #[tokio::test]
    async fn test_dispatch_unknown_method() {
        let registry = RpcRegistry::new();
        let state = crate::state::test_helpers::make_test_state().await;
        let ctx = RpcContext { state };
        let result = registry
            .dispatch("nonexistent", serde_json::Value::Null, &ctx)
            .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, "METHOD_NOT_FOUND");
    }

    #[tokio::test]
    async fn test_dispatch_registered_method() {
        let mut registry = RpcRegistry::new();
        registry.register("echo", Box::new(EchoHandler));
        let state = crate::state::test_helpers::make_test_state().await;
        let ctx = RpcContext { state };
        let result = registry
            .dispatch("echo", serde_json::json!({"hello": "world"}), &ctx)
            .await;
        assert_eq!(result.unwrap(), serde_json::json!({"hello": "world"}));
    }

    #[test]
    fn test_list_methods() {
        let registry = build_default_registry();
        let methods = registry.list_methods();
        assert!(methods.contains(&"health".to_string()));
        assert!(methods.contains(&"config.get".to_string()));
        assert!(methods.contains(&"sessions.list".to_string()));
    }
}
