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
            task_id: None,
            agent_instance_id: None,
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

// --- The shared upload writer (D2) ---------------------------------------
//
// `store_attachment` owns validation and the `ResolvedAttachment` shape and
// nothing else: hashing, the owner-scoped sha256 dedup, placement and the row
// come from `openalpaca_storage::UploadStore`, the same writer
// `POST /v1/files/upload` calls. These tests pin that delegation — the
// behaviours they assert are the writer's, observed from this caller.

mod attachments {
    use super::*;
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard};

    /// Serializes every test here that re-points `OPENALPACA_HOME_STORE`; the
    /// variable is process-global and every store accessor reads it on each
    /// call. No test ever touches the real `~/.openalpaca`.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct HomeStoreGuard {
        _lock: MutexGuard<'static, ()>,
        prev: Option<OsString>,
    }

    impl HomeStoreGuard {
        fn set(path: &Path) -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let prev = std::env::var_os(openalpaca_storage::store::HOME_STORE_ENV);
            // SAFETY: serialized by ENV_LOCK — this module's only writer.
            unsafe { std::env::set_var(openalpaca_storage::store::HOME_STORE_ENV, path) };
            Self { _lock: lock, prev }
        }
    }

    impl Drop for HomeStoreGuard {
        fn drop(&mut self) {
            // SAFETY: as above — still holding ENV_LOCK.
            match self.prev.take() {
                Some(v) => unsafe {
                    std::env::set_var(openalpaca_storage::store::HOME_STORE_ENV, v)
                },
                None => unsafe { std::env::remove_var(openalpaca_storage::store::HOME_STORE_ENV) },
            }
        }
    }

    const MAX_SIZE: u64 = 10 * 1024 * 1024;
    const MAX_DIM: u32 = 8192;

    fn store(db: &Database, owner: &str, name: &str, data: &[u8]) -> ResolvedAttachment {
        store_attachment(db, owner, name, "text/plain", data, MAX_SIZE, MAX_DIM)
            .expect("store_attachment")
    }

    /// D2: a connector attachment has no project signal, so its bytes take the
    /// home store — `<home>/uploads/<YYYY-MM-DD>/NN-<slug>.<ext>`, never the
    /// daemon's working directory.
    #[test]
    fn an_attachment_lands_in_the_home_store_uploads() {
        let home = tempdir().unwrap();
        let home_root = home.path().canonicalize().unwrap();
        let _guard = HomeStoreGuard::set(&home_root);
        let db = test_db();

        let attachment = store(&db, "owner-1", "Photo Notes.TXT", b"hello");

        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let expected = home_root
            .join("uploads")
            .join(&today)
            .join("01-photo-notes.txt");
        assert_eq!(
            std::path::PathBuf::from(&attachment.storage_path),
            expected,
            "a connector attachment takes the home store"
        );
        assert_eq!(std::fs::read(&expected).unwrap(), b"hello");
        // The name the user sees is the one they sent; only the file is slugified.
        assert_eq!(attachment.filename, "Photo Notes.TXT");
        assert_eq!(attachment.size_bytes, 5);
        // The row is the writer's, and it counts as upload traffic.
        let repo = openalpaca_storage::FileAssetRepository::new(&db);
        let row = repo.get_by_id(&attachment.file_id).unwrap().expect("row");
        assert_eq!(row.storage_path, attachment.storage_path);
        assert_eq!(repo.total_storage_bytes().unwrap(), 5);
    }

    #[test]
    fn the_same_bytes_dedup_to_the_first_row() {
        let home = tempdir().unwrap();
        let _guard = HomeStoreGuard::set(&home.path().canonicalize().unwrap());
        let db = test_db();

        let first = store(&db, "owner-1", "notes.txt", b"hello");
        // A different name, the same bytes — dedup keys off sha256, not the path.
        let second = store(&db, "owner-1", "renamed.txt", b"hello");

        assert_eq!(second.file_id, first.file_id);
        assert_eq!(second.storage_path, first.storage_path);
        assert_eq!(
            openalpaca_storage::FileAssetRepository::new(&db)
                .total_storage_bytes()
                .unwrap(),
            5,
            "a dedup hit inserts no second row"
        );
    }

    #[test]
    fn validation_still_runs_before_the_writer() {
        let home = tempdir().unwrap();
        let _guard = HomeStoreGuard::set(&home.path().canonicalize().unwrap());
        let db = test_db();

        let err = store_attachment(
            &db,
            "owner-1",
            "big.txt",
            "text/plain",
            b"hello",
            1,
            MAX_DIM,
        )
        .expect_err("a file over the cap is rejected");
        assert!(err.contains("Upload validation failed"), "{err}");
        assert_eq!(
            openalpaca_storage::FileAssetRepository::new(&db)
                .total_storage_bytes()
                .unwrap(),
            0,
            "a rejected attachment writes nothing"
        );
    }
}
