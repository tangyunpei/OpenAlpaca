use crate::prompt_ctx::section::{
    ContextKey, ContextKind, ContextSection, InjectionMode, SectionPriority, TrustLevel,
};
use crate::prompt_ctx::sources::{ContextRequest, ContextSource, ExecutionPath};
use async_trait::async_trait;
use openalpaca_storage::{Database, repository::memory::MemoryRepository};
use std::sync::Arc;

pub struct MemorySource {
    db: Arc<Database>,
}

impl MemorySource {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ContextSource for MemorySource {
    fn name(&self) -> &'static str {
        "memory"
    }

    async fn resolve(&self, request: &ContextRequest) -> Vec<ContextSection> {
        if request.query.trim().is_empty() {
            return vec![];
        }

        let owner_id = match &request.owner_id {
            Some(id) => id.clone(),
            None => return vec![],
        };

        let scopes = request.scope.cascade_scopes();
        // Convert owned values to the slice form expected by search_hybrid_cascade
        let scope_pairs: Vec<(openalpaca_storage::models::memory::MemoryScope, Option<&str>)> =
            scopes.iter().map(|(s, id)| (*s, id.as_deref())).collect();

        let repo = MemoryRepository::new(&self.db);
        let results = match repo.search_hybrid_cascade(
            &owner_id,
            &request.query,
            None, // no embedding — FTS-only search
            10,
            None,
            &scope_pairs,
        ) {
            Ok(entries) => entries,
            Err(err) => {
                tracing::warn!(error = %err, "MemorySource: search_hybrid_cascade failed");
                return vec![];
            }
        };

        results
            .into_iter()
            .map(|entry| {
                let token_estimate = entry.content.len() / 4;
                ContextSection {
                    source: "memory",
                    kind: ContextKind::Memory,
                    content: entry.content,
                    token_estimate,
                    priority: SectionPriority::Normal,
                    relevance: entry.importance as f32,
                    key: ContextKey::Memory(entry.id),
                    injection: InjectionMode::UserMessage {
                        tag: "retrieved_memory".to_string(),
                        trust: TrustLevel::Untrusted,
                    },
                }
            })
            .collect()
    }

    fn active_for(&self, path: &ExecutionPath) -> bool {
        !matches!(path, ExecutionPath::SocialQuery)
    }
}
