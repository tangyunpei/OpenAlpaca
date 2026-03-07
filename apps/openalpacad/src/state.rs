use arc_swap::ArcSwap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::events::EventBroadcaster;
use openalpaca_core::{
    agent::AgentConfigService,
    chat::{ChatService, ChatStreamManager},
    gateway::Gateway,
    security::confirmation::ConfirmationBroker,
};
use openalpaca_storage::Database;

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub instance_id: String,
    pub token: String,
    pub event_broadcaster: EventBroadcaster,
    pub db: Database,
    pub shutdown_tx: mpsc::Sender<()>,
    pub connector_manager: crate::managers::connector::ConnectorManager,
    pub gateway: Arc<Gateway>,
    pub llm_settings_service: Option<Arc<openalpaca_llm::LlmSettingsService>>,
    pub agent_config_service: Option<Arc<AgentConfigService>>,
    pub chat_service: Option<Arc<ChatService>>,
    pub chat_stream_manager: Option<Arc<ChatStreamManager>>,
    pub token_manager: Option<Arc<openalpaca_llm::TokenManager>>,
    pub provider_usage_tracker: Option<Arc<openalpaca_llm::ProviderUsageTracker>>,
    pub embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    pub local_user_id: String,
    pub default_lane_key: String,
    pub llm_config_path: PathBuf,
    pub daemon_config: Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    pub daemon_config_path: PathBuf,
    pub web_search_config: Arc<ArcSwap<openalpaca_llm::WebSearchConfig>>,
    pub confirmation_broker: Option<Arc<ConfirmationBroker>>,
}
