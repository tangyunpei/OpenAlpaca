use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::sync::RwLock;

use openalpaca_channels::ChannelRegistry;
use openalpaca_config::OpenAlpacaConfig;
use openalpaca_core::context::AppContext;
use openalpaca_sessions::SessionStore;

use crate::auth::GatewayAuth;
use crate::rpc::RpcRegistry;

/// Shared state for the gateway server.
pub struct GatewayState {
    pub app_ctx: Arc<AppContext>,
    pub config: Arc<ArcSwap<OpenAlpacaConfig>>,
    pub channel_registry: Arc<RwLock<ChannelRegistry>>,
    pub session_store: Arc<SessionStore>,
    pub auth: Arc<GatewayAuth>,
    pub rpc_registry: Arc<RpcRegistry>,
    pub broadcast_tx: tokio::sync::broadcast::Sender<BroadcastEvent>,
}

/// Events broadcast to all connected WebSocket clients.
#[derive(Debug, Clone)]
pub enum BroadcastEvent {
    ConfigChanged { hash: String },
    ChannelStatusChanged { channel_id: String },
}

#[cfg(test)]
pub mod test_helpers {
    use super::*;
    use crate::auth::AuthMode;
    use crate::rpc::build_default_registry;
    use openalpaca_core::context::AppContext;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    pub async fn make_test_state() -> Arc<GatewayState> {
        let app_ctx = Arc::new(AppContext {
            config_path: PathBuf::from("/tmp/test-config.yaml"),
            state_dir: PathBuf::from("/tmp/test-state"),
            home_dir: PathBuf::from("/tmp"),
            http_client: reqwest::Client::new(),
            shutdown: CancellationToken::new(),
        });

        let config = Arc::new(ArcSwap::new(Arc::new(OpenAlpacaConfig::default())));
        let (broadcast_tx, _) = tokio::sync::broadcast::channel(16);

        Arc::new(GatewayState {
            app_ctx,
            config,
            channel_registry: Arc::new(RwLock::new(ChannelRegistry::new())),
            session_store: Arc::new(SessionStore::new(PathBuf::from("/tmp/test-sessions.jsonl"))),
            auth: Arc::new(GatewayAuth::new(AuthMode::None)),
            rpc_registry: Arc::new(build_default_registry()),
            broadcast_tx,
        })
    }
}
