use super::*;
use crate::test_util::test_db;

#[test]
fn test_insert_and_list() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let msg = ConversationMessage {
        id: 0,
        lane_key: "user:gui".to_string(),
        role: "user".to_string(),
        content: "Hello world".to_string(),
        source: None,
        model: None,
        tokens_in: None,
        tokens_out: None,
        duration_ms: None,
        created_at: String::new(),
    };

    let id = repo.insert(&msg).unwrap();
    assert!(id > 0);

    let messages = repo.list_by_lane("user:gui", 50, 0).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "Hello world");
}

#[test]
fn test_count_and_delete() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    for i in 0..3 {
        repo.insert(&ConversationMessage {
            id: 0,
            lane_key: "user:gui".to_string(),
            role: "user".to_string(),
            content: format!("Message {i}"),
            source: None,
            model: None,
            tokens_in: None,
            tokens_out: None,
            duration_ms: None,
            created_at: String::new(),
        })
        .unwrap();
    }

    assert_eq!(repo.count_by_lane("user:gui").unwrap(), 3);
    assert_eq!(repo.count_by_lane("other:lane").unwrap(), 0);

    let deleted = repo.delete_by_lane("user:gui").unwrap();
    assert_eq!(deleted, 3);
    assert_eq!(repo.count_by_lane("user:gui").unwrap(), 0);
}

#[test]
fn test_list_with_offset() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    for i in 0..5 {
        repo.insert(&ConversationMessage {
            id: 0,
            lane_key: "user:gui".to_string(),
            role: "user".to_string(),
            content: format!("Message {i}"),
            source: None,
            model: None,
            tokens_in: None,
            tokens_out: None,
            duration_ms: None,
            created_at: String::new(),
        })
        .unwrap();
    }

    let page = repo.list_by_lane("user:gui", 2, 2).unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].content, "Message 2");
    assert_eq!(page[1].content, "Message 3");
}

#[test]
fn test_insert_with_metadata() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let msg = ConversationMessage {
        id: 0,
        lane_key: "user:gui".to_string(),
        role: "assistant".to_string(),
        content: "Response text".to_string(),
        source: None,
        model: Some("claude-3".to_string()),
        tokens_in: Some(100),
        tokens_out: Some(200),
        duration_ms: Some(1500),
        created_at: String::new(),
    };

    repo.insert(&msg).unwrap();

    let messages = repo.list_by_lane("user:gui", 50, 0).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].model.as_deref(), Some("claude-3"));
    assert_eq!(messages[0].tokens_in, Some(100));
    assert_eq!(messages[0].tokens_out, Some(200));
    assert_eq!(messages[0].duration_ms, Some(1500));
}

#[test]
fn test_list_recent_by_lane() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    for i in 0..10 {
        repo.insert(&ConversationMessage {
            id: 0,
            lane_key: "user:gui".to_string(),
            role: "user".to_string(),
            content: format!("Message {i}"),
            source: None,
            model: None,
            tokens_in: None,
            tokens_out: None,
            duration_ms: None,
            created_at: String::new(),
        })
        .unwrap();
    }

    // Should return last 3 messages in chronological order
    let recent = repo.list_recent_by_lane("user:gui", 3).unwrap();
    assert_eq!(recent.len(), 3);
    assert_eq!(recent[0].content, "Message 7");
    assert_eq!(recent[1].content, "Message 8");
    assert_eq!(recent[2].content, "Message 9");
}

#[test]
fn test_get_or_create_conversation() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let conv = repo
        .get_or_create_conversation("user1:telegram", "telegram")
        .unwrap();
    assert_eq!(conv.lane_key, "user1:telegram");
    assert_eq!(conv.source, "telegram");
    assert_eq!(conv.message_count, 0);

    // Second call should return the same conversation
    let conv2 = repo
        .get_or_create_conversation("user1:telegram", "telegram")
        .unwrap();
    assert_eq!(conv.id, conv2.id);
}

#[test]
fn test_list_conversations() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    repo.get_or_create_conversation("user1:gui", "gui").unwrap();
    repo.get_or_create_conversation("user2:telegram", "telegram")
        .unwrap();
    repo.get_or_create_conversation("user3:telegram", "telegram")
        .unwrap();

    // List all
    let all = repo.list_conversations(None, 50, 0).unwrap();
    assert_eq!(all.len(), 3);

    // Filter by source
    let tg = repo.list_conversations(Some("telegram"), 50, 0).unwrap();
    assert_eq!(tg.len(), 2);

    let gui = repo.list_conversations(Some("gui"), 50, 0).unwrap();
    assert_eq!(gui.len(), 1);
}

#[test]
fn test_increment_message_count() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    repo.get_or_create_conversation("user1:gui", "gui").unwrap();
    repo.increment_message_count("user1:gui").unwrap();
    repo.increment_message_count("user1:gui").unwrap();

    let conv = repo.get_conversation_by_lane("user1:gui").unwrap().unwrap();
    assert_eq!(conv.message_count, 2);
    assert!(conv.last_message_at.is_some());
}

#[test]
fn test_list_recent_fewer_than_limit() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    for i in 0..2 {
        repo.insert(&ConversationMessage {
            id: 0,
            lane_key: "user:gui".to_string(),
            role: "user".to_string(),
            content: format!("Message {i}"),
            source: None,
            model: None,
            tokens_in: None,
            tokens_out: None,
            duration_ms: None,
            created_at: String::new(),
        })
        .unwrap();
    }

    // Limit is higher than total — should return all messages
    let recent = repo.list_recent_by_lane("user:gui", 50).unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].content, "Message 0");
    assert_eq!(recent[1].content, "Message 1");
}

#[test]
fn test_get_summary_default() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    repo.get_or_create_conversation("user1:gui", "gui").unwrap();
    let (summary, version, last_id) = repo.get_summary("user1:gui").unwrap();
    assert_eq!(summary, "");
    assert_eq!(version, 0);
    assert_eq!(last_id, 0);
}

#[test]
fn test_get_summary_no_row() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    // No conversation exists — should return defaults
    let (summary, version, last_id) = repo.get_summary("nonexistent:lane").unwrap();
    assert_eq!(summary, "");
    assert_eq!(version, 0);
    assert_eq!(last_id, 0);
}

#[test]
fn test_update_summary_optimistic_success() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    repo.get_or_create_conversation("user1:gui", "gui").unwrap();

    // Update with correct version (0)
    let ok = repo
        .update_summary_optimistic("user1:gui", 0, "Test summary", 42)
        .unwrap();
    assert!(ok);

    let (summary, version, last_id) = repo.get_summary("user1:gui").unwrap();
    assert_eq!(summary, "Test summary");
    assert_eq!(version, 1);
    assert_eq!(last_id, 42);
}

#[test]
fn test_update_summary_optimistic_conflict() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    repo.get_or_create_conversation("user1:gui", "gui").unwrap();

    // First update succeeds
    assert!(
        repo.update_summary_optimistic("user1:gui", 0, "Summary v1", 10)
            .unwrap()
    );

    // Second update with stale version (0) fails
    let ok = repo
        .update_summary_optimistic("user1:gui", 0, "Summary v2", 20)
        .unwrap();
    assert!(!ok);

    // Original update preserved
    let (summary, version, last_id) = repo.get_summary("user1:gui").unwrap();
    assert_eq!(summary, "Summary v1");
    assert_eq!(version, 1);
    assert_eq!(last_id, 10);
}

#[test]
fn test_clear_summary() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    repo.get_or_create_conversation("user1:gui", "gui").unwrap();
    repo.update_summary_optimistic("user1:gui", 0, "Some summary", 50)
        .unwrap();
    repo.increment_message_count("user1:gui").unwrap();

    repo.clear_summary("user1:gui").unwrap();

    let (summary, version, last_id) = repo.get_summary("user1:gui").unwrap();
    assert_eq!(summary, "");
    assert_eq!(version, 0);
    assert_eq!(last_id, 0);

    let conv = repo.get_conversation_by_lane("user1:gui").unwrap().unwrap();
    assert_eq!(conv.message_count, 0);
    assert!(conv.last_message_at.is_none());
}

#[test]
fn test_list_by_lane_id_range() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let mut ids = Vec::new();
    for i in 0..10 {
        let id = repo
            .insert(&ConversationMessage {
                id: 0,
                lane_key: "user:gui".to_string(),
                role: "user".to_string(),
                content: format!("Message {i}"),
                source: None,
                model: None,
                tokens_in: None,
                tokens_out: None,
                duration_ms: None,
                created_at: String::new(),
            })
            .unwrap();
        ids.push(id);
    }

    // Query range: after id[2] and before id[7] → should get ids 3,4,5,6
    let msgs = repo
        .list_by_lane_id_range("user:gui", ids[2], ids[7], 100)
        .unwrap();
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0].content, "Message 3");
    assert_eq!(msgs[1].content, "Message 4");
    assert_eq!(msgs[2].content, "Message 5");
    assert_eq!(msgs[3].content, "Message 6");

    // With limit
    let msgs = repo
        .list_by_lane_id_range("user:gui", ids[2], ids[7], 2)
        .unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].content, "Message 3");
    assert_eq!(msgs[1].content, "Message 4");

    // Empty range
    let msgs = repo
        .list_by_lane_id_range("user:gui", ids[5], ids[5], 100)
        .unwrap();
    assert_eq!(msgs.len(), 0);
}

#[test]
fn test_list_conversations_for_owner() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    repo.get_or_create_conversation("alice:gui", "gui").unwrap();
    repo.get_or_create_conversation("alice:telegram", "telegram").unwrap();
    repo.get_or_create_conversation("bob:gui", "gui").unwrap();
    repo.get_or_create_conversation("bob:telegram", "telegram").unwrap();

    // Alice should only see her own conversations
    let alice_all = repo.list_conversations_for_owner("alice", None, 50, 0).unwrap();
    assert_eq!(alice_all.len(), 2);
    for c in &alice_all {
        assert!(c.lane_key.starts_with("alice:"), "unexpected lane_key: {}", c.lane_key);
    }

    // Bob should only see his own conversations
    let bob_all = repo.list_conversations_for_owner("bob", None, 50, 0).unwrap();
    assert_eq!(bob_all.len(), 2);
    for c in &bob_all {
        assert!(c.lane_key.starts_with("bob:"), "unexpected lane_key: {}", c.lane_key);
    }

    // Alice filtered by source
    let alice_gui = repo.list_conversations_for_owner("alice", Some("gui"), 50, 0).unwrap();
    assert_eq!(alice_gui.len(), 1);
    assert_eq!(alice_gui[0].lane_key, "alice:gui");

    // Nonexistent owner
    let nobody = repo.list_conversations_for_owner("nobody", None, 50, 0).unwrap();
    assert_eq!(nobody.len(), 0);
}
