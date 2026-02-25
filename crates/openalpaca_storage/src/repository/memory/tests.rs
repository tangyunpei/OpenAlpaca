use super::*;
use crate::test_util::test_db;

#[test]
fn test_memory_v2_add_and_get() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    let id = repo
        .add(
            "owner-1",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "The user prefers dark mode",
            None,
            0.8,
            0.9,
        )
        .unwrap();
    assert!(id > 0);

    let mem = repo.get(id).unwrap().unwrap();
    assert_eq!(mem.owner_id, "owner-1");
    assert_eq!(mem.kind, MemoryKind::Fact);
    assert_eq!(mem.scope, MemoryScope::Global);
    assert_eq!(mem.content, "The user prefers dark mode");
    assert!((mem.importance - 0.8).abs() < f64::EPSILON);
    assert!((mem.confidence - 0.9).abs() < f64::EPSILON);
}

#[test]
fn test_memory_v2_dedup() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    let id1 = repo
        .add(
            "owner-1",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "Duplicate content",
            None,
            0.5,
            0.7,
        )
        .unwrap();
    assert!(id1 > 0);

    // Same content, same owner — should be deduped (INSERT OR IGNORE)
    let _id2 = repo
        .add(
            "owner-1",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "Duplicate content",
            None,
            0.5,
            0.7,
        )
        .unwrap();

    // The deduped insert should not change last_insert_rowid to a new value
    // but it still returns the previous id (SQLite behavior with INSERT OR IGNORE)
    let all = repo.recent("owner-1", 100).unwrap();
    assert_eq!(all.len(), 1, "Dedup should prevent second insert");
}

#[test]
fn test_memory_v2_fts_search() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    repo.add(
        "owner-1",
        MemoryKind::Fact,
        MemoryScope::Global,
        "",
        MemorySource::Conversation,
        "I love Rust programming",
        None,
        0.5,
        0.7,
    )
    .unwrap();

    repo.add(
        "owner-1",
        MemoryKind::Preference,
        MemoryScope::Global,
        "",
        MemorySource::Conversation,
        "Python is also great",
        None,
        0.5,
        0.7,
    )
    .unwrap();

    let results = repo
        .search_fts("owner-1", "Rust", 10, None, None, None)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("Rust"));
}

#[test]
fn test_memory_v2_fts_with_filters() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    repo.add(
        "owner-1",
        MemoryKind::Fact,
        MemoryScope::Global,
        "",
        MemorySource::Conversation,
        "The sky is blue",
        None,
        0.5,
        0.7,
    )
    .unwrap();

    repo.add(
        "owner-1",
        MemoryKind::Preference,
        MemoryScope::Global,
        "",
        MemorySource::Conversation,
        "The sky is beautiful today",
        None,
        0.5,
        0.7,
    )
    .unwrap();

    // Filter by kind=Fact
    let results = repo
        .search_fts("owner-1", "sky", 10, Some(MemoryKind::Fact), None, None)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].kind, MemoryKind::Fact);
}

#[test]
fn test_memory_v2_owner_isolation() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    repo.add(
        "owner-A",
        MemoryKind::Fact,
        MemoryScope::Global,
        "",
        MemorySource::Conversation,
        "Secret fact for A",
        None,
        0.5,
        0.7,
    )
    .unwrap();

    repo.add(
        "owner-B",
        MemoryKind::Fact,
        MemoryScope::Global,
        "",
        MemorySource::Conversation,
        "Secret fact for B",
        None,
        0.5,
        0.7,
    )
    .unwrap();

    let a_results = repo
        .search_fts("owner-A", "Secret", 10, None, None, None)
        .unwrap();
    assert_eq!(a_results.len(), 1);
    assert!(a_results[0].content.contains("for A"));

    let b_results = repo
        .search_fts("owner-B", "Secret", 10, None, None, None)
        .unwrap();
    assert_eq!(b_results.len(), 1);
    assert!(b_results[0].content.contains("for B"));

    let a_recent = repo.recent("owner-A", 100).unwrap();
    assert_eq!(a_recent.len(), 1);
}

#[test]
fn test_memory_v2_clear() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    repo.add(
        "owner-1",
        MemoryKind::Fact,
        MemoryScope::Global,
        "",
        MemorySource::Conversation,
        "Some fact",
        None,
        0.5,
        0.7,
    )
    .unwrap();

    repo.add(
        "owner-1",
        MemoryKind::Preference,
        MemoryScope::Global,
        "",
        MemorySource::Conversation,
        "Some preference",
        None,
        0.5,
        0.7,
    )
    .unwrap();

    let deleted = repo.clear("owner-1").unwrap();
    assert_eq!(deleted, 2);

    let remaining = repo.recent("owner-1", 100).unwrap();
    assert!(remaining.is_empty());
}

#[test]
fn test_insert_and_search_vec() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    let id = repo
        .add(
            "owner-1",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "Rust is a systems programming language",
            None,
            0.8,
            0.9,
        )
        .unwrap();

    // Create a dummy 768-dim embedding
    let mut embedding = vec![0.0f32; 768];
    embedding[0] = 1.0;
    embedding[1] = 0.5;
    repo.insert_embedding(id, &embedding).unwrap();

    // Search with a similar embedding
    let mut query_emb = vec![0.0f32; 768];
    query_emb[0] = 0.9;
    query_emb[1] = 0.4;

    let results = repo
        .search_vec("owner-1", &query_emb, 5, None, None, None)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, id);
    assert!(results[0].content.contains("Rust"));
}

#[test]
fn test_search_vec_owner_isolation() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    let id_a = repo
        .add(
            "owner-A",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "Secret A",
            None,
            0.5,
            0.7,
        )
        .unwrap();

    let id_b = repo
        .add(
            "owner-B",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "Secret B",
            None,
            0.5,
            0.7,
        )
        .unwrap();

    let emb = vec![0.1f32; 768];
    repo.insert_embedding(id_a, &emb).unwrap();
    repo.insert_embedding(id_b, &emb).unwrap();

    let query = vec![0.1f32; 768];
    let a_results = repo
        .search_vec("owner-A", &query, 10, None, None, None)
        .unwrap();
    assert_eq!(a_results.len(), 1);
    assert!(a_results[0].content.contains("Secret A"));

    let b_results = repo
        .search_vec("owner-B", &query, 10, None, None, None)
        .unwrap();
    assert_eq!(b_results.len(), 1);
    assert!(b_results[0].content.contains("Secret B"));
}

#[test]
fn test_embedding_stats() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    let id1 = repo
        .add(
            "owner-1",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "Memory one",
            None,
            0.5,
            0.7,
        )
        .unwrap();
    repo.add(
        "owner-1",
        MemoryKind::Fact,
        MemoryScope::Global,
        "",
        MemorySource::Conversation,
        "Memory two",
        None,
        0.5,
        0.7,
    )
    .unwrap();

    let (total, embedded) = repo.embedding_stats("owner-1").unwrap();
    assert_eq!(total, 2);
    assert_eq!(embedded, 0);

    let emb = vec![0.1f32; 768];
    repo.insert_embedding(id1, &emb).unwrap();

    let (total, embedded) = repo.embedding_stats("owner-1").unwrap();
    assert_eq!(total, 2);
    assert_eq!(embedded, 1);
}

#[test]
fn test_list_missing_embeddings() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    let id1 = repo
        .add(
            "owner-1",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "Embedded memory",
            None,
            0.5,
            0.7,
        )
        .unwrap();
    let _id2 = repo
        .add(
            "owner-1",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "Not embedded memory",
            None,
            0.5,
            0.7,
        )
        .unwrap();

    let emb = vec![0.1f32; 768];
    repo.insert_embedding(id1, &emb).unwrap();

    let missing = repo.list_missing_embeddings("owner-1", 10).unwrap();
    assert_eq!(missing.len(), 1);
    assert!(missing[0].1.contains("Not embedded"));
}

#[test]
fn test_search_hybrid_dedup() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    let id = repo
        .add(
            "owner-1",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "Rust programming language",
            None,
            0.8,
            0.9,
        )
        .unwrap();

    // Add embedding so it shows in both FTS and vec
    let emb = vec![0.1f32; 768];
    repo.insert_embedding(id, &emb).unwrap();

    let query_emb = vec![0.1f32; 768];
    let results = repo
        .search_hybrid("owner-1", "Rust", Some(&query_emb), 10, None, None, None)
        .unwrap();

    // Should appear only once despite matching both FTS and vec
    let ids: Vec<i64> = results.iter().map(|m| m.id).collect();
    let unique: std::collections::HashSet<i64> = ids.iter().copied().collect();
    assert_eq!(
        ids.len(),
        unique.len(),
        "Hybrid search should dedup results"
    );
    assert!(results.iter().any(|m| m.id == id));
}

#[test]
fn test_fts_trigger_sync() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    repo.add(
        "owner-1",
        MemoryKind::Fact,
        MemoryScope::Global,
        "",
        MemorySource::Conversation,
        "old keyword here",
        None,
        0.5,
        0.7,
    )
    .unwrap();

    // Update content directly via SQL
    db.with_connection(|c| {
        c.execute(
            "UPDATE memory SET content = 'new keyword here', content_hash = 'updated' WHERE owner_id = 'owner-1'",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    // Old keyword should not match
    let old_results = repo
        .search_fts("owner-1", "old", 10, None, None, None)
        .unwrap();
    assert_eq!(
        old_results.len(),
        0,
        "Old keyword should NOT be found after update"
    );

    // New keyword should match
    let new_results = repo
        .search_fts("owner-1", "new", 10, None, None, None)
        .unwrap();
    assert!(
        !new_results.is_empty(),
        "New keyword should be found after update"
    );
}

#[test]
fn test_delete_cleans_vec() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    let id = repo
        .add(
            "owner-1",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "Memory with embedding",
            None,
            0.8,
            0.9,
        )
        .unwrap();

    let emb = vec![0.1f32; 768];
    repo.insert_embedding(id, &emb).unwrap();

    // Verify embedding exists
    let vec_count: i64 = db
        .with_connection(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM memory_vec WHERE memory_id = ?1",
                [id],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(vec_count, 1, "Embedding should exist before delete");

    // Delete the memory
    let deleted = repo.delete(id).unwrap();
    assert!(deleted);

    // Verify embedding is also cleaned up
    let vec_count_after: i64 = db
        .with_connection(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM memory_vec WHERE memory_id = ?1",
                [id],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(
        vec_count_after, 0,
        "Embedding should be deleted with memory"
    );
}

#[test]
fn test_clear_cleans_vec() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    let id1 = repo
        .add(
            "owner-1",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "First memory",
            None,
            0.5,
            0.7,
        )
        .unwrap();

    let id2 = repo
        .add(
            "owner-1",
            MemoryKind::Preference,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "Second memory",
            None,
            0.5,
            0.7,
        )
        .unwrap();

    let emb = vec![0.1f32; 768];
    repo.insert_embedding(id1, &emb).unwrap();
    repo.insert_embedding(id2, &emb).unwrap();

    // Verify embeddings exist
    let vec_count: i64 = db
        .with_connection(|c| {
            c.query_row("SELECT COUNT(*) FROM memory_vec", [], |r| r.get(0))
                .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(vec_count, 2, "Both embeddings should exist before clear");

    // Clear all memories for owner
    let cleared = repo.clear("owner-1").unwrap();
    assert_eq!(cleared, 2);

    // Verify embeddings are also cleaned up
    let vec_count_after: i64 = db
        .with_connection(|c| {
            c.query_row("SELECT COUNT(*) FROM memory_vec", [], |r| r.get(0))
                .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(
        vec_count_after, 0,
        "All embeddings should be deleted with clear"
    );
}

#[test]
fn test_list_paginated() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    // Add 5 memories: 3 facts, 2 preferences
    for i in 0..3 {
        repo.add(
            "owner-1",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            &format!("Fact number {i}"),
            None,
            0.5,
            0.7,
        )
        .unwrap();
    }
    for i in 0..2 {
        repo.add(
            "owner-1",
            MemoryKind::Preference,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            &format!("Preference number {i}"),
            None,
            0.5,
            0.7,
        )
        .unwrap();
    }

    // List all: total=5, limit=3, offset=0
    let (items, total) = repo.list_paginated("owner-1", 3, 0, None).unwrap();
    assert_eq!(total, 5);
    assert_eq!(items.len(), 3);

    // Page 2
    let (items, total) = repo.list_paginated("owner-1", 3, 3, None).unwrap();
    assert_eq!(total, 5);
    assert_eq!(items.len(), 2);

    // Filter by kind=fact
    let (items, total) = repo
        .list_paginated("owner-1", 10, 0, Some(MemoryKind::Fact))
        .unwrap();
    assert_eq!(total, 3);
    assert_eq!(items.len(), 3);

    // Filter by kind=preference
    let (items, total) = repo
        .list_paginated("owner-1", 10, 0, Some(MemoryKind::Preference))
        .unwrap();
    assert_eq!(total, 2);
    assert_eq!(items.len(), 2);

    // Empty result for different owner
    let (items, total) = repo.list_paginated("owner-2", 10, 0, None).unwrap();
    assert_eq!(total, 0);
    assert_eq!(items.len(), 0);
}

#[test]
fn test_touch_accessed_boosts_importance() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    let id = repo
        .add(
            "owner-1",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "Important memory",
            None,
            0.5, // starting importance
            0.7,
        )
        .unwrap();

    // Touch with boost of 0.1
    repo.touch_accessed(&[id], 0.1).unwrap();

    // Check importance increased
    let mem = repo.get(id).unwrap().unwrap();
    assert!(
        (mem.importance - 0.6).abs() < 0.001,
        "Importance should be 0.6 after 0.1 boost, got {}",
        mem.importance
    );
    assert!(
        mem.last_accessed_at.is_some(),
        "last_accessed_at should be set"
    );

    // Touch again: should be 0.7
    repo.touch_accessed(&[id], 0.1).unwrap();
    let mem = repo.get(id).unwrap().unwrap();
    assert!(
        (mem.importance - 0.7).abs() < 0.001,
        "Importance should be 0.7 after second boost, got {}",
        mem.importance
    );

    // Test cap at 1.0: boost by 0.5 (would be 1.2, should cap at 1.0)
    repo.touch_accessed(&[id], 0.5).unwrap();
    let mem = repo.get(id).unwrap().unwrap();
    assert!(
        (mem.importance - 1.0).abs() < 0.001,
        "Importance should be capped at 1.0, got {}",
        mem.importance
    );
}

#[test]
fn test_cascade_scope_isolation() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    // Create a Global memory
    repo.add(
        "owner-1",
        MemoryKind::Fact,
        MemoryScope::Global,
        "",
        MemorySource::Conversation,
        "Global knowledge about testing frameworks",
        None,
        0.8,
        0.9,
    )
    .unwrap();

    // Create a Workspace-A memory
    repo.add(
        "owner-1",
        MemoryKind::Fact,
        MemoryScope::Workspace,
        "workspace-A",
        MemorySource::Conversation,
        "Workspace A knowledge about testing patterns",
        None,
        0.8,
        0.9,
    )
    .unwrap();

    // Create a Workspace-B memory
    repo.add(
        "owner-1",
        MemoryKind::Fact,
        MemoryScope::Workspace,
        "workspace-B",
        MemorySource::Conversation,
        "Workspace B knowledge about testing strategies",
        None,
        0.8,
        0.9,
    )
    .unwrap();

    // Cascade search with workspace-A context:
    // scopes = [(Global, None), (Workspace, Some("workspace-A"))]
    let scopes: Vec<(MemoryScope, Option<&str>)> = vec![
        (MemoryScope::Global, None),
        (MemoryScope::Workspace, Some("workspace-A")),
    ];

    let results = repo
        .search_hybrid_cascade("owner-1", "testing", None, 10, None, &scopes)
        .unwrap();

    let contents: Vec<&str> = results.iter().map(|m| m.content.as_str()).collect();

    // Workspace-A memories should be returned
    assert!(
        contents.iter().any(|c| c.contains("Workspace A")),
        "Workspace A memory should be in results, got: {contents:?}"
    );

    // Global memories should be returned
    assert!(
        contents.iter().any(|c| c.contains("Global")),
        "Global memory should be in results, got: {contents:?}"
    );

    // Workspace-B memories should NOT be returned
    assert!(
        !contents.iter().any(|c| c.contains("Workspace B")),
        "Workspace B memory should NOT be in results, got: {contents:?}"
    );
}

#[test]
fn test_get_delete_owner_scoped() {
    let db = test_db();
    let repo = MemoryRepository::new(&db);

    let id_a = repo
        .add(
            "owner-A",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "Owner A secret memory",
            None,
            0.8,
            0.9,
        )
        .unwrap();
    assert!(id_a > 0);

    let id_b = repo
        .add(
            "owner-B",
            MemoryKind::Fact,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            "Owner B secret memory",
            None,
            0.8,
            0.9,
        )
        .unwrap();
    assert!(id_b > 0);

    // Owner A can get their own memory
    let mem = repo.get_for_owner(id_a, "owner-A").unwrap();
    assert!(mem.is_some());
    assert_eq!(mem.unwrap().content, "Owner A secret memory");

    // Owner B cannot get owner A's memory
    let mem = repo.get_for_owner(id_a, "owner-B").unwrap();
    assert!(mem.is_none(), "Owner B should not be able to read owner A's memory");

    // Owner B cannot delete owner A's memory
    let deleted = repo.delete_for_owner(id_a, "owner-B").unwrap();
    assert!(!deleted, "Owner B should not be able to delete owner A's memory");

    // Verify owner A's memory still exists
    let mem = repo.get_for_owner(id_a, "owner-A").unwrap();
    assert!(mem.is_some(), "Owner A's memory should still exist after failed delete by B");

    // Owner A can delete their own memory
    let deleted = repo.delete_for_owner(id_a, "owner-A").unwrap();
    assert!(deleted, "Owner A should be able to delete their own memory");

    // Verify it's gone
    let mem = repo.get_for_owner(id_a, "owner-A").unwrap();
    assert!(mem.is_none(), "Memory should be gone after owner deletes it");

    // Owner B's memory is unaffected
    let mem = repo.get_for_owner(id_b, "owner-B").unwrap();
    assert!(mem.is_some(), "Owner B's memory should be unaffected");
}
