use super::*;
use tempfile::tempdir;

#[test]
fn test_database_creation() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let db = Database::open(&db_path).unwrap();
    assert!(db_path.exists());
    assert_eq!(db.schema_version().unwrap(), 39);
}

#[test]
fn test_migrations_idempotent() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    // Open twice - migrations should only run once
    let _db1 = Database::open(&db_path).unwrap();
    let db2 = Database::open(&db_path).unwrap();

    assert_eq!(db2.schema_version().unwrap(), 39);
}

#[test]
fn test_migration_035_drops_planner_telemetry() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    assert_eq!(db.schema_version().unwrap(), 39);

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

        // 2. Verify migration created the table
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='memory_vec')",
            [],
            |row| row.get(0),
        )?;
        assert!(exists, "memory_vec table should exist after migration");

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
fn test_migration_036_adds_artifact_columns() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    assert_eq!(db.schema_version().unwrap(), 39);

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
fn test_migration_036_address_is_unique_but_null_rel_path_is_free() {
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
fn test_migration_036_artifact_versions_cascade() {
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
fn test_migration_037_run_observability_schema() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    assert_eq!(db.schema_version().unwrap(), 39);

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
fn test_migration_038_message_run_links() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    assert_eq!(db.schema_version().unwrap(), 39);

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

        // Every pre-038 row reads back NULL, never an empty string.
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
fn test_migration_039_rebuilds_conversations_as_session() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();

    db.with_connection(|conn| {
        // The old table is gone — rebuilt, not shadowed by a second
        // transcript container.
        let leftover: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'conversations'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(leftover, 0, "`conversations` should have been dropped");

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
        for column in ["task_id", "log_seq", "args_preview", "result_preview", "result_ref"] {
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
fn test_migration_039_partial_index_allows_one_active_session_per_lane() {
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

        // Archived siblings are unlimited — that is the whole point of
        // dropping 011's column-level UNIQUE(lane_key).
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

#[test]
fn test_migration_039_backfills_one_active_session_per_existing_lane() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");

    // Build a pre-039 database, stop at 38, then seed it the way 011..038 left it.
    {
        let db = Database::open(&path).unwrap();
        db.with_connection(|conn| {
            conn.execute("DELETE FROM session", [])?;
            conn.execute(
                "INSERT INTO session (id, lane_key, source, title, message_count, summary)
                 VALUES ('legacy-1', 'alice:gui', 'gui', 'Old chat', 2, 'a summary')",
                [],
            )?;
            conn.execute(
                "INSERT INTO conversation_messages (lane_key, role, content, session_id)
                 VALUES ('alice:gui', 'user', 'hello', 'legacy-1')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    }

    let db = Database::open(&path).unwrap();
    db.with_connection(|conn| {
        let (status, workspace): (String, Option<String>) = conn.query_row(
            "SELECT status, workspace_id FROM session WHERE id = 'legacy-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(status, "active", "a carried-over lane keeps today's semantics");
        assert!(workspace.is_none(), "no workspace is known for a legacy row");
        let linked: String = conn.query_row(
            "SELECT session_id FROM conversation_messages WHERE content = 'hello'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(linked, "legacy-1");
        Ok(())
    })
    .unwrap();
}
