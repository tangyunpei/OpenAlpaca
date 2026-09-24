use super::*;
use tempfile::tempdir;

#[test]
fn test_database_creation() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let db = Database::open(&db_path).unwrap();
    assert!(db_path.exists());
    assert_eq!(db.schema_version().unwrap(), newest_version());
}

/// The version a fresh database opens at: the newest registered migration.
/// `BASELINE_VERSION` keeps meaning "the oldest openable schema" (design 09
/// §8.9), so a fresh database's version is read from here, never from it.
fn newest_version() -> i32 {
    migrations::MIGRATIONS
        .last()
        .expect("at least the baseline is registered")
        .version
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
    assert_eq!(db.schema_version().unwrap(), newest_version());
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
        let registered: Vec<i32> = migrations::MIGRATIONS.iter().map(|m| m.version).collect();
        assert_eq!(versions, registered);
        Ok(())
    })
    .unwrap();
}

/// The legacy guard covers every version in `1..BASELINE_VERSION`, with two
/// wordings: 42 — the last schema before the memory redesign — is refused
/// because its memory storage was replaced (plan §4, the 42 arm), everything
/// older because the baseline cannot upgrade a partly migrated development
/// database (the 1–41 arm, unchanged). Both name the same file, the same
/// version this build starts at and the same verb.
#[test]
fn an_unsupported_old_database_is_refused_without_changing_its_schema_or_data() {
    let dir = tempdir().unwrap();
    assert_eq!(
        migrations::BASELINE_VERSION,
        43,
        "the two wordings below are pinned to the memory redesign's re-squash"
    );
    for version in [1, 41, 42] {
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
        // The remedy deletes the file, so the refusal has to say which file.
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
        // Guard and verb are pinned together on purpose: this refusal is the
        // only place a user is told how to get out of it, and the verb it names
        // is the only form of `openalpaca config` that opens no database
        // (`apps/openalpaca/src/commands/config_factory_reset.rs`). If that verb
        // is ever renamed or made to open the database again, this line fails
        // before the manuals go stale.
        assert!(
            message.contains("openalpaca config reset --factory"),
            "the refusal must name the verb that rescues the database: {message}"
        );
        // Start-up refusals never name each other's remedies: the newer-build
        // sentence belongs to a different file and a different mistake.
        assert!(
            !message.contains("newer than this build knows"),
            "the legacy refusal must not borrow the newer-database wording: {message}"
        );
        if version == 42 {
            // The memory-redesign arm (plan §4): the reason, everything the
            // factory reset removes — the API keys included — what survives,
            // and that an older build still opens the file.
            for pin in [
                "cannot read a database from before the memory redesign",
                "its memory storage was replaced",
                "-wal and -shm",
                "every task, message, session record and memory it holds",
                "`llm.toml` with its API keys and keychain entries",
                "returns `daemon.toml` to defaults",
                "artifact, upload and session files stay on disk",
                "`.master_key`, `logs/` and `backups/` are untouched",
                "with an OpenAlpaca build from before the memory redesign",
            ] {
                assert!(
                    message.contains(pin),
                    "the version-42 refusal must say {pin:?}: {message}"
                );
            }
            assert!(
                !message.contains("cannot upgrade a database created by an older one"),
                "a version-42 database is not a partly migrated one: {message}"
            );
        } else {
            // The 1–41 arm keeps its wording verbatim: a partly migrated
            // development database the baseline cannot upgrade.
            assert!(
                message.contains("cannot upgrade a database created by an older one"),
                "the version-{version} refusal keeps the legacy wording: {message}"
            );
            assert!(
                !message.contains("memory redesign"),
                "a version-{version} database is refused for the old reason, not \
                 because the memory storage was replaced: {message}"
            );
        }

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
fn a_database_newer_than_this_build_is_refused_without_changing_its_schema_or_data() {
    let dir = tempdir().unwrap();
    let newest = migrations::MIGRATIONS.last().unwrap().version;
    let version = 99;
    assert!(
        version > newest,
        "the fixture must be newer than every registered migration"
    );
    let db_path = dir.path().join("future.db");
    let original_schema = {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             CREATE TABLE future_data (id INTEGER PRIMARY KEY, content TEXT NOT NULL);
             INSERT INTO future_data (id, content) VALUES (7, 'Do not discard');",
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
        .unwrap_or_else(|| panic!("schema version {version} must be refused"));
    let message = format!("{error:#}");
    // The whole sentence is pinned: it names the file by its full path (the
    // store root is overridable, so nothing else identifies it), the version
    // the file is at, and the newest version this build knows — and nothing
    // else. A newer store is one a newer build wrote; the only remedy is that
    // build, and start-up refusals never name each other's remedies.
    let named = std::path::absolute(&db_path).unwrap();
    assert_eq!(
        message,
        format!(
            "Database {} is at schema version {version}, which is newer than this build \
             knows ({newest}). Use the OpenAlpaca build that created it. Nothing was changed.",
            named.display()
        ),
        "opening a newer database must refuse it with exactly this text"
    );
    // Pinned on its own as well, so the rule survives a rewording of the
    // sentence: a user who followed the legacy refusal's advice against this
    // file would delete a store that nothing is wrong with.
    assert!(
        !message.contains("reset --factory"),
        "the refusal must not send the user to the factory reset: {message}"
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
        .prepare("SELECT id, content FROM future_data ORDER BY id")
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
        "refusal must not touch the version"
    );
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

/// The memory redesign's schema (plan 3.5): a fresh database gets the 43
/// baseline directly, and that baseline creates none of the old engine's
/// tables — `memory`, its FTS index, the `vec0` table or the triggers that
/// kept them in step — so opening one never needs the sqlite-vec extension.
/// The migrations after the baseline create none either: 044's
/// `learning_cursor` is named without the `memory_` prefix for exactly this
/// check (design 09 §1.3).
#[test]
fn a_fresh_database_opens_at_the_newest_version_with_no_memory_tables() {
    assert_eq!(migrations::BASELINE_VERSION, 43);
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    assert_eq!(db.schema_version().unwrap(), newest_version());
    db.with_connection(|conn| {
        let leftovers = conn
            .prepare(
                "SELECT type || ':' || name FROM sqlite_master
                  WHERE name = 'memory' OR name LIKE 'memory!_%' ESCAPE '!'
                  ORDER BY 1",
            )?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        assert!(
            leftovers.is_empty(),
            "the baseline must create no memory table, index or trigger: {leftovers:?}"
        );
        // The rest of the schema is still there.
        let sessions: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'session'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(sessions, 1);
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

// ---------------------------------------------------------------------------
// `delete_database_files` — the file set a factory reset removes.
// ---------------------------------------------------------------------------

#[test]
fn deleting_a_database_takes_its_wal_and_shm_with_it() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("x.db");
    drop(Database::open(&db_path).unwrap());
    // A clean close checkpoints the sidecars away; the point under test is the
    // file set, not SQLite's behaviour, so they are written by hand.
    let wal = dir.path().join("x.db-wal");
    let shm = dir.path().join("x.db-shm");
    std::fs::write(&wal, b"stale").unwrap();
    std::fs::write(&shm, b"stale").unwrap();
    let keep = dir.path().join("keep.txt");
    std::fs::write(&keep, b"mine").unwrap();

    delete_database_files(&db_path).unwrap();

    assert!(!db_path.exists(), "the database file must be gone");
    assert!(!wal.exists(), "the -wal sidecar must be gone");
    assert!(!shm.exists(), "the -shm sidecar must be gone");
    assert_eq!(
        std::fs::read(&keep).unwrap(),
        b"mine",
        "a file that is not part of the database must be left alone"
    );
}

#[test]
fn deleting_a_database_that_is_not_there_succeeds() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("absent.db");

    delete_database_files(&db_path).unwrap();

    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert!(
        left.is_empty(),
        "deleting a missing database must create nothing: {left:?}"
    );
}

#[test]
fn a_refused_legacy_database_opens_at_the_baseline_after_its_files_are_deleted() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("openalpaca.db");
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             CREATE TABLE legacy_data (id INTEGER PRIMARY KEY, content TEXT NOT NULL);
             INSERT INTO legacy_data (id, content) VALUES (7, 'about to go');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO schema_version (version) VALUES (?1)",
            [migrations::BASELINE_VERSION - 1],
        )
        .unwrap();
    }
    std::fs::write(dir.path().join("openalpaca.db-wal"), b"stale").unwrap();
    std::fs::write(dir.path().join("openalpaca.db-shm"), b"stale").unwrap();
    assert!(
        Database::open(&db_path).is_err(),
        "the version-{} database must be refused before the delete",
        migrations::BASELINE_VERSION - 1
    );

    delete_database_files(&db_path).unwrap();

    let db = Database::open(&db_path).expect("a deleted database must open fresh");
    assert_eq!(db.schema_version().unwrap(), newest_version());
    let legacy_tables: i64 = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'legacy_data'",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(
        legacy_tables, 0,
        "the legacy rows must not survive the delete"
    );
}

// ---------------------------------------------------------------------------
// The schema-42 fixture — `tests/fixtures/schema42.db`.
// ---------------------------------------------------------------------------
//
// A real schema-42 database — made by `Database::open` on the 42 baseline as
// it shipped (WAL mode, sqlite-vec linked), with `memory_vec` and its vec0
// shadow tables holding data — not a hand-written schema. It was generated
// once, by the `#[ignore]`d `generate_schema42_fixture` test that commit
// `2f500ea1` (slice 1) carried, and committed with the file. That generator
// went with the engine it used (plan 3.5): this build's baseline is 43, it
// declares no `memory_vec USING vec0`, and it links no sqlite-vec, so a
// schema-42 database can no longer be *produced* here — which is exactly why
// the file is committed. Its SHA-256 is
// `29e111ce64b2eabd8bcd6bac01e2a3f0232f0f6e2469103d040d5d08c917f6b2`.

/// The fixture's path, relative to the crate root.
const SCHEMA42_FIXTURE: &str = "tests/fixtures/schema42.db";

/// What the generator wrote, so the refusal test can prove the rows are still
/// there afterwards: seven `memory` rows across two owners and both scopes
/// (one CJK, one supersession pair), six of them embedded — `supersede`
/// deleted the superseded row's vector — plus one task and one chat message.
const FIXTURE_MEMORY_ROWS: i64 = 7;
const FIXTURE_EMBEDDED_ROWS: i64 = 6;

fn schema42_fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(SCHEMA42_FIXTURE)
}

fn sha256_hex(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
}

/// Plan §4 (MD-6) and fixture 05 of the invariant set: any opener meeting a
/// schema-42 database is refused with the memory-redesign wording, before any
/// statement touches a memory table, with `vec0` **not** registered on the
/// connection, and the file's bytes are unchanged. Slice 1's sanity test
/// opened the same copy and expected it to work; this is that test inverted.
#[test]
fn the_schema42_fixture_is_refused_without_vec0_and_its_bytes_are_unchanged() {
    let dir = tempdir().unwrap();
    let copy = dir.path().join("schema42.db");
    std::fs::copy(schema42_fixture_path(), &copy).unwrap();
    let before = sha256_hex(&copy);

    let error = Database::open(&copy)
        .err()
        .expect("a schema-42 database must be refused by the 43 baseline");
    let message = format!("{error:#}");
    // The pinned phrases of the 42 arm (fixture 05's `message_contains`).
    for pin in [
        "Unsupported legacy schema version 42",
        "schema version 43",
        "cannot read a database from before the memory redesign",
        "its memory storage was replaced",
        "openalpaca config reset --factory",
        "-wal and -shm",
        "llm.toml",
        "artifact, upload and session files stay on disk",
        ".master_key",
        "with an OpenAlpaca build from before the memory redesign",
    ] {
        assert!(
            message.contains(pin),
            "the refusal must say {pin:?}: {message}"
        );
    }
    assert!(
        !message.contains("newer than this build knows"),
        "the refusal must not borrow the newer-database wording: {message}"
    );
    let named = std::path::absolute(&copy).unwrap();
    assert!(
        message.contains(&named.display().to_string()),
        "the refusal must name the file by its full path: {message}"
    );

    // `vec0` is not registered on any connection this process opens: the
    // extension's function is unknown and its module cannot be loaded. The
    // refusal read only `schema_version`, which is why it worked anyway.
    let conn = Connection::open(&copy).unwrap();
    let vec_fn = conn
        .query_row("SELECT vec_version()", [], |row| row.get::<_, String>(0))
        .expect_err("vec_version() must be unknown without sqlite-vec");
    assert!(
        vec_fn.to_string().contains("no such function"),
        "unexpected error for vec_version(): {vec_fn}"
    );
    let vec_table = conn
        .query_row("SELECT COUNT(*) FROM memory_vec", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect_err("memory_vec must be unreadable without the vec0 module");
    assert!(
        vec_table.to_string().contains("no such module: vec0"),
        "unexpected error for memory_vec: {vec_table}"
    );
    // The rows a pre-redesign build would read are all still there: the
    // plain tables, the FTS index and the vec0 shadow tables the module owns.
    let count = |sql: &str| conn.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
    assert_eq!(count("SELECT COUNT(*) FROM memory"), FIXTURE_MEMORY_ROWS);
    assert_eq!(count("SELECT COUNT(DISTINCT owner_id) FROM memory"), 2);
    assert_eq!(
        count("SELECT COUNT(*) FROM memory WHERE supersedes_id IS NOT NULL"),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM memory_fts WHERE memory_fts MATCH 'vim'"),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM memory_vec_rowids"),
        FIXTURE_EMBEDDED_ROWS
    );
    assert_eq!(count("SELECT COUNT(*) FROM task"), 1);
    assert_eq!(count("SELECT COUNT(*) FROM conversation_messages"), 1);
    let versions = conn
        .prepare("SELECT version FROM schema_version ORDER BY version")
        .unwrap()
        .query_map([], |row| row.get::<_, i32>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        versions,
        vec![42],
        "the refusal must not advance the version"
    );
    drop(conn);

    // A second open is refused the same way (fixture 05, step 2): nothing the
    // first attempt did changed the answer.
    let again = Database::open(&copy)
        .err()
        .expect("the second open must be refused too");
    assert!(format!("{again:#}").contains("Unsupported legacy schema version 42"));

    // The refusal ran no migration and wrote nothing, so the file is
    // byte-identical; the -wal and -shm SQLite opens beside the copy are
    // outside the hash.
    assert_eq!(
        sha256_hex(&copy),
        before,
        "refusing the fixture must not rewrite it"
    );
}

// ── Migration 044: Learning v2's cursor (design 09 §1.3) ────────────────

/// A 43 database with sessions and messages opens at 44: each existing
/// session's cursor starts at its newest message — historic sessions are not
/// re-extracted (design §6) — a session with no message starts at 0, the two
/// new columns exist and read NULL on the old rows, and no row is lost.
#[test]
fn migration_044_seeds_existing_sessions_at_their_last_message() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("at43.db");
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(migrations::MIGRATIONS[0].sql).unwrap();
        assert_eq!(migrations::MIGRATIONS[0].version, 43);
        conn.execute_batch(
            "INSERT INTO session (id, lane_key, source) VALUES ('talked', 'u:gui', 'gui');
             INSERT INTO session (id, lane_key, source, status) VALUES ('silent', 'u:cli', 'cli', 'archived');
             INSERT INTO conversation_messages (lane_key, role, content, session_id)
                 VALUES ('u:gui', 'user', 'one', 'talked');
             INSERT INTO conversation_messages (lane_key, role, content, session_id)
                 VALUES ('u:gui', 'assistant', 'two', 'talked');
             INSERT INTO conversation_messages (lane_key, role, content, session_id)
                 VALUES ('u:gui', 'user', 'three', 'talked');
             INSERT INTO conversation_messages (lane_key, role, content)
                 VALUES ('u:gui', 'user', 'sessionless');",
        )
        .unwrap();
    }

    let db = Database::open(&db_path).expect("a 43 database migrates in place");
    assert_eq!(db.schema_version().unwrap(), 44);
    db.with_connection(|conn| {
        let newest: i64 = conn.query_row(
            "SELECT MAX(id) FROM conversation_messages WHERE session_id = 'talked'",
            [],
            |row| row.get(0),
        )?;
        let cursors = conn
            .prepare(
                "SELECT session_id, extracted_through, attempts, last_error
                   FROM learning_cursor ORDER BY session_id",
            )?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        assert_eq!(
            cursors,
            vec![
                ("silent".to_string(), 0, 0, None),
                ("talked".to_string(), newest, 0, None),
            ]
        );

        let (rows, stamped): (i64, i64) = conn.query_row(
            "SELECT COUNT(*), COUNT(request_id) + COUNT(workspace_root) FROM conversation_messages",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(rows, 4, "no message is lost");
        assert_eq!(stamped, 0, "the old rows carry no turn id and no root");

        let versions = conn
            .prepare("SELECT version FROM schema_version ORDER BY version")?
            .query_map([], |row| row.get::<_, i32>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        assert_eq!(versions, vec![43, 44]);
        Ok(())
    })
    .unwrap();
}
