use arc_swap::ArcSwap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::events::EventBroadcaster;
use openalpaca_core::{
    agent::AgentConfigService,
    chat::{ChatService, ChatStreamManager},
    gateway::Gateway,
    orchestrator::Orchestrator,
    security::confirmation::ConfirmationBroker,
};
use openalpaca_storage::Database;

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub instance_id: String,
    /// When this run began — stamped at the top of `async_main`, before the
    /// listener binds, so `GET /v1/status`'s `uptime_secs` measures the
    /// daemon's own life and not the process table's idea of it.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Whether whichever launcher spawned this run pointed its stdout and
    /// stderr at `store::daemon_log_path()` and rotated it first —
    /// `openalpaca daemon start` and the GUI sidecar both do; a bare
    /// `cargo run` does not. Read once, from `store::MANAGED_LOG_ENV`, before
    /// anything else touches the environment. `GET /v1/status`'s `log_path`
    /// is `null` whenever this is `false`, even if `daemon.log` happens to
    /// exist: a daemon nobody pointed at the file must not claim one some
    /// other daemon wrote (Important #3, T44 fix round 1). The meaning is
    /// ownership, not which launcher — widened from "the CLI marked this run"
    /// when the sidecar started writing the file too (T30).
    pub managed_log: bool,
    pub token: String,
    pub event_broadcaster: EventBroadcaster,
    pub db: Database,
    pub shutdown_tx: mpsc::Sender<()>,
    pub connector_manager: crate::managers::connector::ConnectorManager,
    pub gateway: Arc<Gateway>,
    /// The same orchestrator the gateway's handler wraps, held directly for the
    /// routes that address a *run* rather than send a message: `rerun` and
    /// `start` (GAP-06) dispatch stored rows, which is orchestrator work with
    /// no turn, no lane history and no model behind it.
    pub orchestrator: Arc<Orchestrator>,
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
    /// GAP-18's read path: `GET /v1/tools` renders the live registry. Cloned
    /// **before** the registry moves into `Orchestrator::new` (`main.rs`), the
    /// way the plugin manager's clone already is.
    pub tool_registry: Arc<openalpaca_core::tools::ToolRegistry>,
    /// The ENABLE axis (extension design §6.2 #15). Non-optional: both
    /// supervisors are constructed unconditionally, so no "subsystem absent"
    /// path exists to report and `/v1/extensions` has no `503`.
    pub extensions: Arc<crate::managers::extensions::Extensions>,
    /// The one shutdown token `main.rs` cancels the moment shutdown begins —
    /// from `POST /v1/command {"command":"shutdown"}` or from SIGINT/SIGTERM,
    /// which converge on the same future — before it stops accepting
    /// connections. A handler whose connection outlives its request selects
    /// on it: the `/v1/events` socket, so its client is told the daemon is
    /// going away instead of being left on a socket that dies with the
    /// process; and the `/v1/chat/stream/{id}` SSE body, which
    /// `with_graceful_shutdown` waits on and which nothing else ends at
    /// shutdown, so without it the 10 s watchdog force-exits the daemon.
    pub cancel_token: tokio_util::sync::CancellationToken,
}
