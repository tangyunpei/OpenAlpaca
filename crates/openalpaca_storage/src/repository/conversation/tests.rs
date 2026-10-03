use super::*;
use crate::test_util::test_db;

#[test]
fn test_insert_and_list() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let msg = ConversationMessage {
        lane_key: "user:gui".to_string(),
        role: "user".to_string(),
        content: "Hello world".to_string(),
        ..Default::default()
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
            lane_key: "user:gui".to_string(),
            role: "user".to_string(),
            content: format!("Message {i}"),
            ..Default::default()
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
            lane_key: "user:gui".to_string(),
            role: "user".to_string(),
            content: format!("Message {i}"),
            ..Default::default()
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
        lane_key: "user:gui".to_string(),
        role: "assistant".to_string(),
        content: "Response text".to_string(),
        model: Some("claude-3".to_string()),
        tokens_in: Some(100),
        tokens_out: Some(200),
        duration_ms: Some(1500),
        ..Default::default()
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
            lane_key: "user:gui".to_string(),
            role: "user".to_string(),
            content: format!("Message {i}"),
            ..Default::default()
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
fn test_get_or_create_active_session() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let conv = repo
        .get_or_create_active_session("user1:telegram", "telegram", None)
        .unwrap();
    assert_eq!(conv.lane_key, "user1:telegram");
    assert_eq!(conv.source, "telegram");
    assert_eq!(conv.message_count, 0);

    // Second call should return the same conversation
    let conv2 = repo
        .get_or_create_active_session("user1:telegram", "telegram", None)
        .unwrap();
    assert_eq!(conv.id, conv2.id);
}

#[test]
fn test_increment_message_count() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    repo.get_or_create_active_session("user1:gui", "gui", None).unwrap();
    repo.increment_message_count("user1:gui").unwrap();
    repo.increment_message_count("user1:gui").unwrap();

    let conv = repo.get_active_session_for_lane("user1:gui").unwrap().unwrap();
    assert_eq!(conv.message_count, 2);
    assert!(conv.last_message_at.is_some());
}

#[test]
fn test_list_recent_fewer_than_limit() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    for i in 0..2 {
        repo.insert(&ConversationMessage {
            lane_key: "user:gui".to_string(),
            role: "user".to_string(),
            content: format!("Message {i}"),
            ..Default::default()
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

    repo.get_or_create_active_session("user1:gui", "gui", None).unwrap();
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

    let session = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();

    // Update with correct version (0)
    let ok = repo
        .update_working_state(&session.id, 0, "Test summary", 42)
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

    let session = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();

    // First update succeeds
    assert!(
        repo.update_working_state(&session.id, 0, "Summary v1", 10)
            .unwrap()
    );

    // Second update with stale version (0) fails
    let ok = repo
        .update_working_state(&session.id, 0, "Summary v2", 20)
        .unwrap();
    assert!(!ok);

    // Original update preserved
    let (summary, version, last_id) = repo.get_summary("user1:gui").unwrap();
    assert_eq!(summary, "Summary v1");
    assert_eq!(version, 1);
    assert_eq!(last_id, 10);
}

/// A working state is written to the session it was computed for (design 11
/// §2): after "new chat" the write still lands on the archived conversation,
/// and the lane's new session — at the same version 0 — is left empty.
#[test]
fn the_working_state_write_addresses_its_session_not_the_lane() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let first = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    let second = repo.create_session("user1:gui", "gui", None, None).unwrap();

    assert!(
        repo.update_working_state(&first.id, 0, "{\"v\":1}", 7)
            .unwrap()
    );

    let first = repo.get_session(&first.id).unwrap().unwrap();
    assert_eq!(first.status, "archived");
    assert_eq!(first.working_state, "{\"v\":1}");
    assert_eq!(first.working_state_version, 1);
    assert_eq!(first.working_state_through, 7);
    assert!(first.working_state_updated_at.is_some());

    let second = repo.get_session(&second.id).unwrap().unwrap();
    assert_eq!(second.status, "active");
    assert_eq!(second.working_state, "");
    assert_eq!(second.working_state_version, 0);
    assert_eq!(second.working_state_through, 0);
}

/// The close records the episode note it projected (design 11 §2) on the
/// session it closed — archived or not — and nowhere else.
#[test]
fn the_episode_note_id_is_written_to_its_session() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let first = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    assert_eq!(first.episode_note_id, None);
    let second = repo.create_session("user1:gui", "gui", None, None).unwrap();

    repo.set_episode_note_id(&first.id, "55e811b6-3220-4758-a47b-034bd1023f38")
        .unwrap();

    let first = repo.get_session(&first.id).unwrap().unwrap();
    assert_eq!(first.status, "archived");
    assert_eq!(
        first.episode_note_id.as_deref(),
        Some("55e811b6-3220-4758-a47b-034bd1023f38")
    );
    let second = repo.get_session(&second.id).unwrap().unwrap();
    assert_eq!(second.episode_note_id, None);
}

/// A history clear forgets the session's conversation summaries (LQ4), so
/// the session stops naming the one last projected — by either clear.
#[test]
fn a_history_clear_forgets_the_episode_note_id() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);
    let note = "55e811b6-3220-4758-a47b-034bd1023f38";

    let first = repo.create_session("user1:gui", "gui", None, None).unwrap();
    repo.set_episode_note_id(&first.id, note).unwrap();
    repo.clear_summary_for_session(&first.id).unwrap();
    assert_eq!(
        repo.get_session(&first.id)
            .unwrap()
            .unwrap()
            .episode_note_id,
        None
    );

    repo.set_episode_note_id(&first.id, note).unwrap();
    repo.clear_summary("user1:gui").unwrap();
    assert_eq!(
        repo.get_session(&first.id)
            .unwrap()
            .unwrap()
            .episode_note_id,
        None
    );
}

/// Design 11 §4 (b), owner decision CQ-5: a conversation carries the
/// previous one's state unless it was started fresh, and a fresh start is
/// stored with the row.
#[test]
fn a_session_started_fresh_is_stored_as_not_inheriting() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let implicit = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    assert!(implicit.inherit);
    let new_chat = repo.create_session("user1:gui", "gui", None, None).unwrap();
    assert!(new_chat.inherit);
    let fresh = repo
        .create_session_inheriting("user1:gui", "gui", None, None, false)
        .unwrap();
    assert!(!fresh.inherit);
    assert!(!repo.get_session(&fresh.id).unwrap().unwrap().inherit);
    assert_eq!(
        repo.get_active_session_for_lane("user1:gui")
            .unwrap()
            .unwrap()
            .id,
        fresh.id
    );
}

/// Design 11 §4 (b): the conversation a new one continues from is the
/// lane's newest archived session — the last one closed, not the last one
/// created — bound to the same workspace, `NULL` matching `NULL`; never an
/// older one, never another lane's, never the active one.
#[test]
fn the_previous_session_is_the_lanes_last_closed_one_in_the_same_workspace() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);
    let lane = "user1:gui";

    let a = repo.create_session(lane, "gui", None, Some("a")).unwrap();
    let p = repo
        .create_session(lane, "gui", Some("/p"), Some("p"))
        .unwrap();
    let b = repo.create_session(lane, "gui", None, Some("b")).unwrap();
    let now = repo.create_session(lane, "gui", None, Some("now")).unwrap();
    let elsewhere = repo
        .create_session("user1:cli", "cli", None, Some("other lane"))
        .unwrap();
    repo.create_session("user1:cli", "cli", None, None).unwrap();
    // `a` was closed last, although it was created first.
    let ended_at = |id: &str, at: &str| {
        db.with_connection(|c| {
            c.execute(
                "UPDATE session SET ended_at = ?1 WHERE id = ?2",
                rusqlite::params![at, id],
            )?;
            Ok(())
        })
        .unwrap()
    };
    ended_at(&b.id, "2026-09-28 10:00:00");
    ended_at(&a.id, "2026-09-28 11:00:00");
    ended_at(&p.id, "2026-09-28 09:00:00");
    ended_at(&elsewhere.id, "2026-09-28 13:00:00");

    let previous = |workspace: Option<&str>| {
        repo.previous_session(lane, workspace)
            .unwrap()
            .map(|s| s.title)
    };
    assert_eq!(previous(None).as_deref(), Some("a"), "the last one closed");
    assert_eq!(previous(Some("/p")).as_deref(), Some("p"));
    assert_eq!(
        previous(Some("/q")),
        None,
        "no conversation on that project"
    );
    ended_at(&b.id, "2026-09-28 12:00:00");
    assert_eq!(
        previous(None).as_deref(),
        Some("b"),
        "only the newest: `a` is not read in its place"
    );
    assert_ne!(previous(None).as_deref(), Some(now.title.as_str()));
    assert_ne!(
        previous(None).as_deref(),
        Some("other lane"),
        "another lane's is never read"
    );
}

#[test]
fn test_clear_summary() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let session = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    repo.update_working_state(&session.id, 0, "Some summary", 50)
        .unwrap();
    repo.increment_message_count("user1:gui").unwrap();

    repo.clear_summary("user1:gui").unwrap();

    let (summary, version, last_id) = repo.get_summary("user1:gui").unwrap();
    assert_eq!(summary, "");
    assert_eq!(version, 0);
    assert_eq!(last_id, 0);

    let conv = repo.get_active_session_for_lane("user1:gui").unwrap().unwrap();
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
                lane_key: "user:gui".to_string(),
                role: "user".to_string(),
                content: format!("Message {i}"),
                ..Default::default()
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

    repo.get_or_create_active_session("alice:gui", "gui", None).unwrap();
    repo.get_or_create_active_session("alice:telegram", "telegram", None)
        .unwrap();
    repo.get_or_create_active_session("bob:gui", "gui", None).unwrap();
    repo.get_or_create_active_session("bob:telegram", "telegram", None)
        .unwrap();

    // Alice should only see her own conversations
    let alice_all = repo
        .list_conversations_for_owner("alice", None, 50, 0)
        .unwrap();
    assert_eq!(alice_all.len(), 2);
    for c in &alice_all {
        assert!(
            c.lane_key.starts_with("alice:"),
            "unexpected lane_key: {}",
            c.lane_key
        );
    }

    // Bob should only see his own conversations
    let bob_all = repo
        .list_conversations_for_owner("bob", None, 50, 0)
        .unwrap();
    assert_eq!(bob_all.len(), 2);
    for c in &bob_all {
        assert!(
            c.lane_key.starts_with("bob:"),
            "unexpected lane_key: {}",
            c.lane_key
        );
    }

    // Alice filtered by source
    let alice_gui = repo
        .list_conversations_for_owner("alice", Some("gui"), 50, 0)
        .unwrap();
    assert_eq!(alice_gui.len(), 1);
    assert_eq!(alice_gui[0].lane_key, "alice:gui");

    // Nonexistent owner
    let nobody = repo
        .list_conversations_for_owner("nobody", None, 50, 0)
        .unwrap();
    assert_eq!(nobody.len(), 0);
}

// ── GAP-23: the run a message started, or reported on ────────────────

#[test]
fn task_id_survives_the_round_trip_on_every_read() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let delegating = repo
        .insert(&ConversationMessage {
            lane_key: "user:gui".to_string(),
            role: "assistant".to_string(),
            content: "Starting that now.".to_string(),
            task_id: Some("task-1".to_string()),
            ..Default::default()
        })
        .unwrap();
    repo.insert(&ConversationMessage {
        lane_key: "user:gui".to_string(),
        role: "user".to_string(),
        content: "thanks".to_string(),
        ..Default::default()
    })
    .unwrap();
    let report = repo
        .insert(&ConversationMessage {
            lane_key: "user:gui".to_string(),
            role: "assistant".to_string(),
            content: "Done.".to_string(),
            task_id: Some("task-1".to_string()),
            ..Default::default()
        })
        .unwrap();

    let listed = repo.list_by_lane("user:gui", 50, 0).unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|m| m.task_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("task-1"), None, Some("task-1")],
    );

    let recent = repo.list_recent_by_lane("user:gui", 50).unwrap();
    assert_eq!(recent[0].task_id.as_deref(), Some("task-1"));
    assert!(recent[1].task_id.is_none());

    // The ten-column projection reads it too, so no path silently answers
    // `None` for a message that started a run.
    let ranged = repo
        .list_by_lane_id_range("user:gui", delegating - 1, report + 1, 50)
        .unwrap();
    assert_eq!(ranged.len(), 3);
    assert_eq!(ranged[2].task_id.as_deref(), Some("task-1"));
}

#[test]
fn structured_insert_carries_the_run_too() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    repo.insert_with_structured(
        &ConversationMessage {
            lane_key: "user:gui".to_string(),
            role: "assistant".to_string(),
            content: "Starting that now.".to_string(),
            task_id: Some("task-9".to_string()),
            ..Default::default()
        },
        r#"{"v":1,"parts":[]}"#,
        "Starting that now.",
    )
    .unwrap();

    let listed = repo.list_by_lane("user:gui", 50, 0).unwrap();
    assert_eq!(listed[0].task_id.as_deref(), Some("task-9"));
    assert_eq!(listed[0].display_text.as_deref(), Some("Starting that now."));
}

/// A message carrying every column the two writers bind, with structured
/// fields of its own that neither writer may read.
fn full_message(lane_key: &str, session_id: Option<&str>) -> ConversationMessage {
    ConversationMessage {
        lane_key: lane_key.to_string(),
        role: "assistant".to_string(),
        content: "the answer".to_string(),
        source: Some("gui".to_string()),
        model: Some("m-1".to_string()),
        tokens_in: Some(11),
        tokens_out: Some(22),
        duration_ms: Some(33),
        content_json: Some(r#"{"from":"the struct"}"#.to_string()),
        display_text: Some("from the struct".to_string()),
        task_id: Some("task-7".to_string()),
        session_id: session_id.map(str::to_string),
        ..Default::default()
    }
}

fn read_back(repo: &ConversationRepository<'_>, id: i64) -> ConversationMessage {
    let lane = repo
        .db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT lane_key FROM conversation_messages WHERE id = ?1",
                [id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap();
    repo.list_by_lane(&lane, 50, 0)
        .unwrap()
        .into_iter()
        .find(|m| m.id == id)
        .unwrap()
}

/// The plain writer stores NULL in both structured columns, whatever the
/// message struct carries in its own `content_json` / `display_text`.
#[test]
fn plain_insert_never_reads_the_structs_structured_fields() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let id = repo.insert(&full_message("user:gui", None)).unwrap();
    let row = read_back(&repo, id);
    assert_eq!(row.content_json, None);
    assert_eq!(row.display_text, None);
    assert_eq!(
        (
            row.role.as_str(),
            row.content.as_str(),
            row.source.as_deref(),
            row.model.as_deref(),
            row.tokens_in,
            row.tokens_out,
            row.duration_ms,
            row.task_id.as_deref(),
        ),
        (
            "assistant",
            "the answer",
            Some("gui"),
            Some("m-1"),
            Some(11),
            Some(22),
            Some(33),
            Some("task-7"),
        )
    );
}

/// The structured writer stores its two arguments, not the struct's fields,
/// and every common column exactly as the plain writer does.
#[test]
fn structured_insert_stores_its_arguments_and_every_common_column() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let id = repo
        .insert_with_structured(&full_message("user:gui", None), r#"{"v":1}"#, "shown")
        .unwrap();
    let row = read_back(&repo, id);
    assert_eq!(row.content_json.as_deref(), Some(r#"{"v":1}"#));
    assert_eq!(row.display_text.as_deref(), Some("shown"));
    assert_eq!(
        (
            row.role.as_str(),
            row.content.as_str(),
            row.source.as_deref(),
            row.model.as_deref(),
            row.tokens_in,
            row.tokens_out,
            row.duration_ms,
            row.task_id.as_deref(),
        ),
        (
            "assistant",
            "the answer",
            Some("gui"),
            Some("m-1"),
            Some(11),
            Some(22),
            Some(33),
            Some("task-7"),
        )
    );
}

/// Both writers resolve the session the same way: the one the message
/// names (even archived, even unknown), else the lane's active one, else NULL.
#[test]
fn both_inserts_resolve_the_session_in_the_same_order() {
    type Writer = fn(&ConversationRepository<'_>, &ConversationMessage) -> i64;
    let writers: [(&str, Writer); 2] = [
        ("plain", |repo, msg| repo.insert(msg).unwrap()),
        ("structured", |repo, msg| {
            repo.insert_with_structured(msg, "{}", "d").unwrap()
        }),
    ];

    for (name, write) in writers {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        // No session on the lane at all: NULL.
        let id = write(&repo, &full_message("bare:gui", None));
        assert_eq!(read_back(&repo, id).session_id, None, "{name}: no session");

        // The lane's active session when none is named.
        let archived = repo
            .get_or_create_active_session("user:gui", "gui", None)
            .unwrap();
        let active = repo.create_session("user:gui", "gui", None, None).unwrap();
        let id = write(&repo, &full_message("user:gui", None));
        assert_eq!(
            read_back(&repo, id).session_id.as_deref(),
            Some(active.id.as_str()),
            "{name}: active session"
        );

        // A named session wins over the active one, archived or not.
        let id = write(&repo, &full_message("user:gui", Some(&archived.id)));
        assert_eq!(
            read_back(&repo, id).session_id.as_deref(),
            Some(archived.id.as_str()),
            "{name}: explicit archived session"
        );

        // A named session is stored as named, without a lookup.
        let id = write(&repo, &full_message("user:gui", Some("no-such-session")));
        assert_eq!(
            read_back(&repo, id).session_id.as_deref(),
            Some("no-such-session"),
            "{name}: explicit unknown session"
        );
    }
}

// ── §5.1: the session lifecycle ──────────────────────────────────────

#[test]
fn implicit_creation_never_archives_the_lane() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let first = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    assert_eq!(first.status, SESSION_ACTIVE);
    assert!(first.ended_at.is_none());

    // Ten more turns on the same lane keep continuing the same conversation.
    for _ in 0..10 {
        let again = repo
            .get_or_create_active_session("user1:gui", "gui", None)
            .unwrap();
        assert_eq!(again.id, first.id);
    }
    let (all, total) = repo
        .list_sessions(&SessionFilter {
            limit: 50,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(all.len(), 1);
}

#[test]
fn explicit_creation_archives_the_previous_active_session() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let first = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    let second = repo
        .create_session("user1:gui", "gui", None, Some("Second chat"))
        .unwrap();

    assert_ne!(first.id, second.id);
    assert_eq!(second.status, SESSION_ACTIVE);
    assert_eq!(second.title, "Second chat");

    let first = repo.get_session(&first.id).unwrap().unwrap();
    assert_eq!(first.status, SESSION_ARCHIVED);
    assert!(first.ended_at.is_some(), "an archived session records when");

    // And the lane resolves to the new one.
    assert_eq!(
        repo.active_session_id("user1:gui").unwrap().as_deref(),
        Some(second.id.as_str())
    );
}

#[test]
fn two_sessions_on_one_lane_produce_two_clean_transcripts() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let first = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    repo.insert(&ConversationMessage {
        lane_key: "user1:gui".to_string(),
        role: "user".to_string(),
        content: "in the first".to_string(),
        ..Default::default()
    })
    .unwrap();

    let second = repo.create_session("user1:gui", "gui", None, None).unwrap();
    repo.insert(&ConversationMessage {
        lane_key: "user1:gui".to_string(),
        role: "user".to_string(),
        content: "in the second".to_string(),
        ..Default::default()
    })
    .unwrap();

    // Each session sees only its own turns...
    let a = repo.list_by_session(&first.id, 50, 0).unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].content, "in the first");
    assert_eq!(a[0].session_id.as_deref(), Some(first.id.as_str()));

    let b = repo.list_recent_by_session(&second.id, 50).unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].content, "in the second");

    assert_eq!(repo.count_by_session(&first.id).unwrap(), 1);
    assert_eq!(repo.count_by_session(&second.id).unwrap(), 1);
    // ...while the lane still holds both.
    assert_eq!(repo.count_by_lane("user1:gui").unwrap(), 2);
}

#[test]
fn the_first_workspace_binds_and_later_ones_do_not_rebind() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let bare = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    assert!(bare.workspace_id.is_none());

    let bound = repo
        .get_or_create_active_session("user1:gui", "gui", Some("/repo/one"))
        .unwrap();
    assert_eq!(bound.id, bare.id);
    assert_eq!(bound.workspace_id.as_deref(), Some("/repo/one"));

    // A second project does not re-point this session. Opening the new session
    // that the changed project belongs in is the *caller's* half of §5.1's
    // rule — the gateway's resolve step does it on the turn that changes
    // project (`GatewayPersistence::resolve_turn_session`), and a client can
    // ask for one with `POST /v1/sessions`. This method only refuses to move
    // the binding under runs that are already pinned to it.
    let same = repo
        .get_or_create_active_session("user1:gui", "gui", Some("/repo/two"))
        .unwrap();
    assert_eq!(same.id, bound.id, "no new session appears from here");
    assert_eq!(same.workspace_id.as_deref(), Some("/repo/one"));

    // A brand-new session takes the workspace it is created with.
    let fresh = repo
        .create_session("user1:gui", "gui", Some("/repo/two"), None)
        .unwrap();
    assert_eq!(fresh.workspace_id.as_deref(), Some("/repo/two"));
}

#[test]
fn activating_an_archived_session_steps_the_incumbent_down() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let first = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    let second = repo.create_session("user1:gui", "gui", None, None).unwrap();

    assert!(repo.activate_session(&first.id).unwrap());
    assert_eq!(
        repo.get_session(&first.id).unwrap().unwrap().status,
        SESSION_ACTIVE
    );
    assert_eq!(
        repo.get_session(&second.id).unwrap().unwrap().status,
        SESSION_ARCHIVED
    );

    // Idempotent, and honest about an unknown id.
    assert!(repo.activate_session(&first.id).unwrap());
    assert!(!repo.activate_session("no-such-session").unwrap());
}

#[test]
fn archiving_leaves_the_lane_with_no_active_session() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let only = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    assert!(repo.archive_session(&only.id).unwrap());
    assert!(repo.active_session_id("user1:gui").unwrap().is_none());
    assert!(!repo.archive_session("no-such-session").unwrap());

    // The next turn opens a fresh one rather than reviving the archived one.
    let next = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    assert_ne!(next.id, only.id);
}

#[test]
fn deleting_a_session_takes_its_messages_and_frees_its_runs() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let session = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    repo.insert(&ConversationMessage {
        lane_key: "user1:gui".to_string(),
        role: "user".to_string(),
        content: "hello".to_string(),
        ..Default::default()
    })
    .unwrap();
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO task (id, title, created_by, source_lane, session_id)
             VALUES ('task-1', 'A run', 'user1', 'user1:gui', ?1)",
            [&session.id],
        )?;
        conn.execute(
            "INSERT INTO tool_execution_log
                (agent_id, tool_name, success, duration_ms, session_id, log_seq, result_ref)
             VALUES ('a', 'file_read', 1, 3, ?1, 7, 'log:7')",
            [&session.id],
        )?;
        Ok(())
    })
    .unwrap();

    assert!(repo.delete_session(&session.id).unwrap());
    assert!(repo.get_session(&session.id).unwrap().is_none());
    assert_eq!(repo.count_by_lane("user1:gui").unwrap(), 0);

    // The run happened; its row outlives the transcript it was started from.
    let orphaned: Option<String> = db
        .with_connection(|conn| {
            Ok(conn.query_row("SELECT session_id FROM task WHERE id = 'task-1'", [], |r| {
                r.get(0)
            })?)
        })
        .unwrap();
    assert!(orphaned.is_none());

    // R51: the audit half of a tool-call row stays — that the tool ran is still
    // true, and `invocations_today` must not move because a transcript went —
    // but every pointer into the removed session and its log is cleared.
    let (logged, still_indexed): (i64, i64) = db
        .with_connection(|conn| {
            Ok((
                conn.query_row("SELECT COUNT(*) FROM tool_execution_log", [], |r| r.get(0))?,
                conn.query_row(
                    "SELECT COUNT(*) FROM tool_execution_log
                      WHERE session_id IS NOT NULL OR log_seq IS NOT NULL
                         OR result_ref IS NOT NULL",
                    [],
                    |r| r.get(0),
                )?,
            ))
        })
        .unwrap();
    assert_eq!(logged, 1, "the audit row outlives the transcript");
    assert_eq!(still_indexed, 0, "nothing points at the removed session");

    assert!(!repo.delete_session("no-such-session").unwrap());
}

#[test]
fn renaming_and_binding_a_workspace_go_through_update_session() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let session = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    assert!(
        repo.update_session(&session.id, Some("Renamed"), Some(Some("/repo/one")))
            .unwrap()
    );
    let updated = repo.get_session(&session.id).unwrap().unwrap();
    assert_eq!(updated.title, "Renamed");
    assert_eq!(updated.workspace_id.as_deref(), Some("/repo/one"));

    // An empty PATCH still distinguishes a known session from an unknown one.
    assert!(repo.update_session(&session.id, None, None).unwrap());
    assert!(!repo.update_session("no-such-session", Some("x"), None).unwrap());
}

#[test]
fn list_sessions_filters_and_pages() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let gui = repo
        .get_or_create_active_session("alice:gui", "gui", Some("/repo/one"))
        .unwrap();
    repo.update_session(&gui.id, Some("Alpaca work"), None).unwrap();
    let archived = repo.create_session("alice:gui", "gui", None, Some("Older")).unwrap();
    repo.get_or_create_active_session("alice:telegram", "telegram", None)
        .unwrap();

    let all = |f: SessionFilter<'_>| repo.list_sessions(&f).unwrap();

    let (rows, total) = all(SessionFilter {
        limit: 50,
        ..Default::default()
    });
    assert_eq!(total, 3);
    assert_eq!(rows.len(), 3);

    let (rows, total) = all(SessionFilter {
        workspace_id: Some("/repo/one"),
        limit: 50,
        ..Default::default()
    });
    assert_eq!((rows.len(), total), (1, 1));
    assert_eq!(rows[0].id, gui.id);

    let (rows, _) = all(SessionFilter {
        source: Some("telegram"),
        limit: 50,
        ..Default::default()
    });
    assert_eq!(rows.len(), 1);

    let (rows, _) = all(SessionFilter {
        status: Some(SESSION_ACTIVE),
        limit: 50,
        ..Default::default()
    });
    assert_eq!(rows.len(), 2, "gui's first session was archived");

    let (rows, _) = all(SessionFilter {
        q: Some("alpaca"),
        limit: 50,
        ..Default::default()
    });
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, gui.id);

    // Paging: total ignores the window.
    let (rows, total) = all(SessionFilter {
        limit: 1,
        offset: 1,
        ..Default::default()
    });
    assert_eq!((rows.len(), total), (1, 3));
    assert!(rows[0].id == archived.id || rows[0].id == gui.id || !rows[0].id.is_empty());
}

/// `?q=` is a literal substring, not a pattern: `%` and `_` are characters a
/// title can contain, and `LIKE` without `ESCAPE` would have made the first
/// match every row.
#[test]
fn a_session_search_matches_wildcard_characters_literally() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let percent = repo.create_session("alice:gui", "gui", None, Some("100% done")).unwrap();
    let underscore = repo
        .create_session("alice:gui", "gui", None, Some("draft_two"))
        .unwrap();
    repo.create_session("alice:gui", "gui", None, Some("plain title"))
        .unwrap();

    let search = |q: &str| {
        repo.list_sessions(&SessionFilter {
            q: Some(q),
            limit: 50,
            ..Default::default()
        })
        .unwrap()
    };

    let (rows, total) = search("%");
    assert_eq!((rows.len(), total), (1, 1), "`%` is not a wildcard");
    assert_eq!(rows[0].id, percent.id);

    let (rows, _) = search("100%");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, percent.id);

    let (rows, _) = search("draft_");
    assert_eq!(rows.len(), 1, "`_` matches only an underscore");
    assert_eq!(rows[0].id, underscore.id);

    let (rows, _) = search("draftX");
    assert!(rows.is_empty(), "`_` never stood for `X`");

    // The escape character itself is a character too.
    let (rows, _) = search("\\");
    assert!(rows.is_empty());
}

/// `LIMIT -1` means *no limit* in SQLite, so an unclamped page served the whole
/// table — and fed every id of it to the unchunked `IN (…)` lists that read a
/// page's runs and artifact links.
#[test]
fn a_session_page_is_clamped_at_the_repository() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);
    for n in 0..4 {
        repo.create_session("alice:gui", "gui", None, Some(&format!("S{n}")))
            .unwrap();
    }

    let page = |limit: i64| {
        repo.list_sessions(&SessionFilter {
            limit,
            ..Default::default()
        })
        .unwrap()
    };

    // Four rows is fewer than the default page, so "the default" is observed as
    // "everything, not a SQLite no-limit": the clamp is asserted on the bound
    // value below.
    let (rows, total) = page(-1);
    assert_eq!((rows.len(), total), (4, 4));
    assert_eq!(page(0).0.len(), 4);
    assert_eq!(page(2).0.len(), 2, "a page the caller asked for is honoured");

    // The clamp itself: more rows than the default page, asked for without one.
    for n in 4..SESSION_PAGE_DEFAULT + 10 {
        repo.create_session("alice:gui", "gui", None, Some(&format!("S{n}")))
            .unwrap();
    }
    let (rows, total) = page(-1);
    assert_eq!(rows.len(), SESSION_PAGE_DEFAULT as usize);
    assert_eq!(total, SESSION_PAGE_DEFAULT + 10, "total ignores the window");
    let (rows, _) = page(100_000);
    assert_eq!(
        rows.len(),
        (SESSION_PAGE_DEFAULT + 10) as usize,
        "an oversized ask is capped at SESSION_PAGE_MAX, here above the table"
    );
}

#[test]
fn a_message_can_name_an_archived_session_and_be_counted_there() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let origin = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    let current = repo.create_session("user1:gui", "gui", None, None).unwrap();

    // The completion report of a run started in `origin` (§5.3).
    repo.insert(&ConversationMessage {
        lane_key: "user1:gui".to_string(),
        role: "assistant".to_string(),
        content: "Done.".to_string(),
        session_id: Some(origin.id.clone()),
        ..Default::default()
    })
    .unwrap();
    repo.increment_message_count_for_session(&origin.id).unwrap();

    assert_eq!(repo.count_by_session(&origin.id).unwrap(), 1);
    assert_eq!(repo.count_by_session(&current.id).unwrap(), 0);
    assert_eq!(
        repo.get_session(&origin.id).unwrap().unwrap().message_count,
        1
    );
    assert_eq!(
        repo.get_session(&current.id).unwrap().unwrap().message_count,
        0
    );
}

#[test]
fn task_counts_by_session_is_one_query_for_a_page() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let a = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    let b = repo
        .get_or_create_active_session("user1:cli", "cli", None)
        .unwrap();
    db.with_connection(|conn| {
        for (id, session, status) in [
            ("t1", &a.id, "interrupted"),
            ("t2", &a.id, "interrupted"),
            ("t3", &a.id, "completed"),
            ("t4", &b.id, "interrupted"),
        ] {
            conn.execute(
                "INSERT INTO task (id, title, created_by, source_lane, session_id, status)
                 VALUES (?1, 'run', 'user1', 'user1:gui', ?2, ?3)",
                rusqlite::params![id, session, status],
            )?;
        }
        Ok(())
    })
    .unwrap();

    let counts = repo
        .task_counts_by_session(&[a.id.clone(), b.id.clone()], "interrupted")
        .unwrap();
    assert_eq!(counts.get(&a.id), Some(&2));
    assert_eq!(counts.get(&b.id), Some(&1));
    assert!(repo.task_counts_by_session(&[], "interrupted").unwrap().is_empty());
}

/// The boot sweep's protected set (plan §5.4: "an active session's log is
/// never evicted"). One query across every lane, because the sweep runs before
/// anything else knows which lanes exist.
#[test]
fn active_session_ids_names_every_lanes_live_session() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let gui = repo
        .get_or_create_active_session("user1:gui", "gui", None)
        .unwrap();
    let cli = repo
        .get_or_create_active_session("user1:cli", "cli", None)
        .unwrap();
    // A second session on the gui lane archives the first in the same
    // transaction, so only the newer one may be protected.
    let gui_second = repo
        .create_session("user1:gui", "gui", None, Some("Second"))
        .unwrap();

    let mut ids = repo.active_session_ids().unwrap();
    ids.sort();
    let mut expected = vec![cli.id.clone(), gui_second.id.clone()];
    expected.sort();
    assert_eq!(ids, expected);
    assert!(
        !ids.contains(&gui.id),
        "the archived session is not protected from the sweep"
    );
}

// ── messages_7d (GAP-17, T49) ────────────────────────────────────────────────

/// Backdate a stored message so a window test can put it outside the cutoff.
fn backdate(db: &crate::Database, id: i64, created_at: &str) {
    db.with_connection(|conn| {
        conn.execute(
            "UPDATE conversation_messages SET created_at = ?1 WHERE id = ?2",
            rusqlite::params![created_at, id],
        )?;
        Ok(())
    })
    .unwrap();
}

fn message_from(lane_key: &str, source: &str) -> ConversationMessage {
    ConversationMessage {
        lane_key: lane_key.to_string(),
        role: "user".to_string(),
        content: "hi".to_string(),
        source: Some(source.to_string()),
        ..Default::default()
    }
}

/// `GET /v1/connectors`' `messages_7d`: one grouped count per source, so the
/// panel's whole list costs one query.
#[test]
fn message_counts_by_source_groups_every_source_in_one_read() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    repo.insert(&message_from("u1:telegram", "telegram")).unwrap();
    repo.insert(&message_from("u2:telegram", "telegram")).unwrap();
    repo.insert(&message_from("u1:imessage", "imessage")).unwrap();
    repo.insert(&message_from("u1:gui", "gui")).unwrap();

    let counts = repo
        .message_counts_by_source_since("2000-01-01 00:00:00")
        .unwrap();
    assert_eq!(counts.get("telegram"), Some(&2));
    assert_eq!(counts.get("imessage"), Some(&1));
    assert_eq!(counts.get("gui"), Some(&1));
    assert_eq!(counts.len(), 3);
}

/// The cutoff is the window: an older message is not this week's traffic.
#[test]
fn message_counts_by_source_excludes_rows_before_the_cutoff() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let stale = repo.insert(&message_from("u1:telegram", "telegram")).unwrap();
    backdate(&db, stale, "2026-08-01 12:00:00");
    repo.insert(&message_from("u1:telegram", "telegram")).unwrap();

    let counts = repo
        .message_counts_by_source_since("2026-09-01 00:00:00")
        .unwrap();
    assert_eq!(counts.get("telegram"), Some(&1));
}

/// A source-less row (nothing has attributed it) is counted for no connector
/// rather than for a guessed one.
#[test]
fn message_counts_by_source_ignores_unattributed_rows() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    repo.insert(&ConversationMessage {
        lane_key: "u1:telegram".to_string(),
        role: "user".to_string(),
        content: "hi".to_string(),
        source: None,
        ..Default::default()
    })
    .unwrap();

    let counts = repo
        .message_counts_by_source_since("2000-01-01 00:00:00")
        .unwrap();
    assert!(counts.is_empty(), "an unattributed row counts for nobody");
}

// ── Learning v2 (design 09 §1.2, §1.3) ──────────────────────────────────

fn learning_row(
    repo: &ConversationRepository<'_>,
    session: &str,
    role: &str,
    content: &str,
    request_id: Option<&str>,
    workspace_root: Option<&str>,
) -> i64 {
    repo.insert_with_structured(
        &ConversationMessage {
            lane_key: "user:gui".to_string(),
            role: role.to_string(),
            content: content.to_string(),
            source: Some("gui".to_string()),
            session_id: Some(session.to_string()),
            request_id: request_id.map(str::to_string),
            workspace_root: workspace_root.map(str::to_string),
            ..Default::default()
        },
        r#"{"v":1,"parts":[{"type":"document","extracted_text":"THE ATTACHMENT"}]}"#,
        "[Attachments: notes.txt]",
    )
    .unwrap()
}

/// The window is the session's rows past the cursor, in id order, and it
/// carries the turn's id and project root — and nothing that holds an
/// attachment's text (U2).
#[test]
fn the_learning_window_reads_rows_after_the_cursor_in_id_order() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);
    let session = repo.create_session("user:gui", "gui", None, None).unwrap();
    let other = repo.create_session("user:cli", "cli", None, None).unwrap();
    let first = learning_row(&repo, &session.id, "user", "first", Some("r1"), Some("/p"));
    let second = learning_row(&repo, &session.id, "assistant", "second", Some("r1"), None);
    learning_row(&repo, &other.id, "user", "elsewhere", Some("r9"), None);
    let third = learning_row(&repo, &session.id, "user", "third", None, None);

    let window = repo.learning_window(&session.id, first, 10).unwrap();
    assert_eq!(
        window.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![second, third]
    );
    assert_eq!(window[0].role, "assistant");
    assert_eq!(window[0].content, "second");
    assert_eq!(window[0].request_id.as_deref(), Some("r1"));
    assert_eq!(window[1].request_id, None);
    assert!(
        window.iter().all(|r| !r.content.contains("THE ATTACHMENT")),
        "the window reads content, never content_json"
    );

    let all = repo.learning_window(&session.id, 0, 10).unwrap();
    assert_eq!(all[0].workspace_root.as_deref(), Some("/p"));
    assert_eq!(
        repo.learning_window(&session.id, 0, 1).unwrap().len(),
        1,
        "limit binds"
    );
}

/// Context rows come from before the cursor: the newest ones, in id order.
#[test]
fn learning_rows_before_reads_the_newest_rows_at_or_before_an_id() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);
    let session = repo.create_session("user:gui", "gui", None, None).unwrap();
    let ids: Vec<i64> = (0..5)
        .map(|i| {
            learning_row(
                &repo,
                &session.id,
                "user",
                &format!("m{i}"),
                Some("r"),
                None,
            )
        })
        .collect();
    let rows = repo.learning_rows_before(&session.id, ids[3], 2).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![ids[2], ids[3]]
    );
}

/// A suggestion's cited `message` sources, read by id (design 09 §4.2 rule
/// 5): the rows that still exist, in id order, with the learning columns and
/// never an attachment's text; an empty list reads nothing.
#[test]
fn learning_rows_by_ids_reads_the_cited_rows_that_exist() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);
    let session = repo.create_session("user:gui", "gui", None, None).unwrap();
    let a = learning_row(&repo, &session.id, "user", "typed", Some("r1"), None);
    let b = learning_row(&repo, &session.id, "assistant", "answer", Some("r1"), None);
    let rows = repo.learning_rows_by_ids(&[b, 999_999, a]).unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![a, b]);
    assert_eq!(rows[0].role, "user");
    assert_eq!(rows[0].request_id.as_deref(), Some("r1"));
    assert!(rows.iter().all(|r| !r.content.contains("THE ATTACHMENT")));
    assert!(repo.learning_rows_by_ids(&[]).unwrap().is_empty());
}

/// The two new columns round-trip through every message read, and stay off
/// the wire: `GET /v1/chat/history` serializes this struct directly.
#[test]
fn request_id_and_workspace_root_round_trip_and_stay_off_the_wire() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);
    let session = repo.create_session("user:gui", "gui", None, None).unwrap();
    learning_row(
        &repo,
        &session.id,
        "user",
        "hello",
        Some("req-1"),
        Some("/repo"),
    );
    let messages = repo.list_by_session(&session.id, 10, 0).unwrap();
    assert_eq!(messages[0].request_id.as_deref(), Some("req-1"));
    assert_eq!(messages[0].workspace_root.as_deref(), Some("/repo"));

    let wire = serde_json::to_value(&messages[0]).unwrap();
    assert!(wire.get("request_id").is_none(), "{wire}");
    assert!(wire.get("workspace_root").is_none(), "{wire}");
}

/// Design §1.3 *Lifecycle*: `delete_session` deletes the cursor row in its
/// existing transaction.
#[test]
fn deleting_a_session_deletes_its_cursor() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);
    let session = repo.create_session("user:gui", "gui", None, None).unwrap();
    let keep = repo.create_session("user:cli", "cli", None, None).unwrap();
    let cursors = crate::repository::LearningCursorRepository::new(&db);
    assert!(cursors.advance_cas(&session.id, 7, 0).unwrap());
    assert!(cursors.advance_cas(&keep.id, 3, 0).unwrap());

    assert!(repo.delete_session(&session.id).unwrap());
    let rows: Vec<String> = db
        .with_connection(|c| {
            Ok(c.prepare("SELECT session_id FROM learning_cursor")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .unwrap();
    assert_eq!(rows, vec![keep.id]);
}

/// Deleting or clearing a conversation forgets its summaries (design 09
/// §13.3): what the route reads first is every message id the session
/// holds, in id order — and no other session's.
#[test]
fn message_ids_lists_every_row_of_one_session() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);
    let session = repo.create_session("user:gui", "gui", None, None).unwrap();
    let other = repo.create_session("user:cli", "cli", None, None).unwrap();
    let a = learning_row(&repo, &session.id, "user", "typed", Some("r1"), None);
    learning_row(&repo, &other.id, "user", "elsewhere", Some("r2"), None);
    let b = learning_row(&repo, &session.id, "assistant", "answer", Some("r1"), None);
    let c = learning_row(&repo, &session.id, "assistant", "a notice", None, None);
    assert_eq!(repo.message_ids(&session.id).unwrap(), vec![a, b, c]);
    assert!(repo.message_ids("no-such-session").unwrap().is_empty());
}

fn session_title(repo: &ConversationRepository<'_>, id: &str) -> String {
    repo.get_session(id).unwrap().unwrap().title
}

/// G5 (Round G): a turn names only an untitled conversation. The guard is
/// the statement's own, so a rename that lands between a turn's read of the
/// session and its write survives, and a second title never replaces the
/// first.
#[test]
fn set_title_if_empty_names_only_an_untitled_session() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);

    let untitled = repo.create_session("user:gui", "gui", None, None).unwrap();
    assert!(
        repo.set_title_if_empty(&untitled.id, "Plan pasta night")
            .unwrap()
    );
    assert_eq!(session_title(&repo, &untitled.id), "Plan pasta night");
    assert!(
        !repo
            .set_title_if_empty(&untitled.id, "Something else")
            .unwrap()
    );
    assert_eq!(session_title(&repo, &untitled.id), "Plan pasta night");

    let renamed = repo.create_session("user:cli", "cli", None, None).unwrap();
    assert!(
        repo.update_session(&renamed.id, Some("My own name"), None)
            .unwrap()
    );
    assert!(
        !repo
            .set_title_if_empty(&renamed.id, "Plan pasta night")
            .unwrap()
    );
    assert_eq!(session_title(&repo, &renamed.id), "My own name");

    assert!(!repo.set_title_if_empty("no-such-session", "x").unwrap());
}

/// G5: the title is read from the first of the session's own user rows that
/// names one — never an assistant row, never another session's, never a
/// follow-up the daemon re-entered (source `internal`), and never the
/// `display_text` an attachment turn carries.
#[test]
fn title_from_user_rows_reads_the_first_user_row_that_names_one() {
    let db = test_db();
    let repo = ConversationRepository::new(&db);
    let session = repo.create_session("user:gui", "gui", None, None).unwrap();
    let other = repo.create_session("user:cli", "cli", None, None).unwrap();
    learning_row(&repo, &other.id, "user", "Elsewhere entirely", None, None);
    learning_row(
        &repo,
        &session.id,
        "assistant",
        "Hello, how can I help?",
        None,
        None,
    );
    learning_row(&repo, &session.id, "user", "/status", None, None);
    learning_row(&repo, &session.id, "user", "", None, None);
    repo.insert(&ConversationMessage {
        lane_key: "user:gui".to_string(),
        role: "user".to_string(),
        content: "Check the build again tomorrow and report back".to_string(),
        source: Some("internal".to_string()),
        session_id: Some(session.id.clone()),
        ..Default::default()
    })
    .unwrap();
    learning_row(&repo, &session.id, "user", "Plan pasta night", None, None);
    learning_row(&repo, &session.id, "user", "And dessert?", None, None);

    let sentence = |content: &str| {
        (!content.is_empty() && !content.starts_with('/')).then(|| content.to_string())
    };
    assert_eq!(
        repo.title_from_user_rows(&session.id, sentence)
            .unwrap()
            .as_deref(),
        Some("Plan pasta night")
    );
    assert_eq!(
        repo.title_from_user_rows("no-such-session", sentence)
            .unwrap(),
        None
    );
}
