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
pub use manager::{PluginEventSink, PluginInfo, PluginManager, SecretStorage, legacy_status_word};
pub use manifest::PluginManifest;
pub use permission_gate::{PermissionGate, PermissionTable, SecretReference};
pub use process_pool::PluginProcess;
pub use stdio_channel::StdioChannel;
