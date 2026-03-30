pub mod conversation;
pub mod memory;
pub mod skill;
pub mod user_profile;
pub mod workspace;

use crate::prompt_ctx::section::ContextSection;
use async_trait::async_trait;

/// Which execution path is requesting context.
#[derive(Debug, Clone)]
pub enum ExecutionPath {
    SimpleQuery,
    SocialQuery,
    SkillInvocation { skill_id: String },
    PipelineStep { step: usize, total: usize },
    DagNode { node_id: String },
    LeadAgent,
}

/// Request for context resolution.
#[derive(Debug, Clone)]
pub struct ContextRequest {
    pub query: String,
    pub intent: crate::orchestrator::intent::Intent,
    pub path: ExecutionPath,
    pub skill: Option<std::sync::Arc<crate::middleware::skill::types::SkillDocument>>,
    pub owner_id: Option<String>,
    pub scope: crate::memory::scope_context::MemoryScopeContext,
    pub model_context_window: usize,
    pub reserved_tokens: usize,
}

#[async_trait]
pub trait ContextSource: Send + Sync {
    fn name(&self) -> &'static str;
    async fn resolve(&self, request: &ContextRequest) -> Vec<ContextSection>;
    fn active_for(&self, _path: &ExecutionPath) -> bool {
        true
    }
}
