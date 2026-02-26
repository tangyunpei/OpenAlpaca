use super::*;
use crate::models::conversation::ConversationMessage;
use crate::repository::ConversationRepository;
use crate::test_util::test_db;

#[test]
fn test_global_user_crud() {
    let db = test_db();
    let repo = IdentityRepository::new(&db);

    // Create
    let user = repo.create_global_user("user-1", Some("Alice")).unwrap();
    assert_eq!(user.id, "user-1");
    assert_eq!(user.display_name, Some("Alice".to_string()));

    // Read
    let fetched = repo.get_global_user("user-1").unwrap().unwrap();
    assert_eq!(fetched.display_name, Some("Alice".to_string()));
}

#[test]
fn test_external_identity_link() {
    let db = test_db();
    let repo = IdentityRepository::new(&db);

    // Create global user first
    let user = repo.create_global_user("user-1", Some("Alice")).unwrap();

    // Create external identity
    let ext = repo
        .get_or_create_external_identity("telegram", "12345", Some("TG Alice"))
        .unwrap();
    assert!(ext.global_user_id.is_none());
    assert_eq!(ext.provider, "telegram");

    // Link
    repo.link_external_identity(ext.id, &user.id).unwrap();

    // Verify link
    let linked = repo
        .get_external_identity("telegram", "12345")
        .unwrap()
        .unwrap();
    assert_eq!(linked.global_user_id, Some("user-1".to_string()));
    assert!(linked.linked_at.is_some());
}

#[test]
fn test_link_token_flow() {
    let db = test_db();
    let repo = IdentityRepository::new(&db);

    // Create user
    repo.create_global_user("user-1", None).unwrap();

    // Create token
    let token = repo.create_link_token("user-1", "ABC123").unwrap();
    assert_eq!(token.token, "ABC123");

    // Consume token (should succeed)
    let result = repo.consume_link_token("ABC123").unwrap();
    assert_eq!(result, Some("user-1".to_string()));

    // Consume again (should fail - already used)
    let result2 = repo.consume_link_token("ABC123").unwrap();
    assert!(result2.is_none());
}

#[test]
fn test_migrate_lane_on_link() {
    let db = test_db();
    let identity_repo = IdentityRepository::new(&db);
    let conv_repo = ConversationRepository::new(&db);

    // Seed: create messages under old lane (provider_user_id:telegram)
    let old_lane = "tg123:telegram";
    let new_lane = "global1:telegram";

    let msg = ConversationMessage {
        id: 0,
        lane_key: old_lane.to_string(),
        role: "user".to_string(),
        content: "hello".to_string(),
        source: Some("telegram".to_string()),
        model: None,
        tokens_in: None,
        tokens_out: None,
        duration_ms: None,
        created_at: String::new(),
        content_json: None,
        display_text: None,
    };
    conv_repo.insert(&msg).unwrap();
    conv_repo
        .insert(&ConversationMessage {
            content: "world".to_string(),
            role: "assistant".to_string(),
            ..msg.clone()
        })
        .unwrap();

    // Verify messages exist under old lane
    let before = conv_repo.list_by_lane(old_lane, 50, 0).unwrap();
    assert_eq!(before.len(), 2);

    // Setup conversation_map entry
    identity_repo
        .update_conversation_map_lane_key("telegram", "999", old_lane)
        .unwrap();

    // Run migration
    identity_repo
        .migrate_lane_on_link("tg123", "global1", "telegram", "999")
        .unwrap();

    // Verify messages moved to new lane
    let after_old = conv_repo.list_by_lane(old_lane, 50, 0).unwrap();
    assert_eq!(after_old.len(), 0);

    let after_new = conv_repo.list_by_lane(new_lane, 50, 0).unwrap();
    assert_eq!(after_new.len(), 2);
    assert_eq!(after_new[0].content, "hello");

    // Verify conversation_map updated
    let cmap = identity_repo
        .get_conversation_map("telegram", "999")
        .unwrap()
        .unwrap();
    assert_eq!(cmap.lane_key, Some(new_lane.to_string()));
}

#[test]
fn test_migrate_lane_noop_when_same() {
    let db = test_db();
    let repo = IdentityRepository::new(&db);

    // Same provider_user_id and global_user_id => no-op
    repo.migrate_lane_on_link("user1", "user1", "telegram", "123")
        .unwrap();
}

#[test]
fn test_migrate_lane_relink_no_unique_violation() {
    let db = test_db();
    let identity_repo = IdentityRepository::new(&db);
    let conv_repo = ConversationRepository::new(&db);

    // First link: create messages under old lane, migrate
    let msg = ConversationMessage {
        id: 0,
        lane_key: "tg456:telegram".to_string(),
        role: "user".to_string(),
        content: "first".to_string(),
        source: Some("telegram".to_string()),
        model: None,
        tokens_in: None,
        tokens_out: None,
        duration_ms: None,
        created_at: String::new(),
        content_json: None,
        display_text: None,
    };
    conv_repo.insert(&msg).unwrap();

    identity_repo
        .update_conversation_map_lane_key("telegram", "888", "tg456:telegram")
        .unwrap();

    identity_repo
        .migrate_lane_on_link("tg456", "global2", "telegram", "888")
        .unwrap();

    // Now add more messages under the new provider lane (simulating unlink + new messages)
    conv_repo
        .insert(&ConversationMessage {
            lane_key: "tg456:telegram".to_string(),
            content: "second".to_string(),
            ..msg.clone()
        })
        .unwrap();

    // Re-link: should not cause UNIQUE violation
    identity_repo
        .migrate_lane_on_link("tg456", "global2", "telegram", "888")
        .unwrap();

    // All messages under global lane
    let msgs = conv_repo.list_by_lane("global2:telegram", 50, 0).unwrap();
    assert_eq!(msgs.len(), 2);
}
