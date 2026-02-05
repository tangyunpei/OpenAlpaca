pub mod config;
pub mod registry;
pub mod subagent;

use crate::types::{AgentInput, AgentOutput, Capability};
use async_trait::async_trait;

pub use config::AgentConfigFile;
pub use registry::AgentRegistry;
pub use subagent::{AgentConstraints, AgentPreset, AgentStatus, Skill, SubAgent};

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
