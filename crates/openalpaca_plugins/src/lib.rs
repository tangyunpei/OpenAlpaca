pub mod bridge;
pub mod error;
pub mod manager;
pub mod manifest;
pub mod permission_gate;
pub mod process_pool;
pub mod stdio_channel;

pub use bridge::PluginAgentBridge;
pub use bridge::PluginConnector;
pub use bridge::PluginLlmProvider;
pub use bridge::PluginSkillBridge;
pub use bridge::PluginToolProxy;
pub use error::PluginError;
pub use manager::{PluginInfo, PluginManager, PluginStatus};
pub use manifest::PluginManifest;
pub use permission_gate::PermissionGate;
pub use process_pool::PluginProcess;
pub use stdio_channel::StdioChannel;
