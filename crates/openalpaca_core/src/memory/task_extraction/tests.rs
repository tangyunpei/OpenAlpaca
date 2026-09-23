use super::*;

#[test]
fn test_parse_json_response_raw() {
    let input = r#"{"extractions": [{"content": "test", "kind": "fact"}]}"#;
    let result = parse_json_response(input);
    assert!(result.is_some());
    let arr = result.unwrap()["extractions"].as_array().unwrap().len();
    assert_eq!(arr, 1);
}

#[test]
fn test_parse_json_response_fenced() {
    let input = "```json\n{\"extractions\": []}\n```";
    let result = parse_json_response(input);
    assert!(result.is_some());
}

#[test]
fn test_parse_json_response_plain_fence() {
    let input = "```\n{\"extractions\": [{\"content\": \"hello\"}]}\n```";
    let result = parse_json_response(input);
    assert!(result.is_some());
}

#[test]
fn test_parse_json_response_invalid() {
    let input = "This is not JSON at all";
    assert!(parse_json_response(input).is_none());
}

#[test]
fn test_parse_json_response_with_whitespace() {
    let input = "  \n  {\"extractions\": []}  \n  ";
    let result = parse_json_response(input);
    assert!(result.is_some());
}

mod persist {
    //! **Fix 0.3.** Persisting a memory never retires another one because
    //! the two look alike. `persist_memory_item` used to embed the new text,
    //! look for a "similar" existing fact or preference — by vector distance,
    //! or by Jaccard word overlap when no embedder was configured — and
    //! supersede it: importance dropped to 0.1, vector deleted, for any
    //! caller and any source, so a task extraction could retire an explicit
    //! save. Two distinct memories must both stand, importance untouched.

    use super::super::*;
    use crate::test_util::ConstantEmbedder;
    use openalpaca_storage::models::memory::{MemoryKind, MemoryScope, MemorySource};
    use openalpaca_storage::repository::MemoryRepository;
    use std::collections::BTreeSet;
    use std::sync::Arc;

    const OWNER: &str = "owner-1";
    const IMPORTANCE: f64 = 0.9;

    async fn persist(
        repo: &MemoryRepository<'_>,
        embedder: &Option<Arc<dyn Embedder>>,
        content: &str,
    ) -> PersistResult {
        persist_memory_item(
            repo,
            embedder,
            OWNER,
            content,
            MemoryKind::Preference,
            MemoryScope::Global,
            "",
            MemorySource::Conversation,
            IMPORTANCE,
            1.0,
            None,
        )
        .await
    }

    /// Both rows are there, each with the importance it was saved with, and
    /// neither points at the other.
    fn assert_both_stand(repo: &MemoryRepository<'_>, first: &str, second: &str) {
        let rows = repo.recent(OWNER, 10).expect("rows");
        assert_eq!(rows.len(), 2, "both memories must be rows");
        for row in &rows {
            assert_eq!(
                row.importance, IMPORTANCE,
                "importance of {:?} was changed by a later persist",
                row.content
            );
            assert_eq!(
                row.supersedes_id, None,
                "{:?} must not supersede another memory",
                row.content
            );
        }
        let contents: BTreeSet<&str> = rows.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(contents, BTreeSet::from([first, second]));
    }

    /// With an embedder, every text embeds to the same vector, so the second
    /// preference is at distance 0 from the first — the input that made the
    /// old vector step supersede.
    #[tokio::test]
    async fn two_distinct_preferences_both_stand_with_an_embedder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(&dir.path().join("m.db")).expect("db");
        let repo = MemoryRepository::new(&db);
        let embedder: Option<Arc<dyn Embedder>> = Some(Arc::new(ConstantEmbedder));

        let first = persist(&repo, &embedder, "User prefers vim").await;
        let second = persist(&repo, &embedder, "User prefers dark mode").await;

        assert_both_stand(&repo, "User prefers vim", "User prefers dark mode");
        assert!(
            matches!(first, PersistResult::Inserted(_)),
            "the first persist inserts"
        );
        assert!(
            matches!(second, PersistResult::Inserted(_)),
            "the second persist inserts; nothing is superseded"
        );

        // Both vectors are still there: the old step deleted the retired row's.
        let missing = repo
            .list_missing_embeddings(OWNER, 10)
            .expect("missing embeddings");
        assert!(
            missing.is_empty(),
            "every row keeps its vector, but these have none: {missing:?}"
        );
    }

    /// With no embedder, the old code fell back to word overlap: the second
    /// text's significant words are all in the first and the Jaccard score
    /// (6/8) clears the default 0.4 threshold, so it superseded too. (Both
    /// texts carry the word "and" because the old fallback quoted its own
    /// `AND` joiner into a literal search term.)
    #[tokio::test]
    async fn two_distinct_preferences_both_stand_without_an_embedder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(&dir.path().join("m.db")).expect("db");
        let repo = MemoryRepository::new(&db);
        let embedder: Option<Arc<dyn Embedder>> = None;

        let first = "User prefers vim keybindings and dark themes everywhere";
        let second = "User prefers keybindings and themes everywhere";
        let r1 = persist(&repo, &embedder, first).await;
        let r2 = persist(&repo, &embedder, second).await;

        assert_both_stand(&repo, first, second);
        assert!(matches!(r1, PersistResult::Inserted(_)));
        assert!(
            matches!(r2, PersistResult::Inserted(_)),
            "the second persist inserts; nothing is superseded"
        );
    }
}
