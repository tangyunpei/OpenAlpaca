pub mod bridge;
pub mod config_codec;
pub mod error;
pub mod install;
pub mod manager;
pub mod manifest;
pub mod permission_gate;
pub mod process_pool;
pub mod stdio_channel;

pub use bridge::PluginAgentBridge;
pub use bridge::PluginConnector;
pub use bridge::PluginSkillBridge;
pub use bridge::PluginToolProxy;
pub use error::PluginError;
pub use install::{InstallError, ManifestSummary};
pub use manager::{InstallOutcome, PluginManager, SecretStorage, UninstallOutcome};
pub use manifest::PluginManifest;
pub use permission_gate::{PermissionGate, PermissionTable, SecretReference};
pub use process_pool::PluginProcess;
pub use stdio_channel::StdioChannel;

#[cfg(test)]
mod tests {
    /// LLM-TEST-GUARD: a provider refuses Anthropic's and OpenAI's real hosts
    /// only in a build that carries `openalpaca_llm`'s `test-endpoint-guard`,
    /// which this crate turns on through `[dev-dependencies]` — unless a
    /// workspace run unifies core's `live-eval` on, which turns it off.
    #[test]
    fn the_llm_test_endpoint_guard_is_armed_in_this_test_build() {
        assert_eq!(
            openalpaca_llm::providers::test_endpoint_guard_armed(),
            !openalpaca_llm::providers::test_endpoint_guard_yields()
        );
    }
}
