use super::*;
use openalpaca_storage::Database;
use tempfile::tempdir;

fn test_db() -> Database {
    let dir = tempdir().unwrap();
    Database::open(&dir.path().join("test.db")).unwrap()
}

#[test]
fn test_resolve_principal_untrusted() {
    let db = test_db();
    let repo = IdentityRepository::new(&db);

    // Test untrusted
    let (principal, _) = resolve_principal(&repo, "telegram", "user123", Some("Alice")).unwrap();
    assert!(matches!(principal, Principal::External { id, .. } if id == "user123"));
}

#[test]
fn test_resolve_principal_trusted() {
    let db = test_db();
    let repo = IdentityRepository::new(&db);

    // Link user first
    repo.create_global_user("global1", None).unwrap();
    let ext = repo
        .get_or_create_external_identity("telegram", "user123", None)
        .unwrap();
    repo.link_external_identity(ext.id, "global1").unwrap();

    // Test trusted
    let (principal, _) = resolve_principal(&repo, "telegram", "user123", None).unwrap();
    assert!(matches!(principal, Principal::User { global_id } if global_id == "global1"));
}

#[test]
fn test_handle_link_token_flow() {
    let db = test_db();
    let repo = IdentityRepository::new(&db);

    repo.create_global_user("global1", None).unwrap();
    repo.create_link_token("global1", "TOKEN1").unwrap();
    let ext = repo
        .get_or_create_external_identity("telegram", "user123", None)
        .unwrap();

    // Consume
    let res = handle_link_token(&repo, "TOKEN1", ext.id).unwrap();
    assert!(matches!(res, LinkResult::Success(uid) if uid == "global1"));

    // Verify linked in DB
    let ext_after = repo
        .get_external_identity("telegram", "user123")
        .unwrap()
        .unwrap();
    assert_eq!(ext_after.global_user_id, Some("global1".to_string()));
}

// --- Tool confirmation helpers (shared by iMessage/Discord intercepts) ---

mod confirmation {
    use super::super::{format_confirmation_prompt, intercept_confirmation_reply};
    use dashmap::DashMap;
    use openalpaca_core::security::confirmation::{ConfirmationBroker, ConfirmationRequest};
    use std::collections::VecDeque;

    fn make_request(request_id: &str) -> ConfirmationRequest {
        ConfirmationRequest {
            request_id: request_id.to_string(),
            agent_id: "agent-1".to_string(),
            tool_name: "shell_exec".to_string(),
            tool_arguments: serde_json::json!({"cmd": "ls"}),
            stream_id: None,
            lane_key: Some("global1:discord".to_string()),
            timestamp: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_format_confirmation_prompt_basic() {
        let prompt =
            format_confirmation_prompt("shell_exec", &serde_json::json!({"cmd": "ls"}), 1);
        assert!(prompt.contains("Tool: shell_exec"));
        assert!(prompt.contains("\"cmd\": \"ls\""));
        assert!(prompt.contains("Reply /yes or /no"));
        assert!(!prompt.contains("pending"));
    }

    #[test]
    fn test_format_confirmation_prompt_queue_hint() {
        let prompt = format_confirmation_prompt("shell_exec", &serde_json::json!({}), 3);
        assert!(prompt.contains("(1 of 3 pending)"));
    }

    #[test]
    fn test_format_confirmation_prompt_truncates_long_args_on_char_boundary() {
        // Multi-byte chars straddling the 500-byte cut must not panic
        let args = serde_json::json!({"text": "\u{4e16}".repeat(400)});
        let prompt = format_confirmation_prompt("t", &args, 1);
        assert!(prompt.contains("..."));
    }

    #[test]
    fn test_intercept_roundtrip_approve() {
        let broker = ConfirmationBroker::new();
        let mut rx = broker.request(&make_request("req-1"));

        let pending: DashMap<u64, VecDeque<String>> = DashMap::new();
        pending
            .entry(42u64)
            .or_default()
            .push_back("req-1".to_string());

        let reply = intercept_confirmation_reply("/yes", &42u64, &broker, &pending)
            .expect("should intercept");
        assert!(reply.contains("Approved"));
        assert!(!reply.contains("more pending"));

        let response = rx.try_recv().expect("broker should deliver response");
        assert!(response.approved);
        // Queue drained
        assert!(pending.get(&42u64).map(|q| q.is_empty()).unwrap_or(true));
    }

    #[test]
    fn test_intercept_roundtrip_deny_with_remaining_queue() {
        let broker = ConfirmationBroker::new();
        let mut rx1 = broker.request(&make_request("req-1"));
        let _rx2 = broker.request(&make_request("req-2"));

        let pending: DashMap<String, VecDeque<String>> = DashMap::new();
        pending
            .entry("chat123".to_string())
            .or_default()
            .extend(["req-1".to_string(), "req-2".to_string()]);

        let reply =
            intercept_confirmation_reply("/n", &"chat123".to_string(), &broker, &pending)
                .expect("should intercept");
        assert!(reply.contains("Denied"));
        assert!(reply.contains("1 more pending"));

        let response = rx1.try_recv().expect("broker should deliver response");
        assert!(!response.approved);
        // FIFO: req-2 remains
        assert_eq!(
            pending.get("chat123").unwrap().front(),
            Some(&"req-2".to_string())
        );
    }

    #[test]
    fn test_intercept_ignores_non_commands_and_unknown_keys() {
        let broker = ConfirmationBroker::new();
        let pending: DashMap<u64, VecDeque<String>> = DashMap::new();
        pending
            .entry(42u64)
            .or_default()
            .push_back("req-1".to_string());

        // Not a confirmation command -> fall through
        assert!(intercept_confirmation_reply("hello", &42u64, &broker, &pending).is_none());
        // Command but no pending confirmation for this conversation -> fall through
        assert!(intercept_confirmation_reply("/yes", &7u64, &broker, &pending).is_none());
        // Queue untouched
        assert_eq!(pending.get(&42u64).unwrap().len(), 1);
    }

    #[test]
    fn test_intercept_accepts_all_command_forms_case_insensitive() {
        let broker = ConfirmationBroker::new();
        let pending: DashMap<u64, VecDeque<String>> = DashMap::new();
        for (cmd, expect_approved) in
            [("/yes", true), ("/Y", true), (" /No ", false), ("/n", false)]
        {
            let mut rx = broker.request(&make_request("req-x"));
            pending
                .entry(1u64)
                .or_default()
                .push_back("req-x".to_string());
            let reply = intercept_confirmation_reply(cmd, &1u64, &broker, &pending)
                .unwrap_or_else(|| panic!("{cmd} should intercept"));
            let response = rx.try_recv().expect("broker should deliver response");
            assert_eq!(response.approved, expect_approved, "cmd={cmd}");
            assert_eq!(reply.contains("Approved"), expect_approved, "cmd={cmd}");
        }
    }
}
