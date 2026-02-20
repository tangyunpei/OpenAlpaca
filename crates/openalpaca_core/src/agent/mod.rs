pub mod config;
pub mod config_service;
pub mod registry;
pub mod subagent;
pub mod template;

pub use config::AgentConfigFile;
pub use config_service::AgentConfigService;
pub use registry::{AgentRegistry, DestroyOutcome};
pub use subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Skill, SubAgent};
pub use template::{AgentParseError, AgentTemplate, AgentTemplateFrontmatter};
