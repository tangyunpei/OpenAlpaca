pub mod config;
pub mod config_service;
pub mod registry;
pub mod subagent;
pub mod template;

use crate::types::{AgentInput, AgentOutput, Capability};
use async_trait::async_trait;

pub use config::AgentConfigFile;
pub use config_service::AgentConfigService;
pub use registry::{AgentRegistry, DestroyOutcome};
pub use subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Skill, SubAgent};
pub use template::{AgentTemplate, AgentTemplateFrontmatter, AgentParseError};

// The core interface for all intelligent agents.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Unique identifier for this agent instance.
    fn id(&self) -> &str;

    /// Examples of what this agent can do (for discovery/routing).
    fn capabilities(&self) -> Vec<Capability>;

    /// The main execution loop for a single request.
    /// Input: Structure containing user content + context + preferences.
    /// Output: Structure containing response payload + confidence + metadata.
    async fn process(&self, input: AgentInput) -> Result<AgentOutput, String>;
}
