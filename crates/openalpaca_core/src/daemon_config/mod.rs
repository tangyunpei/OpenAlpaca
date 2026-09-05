//! Daemon-level configuration loaded from `config/daemon.toml`.
//!
//! All fields have serde defaults matching the previously hardcoded constants,
//! so an empty file or missing sections produce identical behavior to before.

pub mod execution;
pub mod orchestrator;
pub mod security;
pub mod server;
pub mod upload;
mod validation;

pub use execution::*;
pub use orchestrator::*;
pub use security::*;
pub use server::*;
pub use upload::*;

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Experimental / opt-in features. Defaults to all-off until validated.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExperimentalConfig {
    /// When true, inject an ephemeral system notice when budget pressure
    /// reaches 80% of cost or rounds (spec P0). Default false until
    /// validated in real use.
    pub ephemeral_pressure_layer: bool,
}

/// `[extensions]` — the ENABLE axis's one knob (extension design §3.2 T3).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtensionsConfig {
    /// How long a disable waits for in-flight tool calls and out-of-process
    /// runs to finish before tearing the extension down anyway.
    ///
    /// There is no per-request `SandboxPolicy` at the supervisor level to take
    /// a `max_tool_runtime_secs` from — policies are built per call site — so
    /// this is the only input. On expiry the supervisor warns with the
    /// straggler count and proceeds.
    pub drain_timeout_secs: u64,
}

impl Default for ExtensionsConfig {
    fn default() -> Self {
        Self {
            drain_timeout_secs: 10,
        }
    }
}

/// Root config loaded from `config/daemon.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct DaemonConfig {
    pub orchestrator: OrchestratorConfig,
    pub execution: ExecutionConfig,
    pub security: SecurityConfig,
    pub server: ServerConfig,
    pub upload: UploadConfig,
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub experimental: ExperimentalConfig,
    #[serde(default)]
    pub extensions: ExtensionsConfig,
}

/// Load daemon config from a TOML file. Returns defaults if file is missing or unparseable.
pub fn load_daemon_config(path: &Path) -> DaemonConfig {
    match std::fs::read_to_string(path) {
        Ok(content) => match toml::from_str::<DaemonConfig>(&content) {
            Ok(mut config) => {
                config.validate();
                tracing::info!("Daemon config loaded from {}", path.display());
                config
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to parse daemon config {}: {e}; using defaults",
                    path.display()
                );
                DaemonConfig::default()
            }
        },
        Err(_) => {
            tracing::info!("No daemon config at {}; using defaults", path.display());
            DaemonConfig::default()
        }
    }
}

#[cfg(test)]
mod tests;
