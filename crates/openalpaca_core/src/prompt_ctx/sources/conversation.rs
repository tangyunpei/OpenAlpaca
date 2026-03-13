use crate::prompt_ctx::section::ContextSection;
use crate::prompt_ctx::sources::{ContextRequest, ContextSource, ExecutionPath};
use async_trait::async_trait;

pub struct ConversationSource;

impl ConversationSource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ConversationSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ContextSource for ConversationSource {
    fn name(&self) -> &'static str {
        "conversation"
    }

    async fn resolve(&self, _request: &ContextRequest) -> Vec<ContextSection> {
        // Stub: will be wired with actual session context in a later task.
        vec![]
    }

    fn active_for(&self, path: &ExecutionPath) -> bool {
        !matches!(path, ExecutionPath::SocialQuery)
    }
}
