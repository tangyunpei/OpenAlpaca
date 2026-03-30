use crate::prompt_ctx::section::ContextSection;
use crate::prompt_ctx::sources::{ContextRequest, ContextSource, ExecutionPath};
use async_trait::async_trait;

pub struct SkillContextSource;

impl SkillContextSource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SkillContextSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ContextSource for SkillContextSource {
    fn name(&self) -> &'static str {
        "skill"
    }

    async fn resolve(&self, _request: &ContextRequest) -> Vec<ContextSection> {
        // Stub: will be fully wired in Task 15.
        vec![]
    }

    fn active_for(&self, path: &ExecutionPath) -> bool {
        matches!(path, ExecutionPath::SkillInvocation { .. })
    }
}
