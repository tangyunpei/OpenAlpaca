use crate::prompt_ctx::section::ContextSection;
use crate::prompt_ctx::sources::{ContextRequest, ContextSource, ExecutionPath};
use async_trait::async_trait;

pub struct WorkspaceSource;

impl WorkspaceSource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WorkspaceSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ContextSource for WorkspaceSource {
    fn name(&self) -> &'static str {
        "workspace"
    }

    async fn resolve(&self, _request: &ContextRequest) -> Vec<ContextSection> {
        // Stub: will be wired in Tasks 12-14.
        vec![]
    }

    fn active_for(&self, path: &ExecutionPath) -> bool {
        matches!(path, ExecutionPath::LeadAgent)
    }
}
