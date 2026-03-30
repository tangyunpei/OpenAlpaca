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
