use super::*;
use tempfile::tempdir;

#[test]
fn test_database_creation() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let db = Database::open(&db_path).unwrap();
    assert!(db_path.exists());
    assert_eq!(db.schema_version().unwrap(), 36);
}

#[test]
fn test_migrations_idempotent() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    // Open twice - migrations should only run once
    let _db1 = Database::open(&db_path).unwrap();
    let db2 = Database::open(&db_path).unwrap();

    assert_eq!(db2.schema_version().unwrap(), 36);
}

#[test]
fn test_migration_035_drops_planner_telemetry() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    assert_eq!(db.schema_version().unwrap(), 36);

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
    assert_eq!(db.schema_version().unwrap(), 36);

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
