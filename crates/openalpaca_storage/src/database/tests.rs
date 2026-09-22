use super::*;
use tempfile::tempdir;

#[test]
fn test_database_creation() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let db = Database::open(&db_path).unwrap();
    assert!(db_path.exists());
    assert_eq!(db.schema_version().unwrap(), migrations::BASELINE_VERSION);
}

#[test]
fn reopening_a_current_database_preserves_its_data_and_version() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    {
        let db = Database::open(&db_path).unwrap();
        db.with_connection(|conn| {
            conn.execute_batch(
                "INSERT INTO session (id, lane_key, source, title)
                 VALUES ('session-1', 'user:gui', 'gui', 'Keep this conversation');
                 INSERT INTO conversation_messages (lane_key, role, content, session_id)
                 VALUES ('user:gui', 'user', 'Keep this message', 'session-1');",
            )?;
            Ok(())
        })
        .unwrap();
    }

    let db = Database::open(&db_path).unwrap();
    assert_eq!(db.schema_version().unwrap(), migrations::BASELINE_VERSION);
    db.with_connection(|conn| {
        let (title, content): (String, String) = conn.query_row(
            "SELECT s.title, m.content FROM session s
             JOIN conversation_messages m ON m.session_id = s.id
             WHERE s.id = 'session-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(title, "Keep this conversation");
        assert_eq!(content, "Keep this message");

        let versions = conn
            .prepare("SELECT version FROM schema_version ORDER BY version")?
            .query_map([], |row| row.get::<_, i32>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        assert_eq!(versions, vec![migrations::BASELINE_VERSION]);
        Ok(())
    })
    .unwrap();
}

#[test]
fn an_unsupported_old_database_is_refused_without_changing_its_schema_or_data() {
    let dir = tempdir().unwrap();
    for version in [1, migrations::BASELINE_VERSION - 1] {
        let db_path = dir.path().join(format!("legacy-{version}.db"));
        let original_schema = {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
                 CREATE TABLE legacy_data (id INTEGER PRIMARY KEY, content TEXT NOT NULL);
                 INSERT INTO legacy_data (id, content) VALUES (7, 'Do not discard');",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO schema_version (version) VALUES (?1)",
                [version],
            )
            .unwrap();
            conn.prepare("SELECT name, sql FROM sqlite_master ORDER BY name")
                .unwrap()
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };

        let error = Database::open(&db_path)
            .err()
            .unwrap_or_else(|| panic!("legacy version {version} must be refused"));
        let message = format!("{error:#}");
        assert!(
            message.contains(&format!("Unsupported legacy schema version {version}")),
            "opening an old database must deliberately refuse it: {message}"
        );
        // The remedy is deleting the file, so the refusal has to say which file.
        // The store root is overridable, and a developer reading this line in a
        // log has no other way to find out.
        let named = std::path::absolute(&db_path).unwrap();
        assert!(
            message.contains(&named.display().to_string()),
            "the refusal must name the database file by its full path ({}): {message}",
            named.display()
        );
        assert!(
            message.contains(&format!("schema version {}", migrations::BASELINE_VERSION)),
            "the refusal must name the version this build starts at: {message}"
        );
        assert!(
            message.contains("Delete "),
            "the refusal must prescribe deleting that file: {message}"
        );

        let conn = Connection::open(&db_path).unwrap();
        let schema = conn
            .prepare("SELECT name, sql FROM sqlite_master ORDER BY name")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            schema, original_schema,
            "refusal must leave the schema intact"
        );
        let rows = conn
            .prepare("SELECT id, content FROM legacy_data ORDER BY id")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(rows, vec![(7, "Do not discard".to_string())]);
        let versions = conn
            .prepare("SELECT version FROM schema_version ORDER BY version")
            .unwrap()
            .query_map([], |row| row.get::<_, i32>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            versions,
            vec![version],
            "refusal must not advance the version"
        );
    }
}

#[test]
fn test_schema_omits_obsolete_planner_telemetry() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();

    db.with_connection(|conn| {
        let columns = |table: &str| -> rusqlite::Result<Vec<String>> {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(names)
        };

        let latency = columns("orchestrator_latency")?;
        assert!(
            !latency.contains(&"planner_ms".to_string()),
            "orchestrator_latency.planner_ms should be gone: {latency:?}"
        );
        assert!(
            !latency.contains(&"dispatch_ms".to_string()),
            "orchestrator_latency.dispatch_ms should be gone: {latency:?}"
        );
        // The column the metric actually uses survives.
        assert!(latency.contains(&"ack_ms".to_string()), "{latency:?}");

        let decisions = columns("dispatch_decisions")?;
        assert!(
            !decisions.contains(&"planner_requested_mode".to_string()),
            "dispatch_decisions.planner_requested_mode should be gone: {decisions:?}"
        );
        assert!(
            decisions.contains(&"error_message".to_string()),
            "{decisions:?}"
        );

        Ok(())
    })
    .unwrap();
}

#[test]
fn test_foreign_keys_enforced() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();

    // Insert task_agent_assignment with nonexistent task_id should fail (FK)
    let result = db.with_connection(|c| {
        c.execute(
            "INSERT INTO task_agent_assignment(id, task_id, agent_id, role, status)
             VALUES ('a1', 'nonexistent-task', 'agent-1', 'test', 'pending')",
            [],
        )?;
        Ok(())
    });

    assert!(result.is_err(), "Expected foreign key constraint error");
}

#[test]
fn test_sqlite_vec_available() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    db.with_connection(|conn| {
        // 1. Verify extension loaded
        let version: String = conn.query_row("SELECT vec_version()", [], |row| row.get(0))?;
        assert!(
            !version.is_empty(),
            "vec_version() should return a version string"
        );

        // 2. Verify the schema includes the vector table
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='memory_vec')",
            [],
            |row| row.get(0),
        )?;
        assert!(exists, "memory_vec table should exist");

        // 3. Insert a zero vector (768 floats x 4 bytes = 3072 bytes of zeroblob)
        conn.execute(
            "INSERT INTO memory_vec(memory_id, embedding) VALUES (1, vec_f32(zeroblob(3072)))",
            [],
        )?;

        // 4. Verify round-trip
        let count: i64 = conn.query_row("SELECT count(*) FROM memory_vec", [], |row| row.get(0))?;
        assert_eq!(count, 1);

        Ok(())
    })
    .unwrap();
}

#[test]
fn test_fts_update_sync() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();

    db.with_connection(|c| {
        // Insert v2 memory with "old" keyword
        c.execute(
            "INSERT INTO memory(owner_id, kind, scope, scope_id, source, content, content_hash)
             VALUES ('owner-1', 'fact', 'global', '', 'conversation', 'old keyword here', 'hash1')",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    // Update content to "new" keyword
    db.with_connection(|c| {
        c.execute(
            "UPDATE memory SET content = 'new keyword here', content_hash = 'hash2' WHERE owner_id='owner-1'",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    // Verify FTS sync: old should not match, new should match
    db.with_connection(|c| {
        let old_hits: i64 = c.query_row(
            "SELECT count(*) FROM memory_fts WHERE content MATCH 'old'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(old_hits, 0, "Old keyword should NOT be found after update");

        let new_hits: i64 = c.query_row(
            "SELECT count(*) FROM memory_fts WHERE content MATCH 'new'",
            [],
            |r| r.get(0),
        )?;
        assert!(new_hits >= 1, "New keyword should be found after update");
        Ok(())
    })
    .unwrap();
}

/// Insert a minimal `file_assets` row, returning the rusqlite result so tests
/// can assert on constraint violations.
fn insert_asset(
    conn: &rusqlite::Connection,
    id: &str,
    project_root: Option<&str>,
    rel_path: Option<&str>,
) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO file_assets (id, owner_id, sha256, filename, mime_type, size_bytes, storage_path, project_root, rel_path)
         VALUES (?1, 'owner-1', 'sha-' || ?1, 'f.md', 'text/markdown', 10, '/tmp/' || ?1, ?2, ?3)",
        rusqlite::params![id, project_root, rel_path],
    )
}

#[test]
fn test_artifact_schema_columns_defaults_and_indexes() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();

    db.with_connection(|conn| {
        let columns = |table: &str| -> rusqlite::Result<Vec<String>> {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(names)
        };

        let assets = columns("file_assets")?;
        for expected in [
            "origin",
            "kind",
            "task_id",
            "agent_id",
            "agent_template_id",
            "project_root",
            "rel_path",
            "version",
            "version_count",
            "pinned",
            "summary",
            "missing_since",
        ] {
            assert!(
                assets.contains(&expected.to_string()),
                "file_assets.{expected} should exist: {assets:?}"
            );
        }

        // The address columns default to NULL, the rest to the documented values.
        insert_asset(conn, "a1", None, None)?;
        let (origin, version, version_count, pinned, kind): (
            String,
            i64,
            i64,
            i64,
            Option<String>,
        ) = conn.query_row(
            "SELECT origin, version, version_count, pinned, kind FROM file_assets WHERE id = 'a1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        assert_eq!(origin, "upload");
        assert_eq!(version, 1);
        assert_eq!(version_count, 1);
        assert_eq!(pinned, 0);
        assert_eq!(kind, None);

        let task = columns("task")?;
        assert!(
            task.contains(&"workspace_id".to_string()),
            "task.workspace_id should exist: {task:?}"
        );

        let versions = columns("artifact_versions")?;
        for expected in [
            "artifact_id",
            "version",
            "rel_path",
            "sha256",
            "size_bytes",
            "note",
            "author_agent_id",
            "added_lines",
            "removed_lines",
            "created_at",
        ] {
            assert!(
                versions.contains(&expected.to_string()),
                "artifact_versions.{expected} should exist: {versions:?}"
            );
        }

        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%'")?;
        let indexes = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for expected in [
            "idx_file_assets_task",
            "idx_file_assets_origin",
            "idx_file_assets_project",
            "idx_file_assets_addr",
            "idx_artifact_versions_artifact",
            "idx_task_workspace",
        ] {
            assert!(
                indexes.contains(&expected.to_string()),
                "{expected} should exist: {indexes:?}"
            );
        }

        Ok(())
    })
    .unwrap();
}

#[test]
fn test_artifact_address_is_unique_but_null_rel_path_is_free() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();

    db.with_connection(|conn| {
        insert_asset(conn, "a1", Some("/proj"), Some("notes/plan.md"))?;

        // Same address => rejected.
        let dup = insert_asset(conn, "a2", Some("/proj"), Some("notes/plan.md"));
        assert!(
            dup.is_err(),
            "duplicate (project_root, rel_path) should violate idx_file_assets_addr"
        );

        // Same rel_path under a different project => allowed.
        insert_asset(conn, "a3", Some("/other"), Some("notes/plan.md"))?;

        // The home store is COALESCE'd to '' — distinct from any project root,
        // and still unique within itself.
        insert_asset(conn, "a4", None, Some("notes/plan.md"))?;
        let dup_home = insert_asset(conn, "a5", None, Some("notes/plan.md"));
        assert!(
            dup_home.is_err(),
            "duplicate home-store rel_path should violate idx_file_assets_addr"
        );

        // The index is partial: unaddressed rows (legacy uploads) never collide.
        insert_asset(conn, "a6", None, None)?;
        insert_asset(conn, "a7", None, None)?;
        insert_asset(conn, "a8", Some("/proj"), None)?;

        let count: i64 = conn.query_row("SELECT count(*) FROM file_assets", [], |r| r.get(0))?;
        assert_eq!(count, 6, "a2 and a5 should be the only rejected inserts");

        Ok(())
    })
    .unwrap();
}

#[test]
fn test_artifact_versions_are_unique_and_cascade() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();

    db.with_connection(|conn| {
        insert_asset(conn, "a1", Some("/proj"), Some("notes/plan.md"))?;
        conn.execute(
            "INSERT INTO artifact_versions (artifact_id, version, rel_path, sha256, size_bytes)
             VALUES ('a1', 1, 'notes/plan.md', 'sha1', 10)",
            [],
        )?;
        // The PK is (artifact_id, version).
        let dup = conn.execute(
            "INSERT INTO artifact_versions (artifact_id, version, rel_path, sha256, size_bytes)
             VALUES ('a1', 1, '.versions/plan/v1.md', 'sha2', 11)",
            [],
        );
        assert!(dup.is_err(), "(artifact_id, version) should be unique");

        // Orphan version rows are impossible.
        let orphan = conn.execute(
            "INSERT INTO artifact_versions (artifact_id, version, rel_path, sha256, size_bytes)
             VALUES ('nope', 1, 'x.md', 'sha3', 12)",
            [],
        );
        assert!(orphan.is_err(), "artifact_id should be a live FK");

        conn.execute("DELETE FROM file_assets WHERE id = 'a1'", [])?;
        let left: i64 =
            conn.query_row("SELECT count(*) FROM artifact_versions", [], |r| r.get(0))?;
        assert_eq!(left, 0, "versions should cascade with the artifact");

        Ok(())
    })
    .unwrap();
}

#[test]
fn test_run_observability_schema_and_constraints() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();

    db.with_connection(|conn| {
        let columns = |table: &str| -> rusqlite::Result<Vec<String>> {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(names)
        };

        let span = columns("subagent_span")?;
        for expected in [
            "id",
            "task_id",
            "template_id",
            "agent_instance_id",
            "label",
            "objective",
            "state",
            "detail",
            "started_at",
            "ended_at",
            "duration_ms",
            "output_preview",
        ] {
            assert!(
                span.contains(&expected.to_string()),
                "subagent_span.{expected} should exist: {span:?}"
            );
        }

        // GAP-10's run-scoped log column and GAP-06's re-run provenance link.
        assert!(columns("event_log")?.contains(&"task_id".to_string()));
        assert!(columns("task")?.contains(&"source_task_id".to_string()));

        conn.execute(
            "INSERT INTO task (id, title, status, priority, created_by, source_lane)
             VALUES ('t1', 'run', 'running', 0, 'tester', 'user:cli')",
            [],
        )?;

        // The state word is constrained.
        let bad = conn.execute(
            "INSERT INTO subagent_span (id, task_id, template_id, agent_instance_id, label, state, started_at)
             VALUES ('s-bad', 't1', 'a', 'a::1', 'a·1', 'wandering', '2026-09-05T10:00:00.000Z')",
            [],
        );
        assert!(bad.is_err(), "state should be CHECK-constrained");

        conn.execute(
            "INSERT INTO subagent_span (id, task_id, template_id, agent_instance_id, label, state, started_at)
             VALUES ('s1', 't1', 'review_agent', 'review_agent::1', 'review·1', 'running', '2026-09-05T10:00:00.000Z')",
            [],
        )?;

        // Labels are unique per task, not globally.
        let dup = conn.execute(
            "INSERT INTO subagent_span (id, task_id, template_id, agent_instance_id, label, state, started_at)
             VALUES ('s2', 't1', 'review_agent', 'review_agent::2', 'review·1', 'running', '2026-09-05T10:00:01.000Z')",
            [],
        );
        assert!(dup.is_err(), "(task_id, label) should be unique");

        // An orphan span is impossible, and spans cascade with their run.
        let orphan = conn.execute(
            "INSERT INTO subagent_span (id, task_id, template_id, agent_instance_id, label, state, started_at)
             VALUES ('s3', 'ghost', 'a', 'a::1', 'a·1', 'running', '2026-09-05T10:00:00.000Z')",
            [],
        );
        assert!(orphan.is_err(), "task_id should be a live FK");

        conn.execute("DELETE FROM task WHERE id = 't1'", [])?;
        let left: i64 = conn.query_row("SELECT count(*) FROM subagent_span", [], |r| r.get(0))?;
        assert_eq!(left, 0, "spans should cascade with the task");

        Ok(())
    })
    .unwrap();
}

#[test]
fn test_message_run_links_are_nullable_without_foreign_keys() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();

    db.with_connection(|conn| {
        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(conversation_messages)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        assert!(
            columns.contains(&"task_id".to_string()),
            "conversation_messages.task_id should exist: {columns:?}"
        );

        let indexes: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'conversation_messages'")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        assert!(
            indexes.contains(&"idx_conv_msg_task".to_string()),
            "idx_conv_msg_task should exist: {indexes:?}"
        );

        // The link is deliberately *not* a foreign key: a run's row can be
        // purged while the turn that started it stays in the transcript, and
        // the column is then a dangling id the client renders as plain text.
        conn.execute(
            "INSERT INTO conversation_messages (lane_key, role, content, task_id)
             VALUES ('user:gui', 'assistant', 'started', 'no-such-run')",
            [],
        )?;
        let stored: Option<String> = conn.query_row(
            "SELECT task_id FROM conversation_messages WHERE content = 'started'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(stored.as_deref(), Some("no-such-run"));

        // A message without a run link reads back NULL, never an empty string.
        conn.execute(
            "INSERT INTO conversation_messages (lane_key, role, content)
             VALUES ('user:gui', 'user', 'chat only')",
            [],
        )?;
        let bare: Option<String> = conn.query_row(
            "SELECT task_id FROM conversation_messages WHERE content = 'chat only'",
            [],
            |row| row.get(0),
        )?;
        assert!(bare.is_none());

        Ok(())
    })
    .unwrap();
}

#[test]
fn test_session_schema_and_links() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();

    db.with_connection(|conn| {
        // Session is the only transcript container.
        let leftover: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'conversations'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            leftover, 0,
            "`conversations` must not coexist with `session`"
        );

        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(session)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for expected in [
            "id",
            "lane_key",
            "source",
            "title",
            "workspace_id",
            "status",
            "message_count",
            "last_message_at",
            "summary",
            "summary_version",
            "last_summarized_message_id",
            "summary_updated_at",
            "ended_at",
            "created_at",
            "updated_at",
        ] {
            assert!(
                columns.contains(&expected.to_string()),
                "session.{expected} should exist: {columns:?}"
            );
        }

        // `session_id` reaches every table §5.2 keys by it.
        let has_column = |table: &str, column: &str| -> rusqlite::Result<bool> {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(names.contains(&column.to_string()))
        };
        for table in [
            "conversation_messages",
            "task",
            "lane_followups",
            "tool_execution_log",
        ] {
            assert!(
                has_column(table, "session_id")?,
                "{table}.session_id should exist"
            );
        }
        for column in [
            "task_id",
            "log_seq",
            "args_preview",
            "result_preview",
            "result_ref",
        ] {
            assert!(
                has_column("tool_execution_log", column)?,
                "tool_execution_log.{column} should exist"
            );
        }

        Ok(())
    })
    .unwrap();
}

#[test]
fn test_partial_index_allows_one_active_session_per_lane() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();

    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO session (id, lane_key, source, status) VALUES ('s1', 'u:gui', 'gui', 'active')",
            [],
        )?;

        // A second *active* session on the same lane is refused by the DB —
        // one-active-per-lane is an invariant, not a convention.
        let clash = conn.execute(
            "INSERT INTO session (id, lane_key, source, status) VALUES ('s2', 'u:gui', 'gui', 'active')",
            [],
        );
        assert!(clash.is_err(), "a second active session must be rejected");

        // Archived siblings are unlimited; only active lanes are unique.
        conn.execute(
            "INSERT INTO session (id, lane_key, source, status) VALUES ('s2', 'u:gui', 'gui', 'archived')",
            [],
        )?;
        conn.execute(
            "INSERT INTO session (id, lane_key, source, status) VALUES ('s3', 'u:gui', 'gui', 'archived')",
            [],
        )?;
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM session WHERE lane_key = 'u:gui'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(total, 3);

        // And the status domain is checked.
        assert!(
            conn.execute(
                "INSERT INTO session (id, lane_key, source, status) VALUES ('s4', 'u:cli', 'cli', 'paused')",
                [],
            )
            .is_err(),
            "status must be 'active' or 'archived'"
        );

        Ok(())
    })
    .unwrap();
}

/// `factory_reset` must empty every table holding user content, including
/// those no foreign key reaches. `lane_followups` is free-standing: a queued
/// follow-up that survives the wipe is fired by `GatewayFollowupRunner` as a
/// turn against the emptied database.
#[test]
fn factory_reset_empties_current_schema_tables() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("reset.db")).unwrap();
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO session (id, lane_key, source, status, created_at, updated_at) \
             VALUES ('s1', 'u:gui', 'gui', 'active', datetime('now'), datetime('now'))",
            [],
        )?;
        conn.execute(
            "INSERT INTO lane_followups (lane_key, kind, content, principal_json, status) \
             VALUES ('u:gui', 'followup', 'and then check the logs', '{}', 'queued')",
            [],
        )?;
        seed_an_artifact_and_its_version(conn)?;
        Ok(())
    })
    .unwrap();
    db.factory_reset()
        .expect("factory_reset must succeed on the current schema");
    let (sessions, followups, versions): (i64, i64, i64) = db
        .with_connection(|conn| {
            Ok((
                conn.query_row("SELECT COUNT(*) FROM session", [], |r| r.get(0))?,
                conn.query_row("SELECT COUNT(*) FROM lane_followups", [], |r| r.get(0))?,
                conn.query_row("SELECT COUNT(*) FROM artifact_versions", [], |r| r.get(0))?,
            ))
        })
        .unwrap();
    assert_eq!(sessions, 0, "the reset empties the session table");
    assert_eq!(
        followups, 0,
        "a queued follow-up must not outlive a factory reset"
    );
    assert_eq!(versions, 0, "version history goes with the rows");
}

/// One produced artifact row and one `artifact_versions` row for it.
fn seed_an_artifact_and_its_version(conn: &Connection) -> Result<()> {
    conn.execute(
        "INSERT INTO file_assets \
            (id, owner_id, sha256, filename, mime_type, size_bytes, storage_path, status, origin) \
         VALUES ('f1', 'owner', 'sha', '01-notes.md', 'text/markdown', 4, \
                 '/nowhere/01-notes.md', 'ready', 'produced')",
        [],
    )?;
    conn.execute(
        "INSERT INTO artifact_versions (artifact_id, version, rel_path, sha256, size_bytes) \
         VALUES ('f1', 1, 'loose/2026-09-01/01-notes.md', 'sha', 4)",
        [],
    )?;
    Ok(())
}

/// The reset names every table it empties rather than leaning on a cascade —
/// the rule `subagent_span`'s line already states in as many words. Proven the
/// only way it can be: with `foreign_keys` off, where an unnamed child table
/// simply survives.
#[test]
fn factory_reset_empties_artifact_versions_without_the_cascade() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("reset-nofk.db")).unwrap();
    db.with_connection(|conn| {
        seed_an_artifact_and_its_version(conn)?;
        conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
        Ok(())
    })
    .unwrap();

    db.factory_reset().unwrap();

    let versions: i64 = db
        .with_connection(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM artifact_versions", [], |r| r.get(0))?)
        })
        .unwrap();
    assert_eq!(versions, 0, "the DELETE is named, not inherited");
}

/// The usage summary needs a timestamp-leading index to avoid a full scan
/// of the append-only call log on every completion refetch.
#[test]
fn test_llm_call_log_has_a_timestamp_index() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();

    db.with_connection(|conn| {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master \
             WHERE type = 'index' AND name = 'idx_llm_call_log_timestamp' \
             AND tbl_name = 'llm_call_log')",
            [],
            |row| row.get(0),
        )?;
        assert!(
            exists,
            "idx_llm_call_log_timestamp should exist on llm_call_log"
        );
        Ok(())
    })
    .unwrap();
}

/// The tools and skills "today" counts must search the timestamp-leading
/// indexes instead of scanning append-only logs in grouping-column order.
#[test]
fn test_execution_log_counts_use_timestamp_indexes() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();

    db.with_connection(|conn| {
        let plan = |sql: &str| -> rusqlite::Result<String> {
            let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
            let steps = stmt
                .query_map(["2026-09-11 00:00:00"], |row| row.get::<_, String>(3))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            Ok(steps.join(" | "))
        };

        let tools = plan(
            "SELECT tool_name, COUNT(*) FROM tool_execution_log \
             INDEXED BY idx_tel_timestamp \
             WHERE timestamp >= ?1 GROUP BY tool_name",
        )?;
        assert!(
            tools.contains("SEARCH") && tools.contains("idx_tel_timestamp"),
            "the tool counts must search the new index, not scan the log: {tools}"
        );

        let skills = plan(
            "SELECT skill_id, COUNT(*) FROM skill_execution_log \
             INDEXED BY idx_sel_timestamp \
             WHERE timestamp >= ?1 GROUP BY skill_id",
        )?;
        assert!(
            skills.contains("SEARCH") && skills.contains("idx_sel_timestamp"),
            "and so must the skill counts: {skills}"
        );

        // Yesterday and today, so the range bound is doing something.
        conn.execute(
            "INSERT INTO tool_execution_log (agent_id, tool_name, success, duration_ms, timestamp)
             VALUES ('a', 'file_read', 1, 2, '2026-09-10 23:00:00'),
                    ('a', 'file_read', 1, 2, '2026-09-11 09:00:00')",
            [],
        )?;
        conn.execute(
            "INSERT INTO skill_execution_log
                (request_id, skill_id, agent_id, status, duration_ms, timestamp)
             VALUES ('r1', 'summarise', 'a', 'complete', 2, '2026-09-10 23:00:00'),
                    ('r2', 'summarise', 'a', 'complete', 2, '2026-09-11 09:00:00')",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    // The hinted statements are the repository's own: they must prepare against
    // the schema's indexes, and answer the same counts.
    let repo = crate::repository::SkillExecutionRepository::new(&db);
    let since = "2026-09-11 00:00:00";
    assert_eq!(
        repo.tool_invocations_since(since).unwrap().get("file_read"),
        Some(&1)
    );
    assert_eq!(
        repo.skill_invocations_since(since)
            .unwrap()
            .get("summarise"),
        Some(&1)
    );
}
