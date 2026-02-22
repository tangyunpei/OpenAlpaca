//! Memory scope cascade resolution.
//!
//! Provides cascading memory retrieval across scope levels:
//! Workspace → Global (most specific wins for deduplication).
//!
//! Used by the orchestrator to search all relevant scope layers
//! and merge results with deduplication.

use crate::memory::scope_context::MemoryScopeContext;
use openalpaca_storage::models::memory::{MemoryKind, MemoryV2};
use openalpaca_storage::repository::MemoryRepository;

/// Resolve memories using cascading scope search.
///
/// Searches Workspace (if available) then Global, merging and deduplicating.
/// This is a convenience wrapper around `MemoryRepository::search_hybrid_cascade`.
pub fn resolve_cascade(
    repo: &MemoryRepository<'_>,
    owner_id: &str,
    query: &str,
    embedding: Option<&[f32]>,
    limit: usize,
    kind_filter: Option<MemoryKind>,
    scope_ctx: &MemoryScopeContext,
) -> Vec<MemoryV2> {
    let cascade_scopes = scope_ctx.cascade_scopes();
    repo.search_hybrid_cascade(
        owner_id,
        query,
        embedding,
        limit,
        kind_filter,
        &cascade_scopes,
    )
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_cascade_builds_correct_scope_list() {
        let ctx = MemoryScopeContext::new(Some("/workspace".to_string()));
        let scopes = ctx.cascade_scopes();
        assert_eq!(scopes.len(), 2);
        // Global first, then Workspace
        assert_eq!(
            scopes[0].0,
            openalpaca_storage::models::memory::MemoryScope::Global
        );
        assert_eq!(
            scopes[1].0,
            openalpaca_storage::models::memory::MemoryScope::Workspace
        );
    }

    #[test]
    fn test_resolve_cascade_global_only() {
        let ctx = MemoryScopeContext::global_only();
        let scopes = ctx.cascade_scopes();
        assert_eq!(scopes.len(), 1);
        assert_eq!(
            scopes[0].0,
            openalpaca_storage::models::memory::MemoryScope::Global
        );
    }
}
