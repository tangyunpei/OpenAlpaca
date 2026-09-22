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
#[tokio::test]
#[tracing_test::traced_test]
async fn unicode_memory_preview_is_safe_after_insert_and_supersession() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(&tmp.path().join("memory.db")).unwrap();
    let repo = MemoryRepository::new(&db);
    // Byte 60 falls within 中. Identical vectors deterministically select supersession.
    let original = format!("{}中 alpaca wool colour old", "a".repeat(59));
    let revised = format!("{original}!");
    let inserted = persist_memory_item_with_embedding(
        &repo,
        &None,
        "owner",
        &original,
        MemoryKind::Fact,
        MemoryScope::Global,
        "",
        MemorySource::Conversation,
        0.8,
        0.9,
        None,
        0.95,
        0.0,
        Some(vec![0.1; 768]),
    )
    .await;
    let PersistResult::Inserted(old_id) = inserted else {
        panic!("expected insert");
    };
    assert_eq!(repo.get(old_id).unwrap().unwrap().content, original);
    let superseded = persist_memory_item_with_embedding(
        &repo,
        &None,
        "owner",
        &revised,
        MemoryKind::Fact,
        MemoryScope::Global,
        "",
        MemorySource::Conversation,
        0.8,
        0.9,
        None,
        0.95,
        0.0,
        Some(vec![0.1; 768]),
    )
    .await;
    let PersistResult::Superseded {
        old_id: replaced,
        new_id,
        ..
    } = superseded
    else {
        panic!("expected supersession");
    };
    assert_eq!(replaced, old_id);
    assert_eq!(repo.get(new_id).unwrap().unwrap().content, revised);
    assert!(logs_contain("Memory persist: stored new"));
    assert!(logs_contain("Memory persist: superseded"));
}
