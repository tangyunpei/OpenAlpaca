use super::*;
use crate::Database;
use tempfile::tempdir;

fn setup_db() -> Database {
    let dir = tempdir().unwrap();
    Database::open(&dir.path().join("test.db")).unwrap()
}

fn make_skill_entry(request_id: &str, skill_id: &str, status: &str) -> SkillExecutionEntry {
    SkillExecutionEntry {
        id: None,
        request_id: request_id.to_string(),
        skill_id: skill_id.to_string(),
        agent_id: "orchestrator".to_string(),
        status: status.to_string(),
        finish_reason: Some("complete".to_string()),
        error_message: None,
        validation_failures: None,
        duration_ms: 1500,
        rounds_used: Some(3),
        tool_calls_made: Some(2),
        input_tokens: 100,
        output_tokens: 50,
        cost_usd: 0.01,
        model_used: Some("claude-sonnet".to_string()),
        query_preview: Some("test query".to_string()),
        route_score: Some(0.85),
        was_auto_selected: true,
        repair_attempted: false,
        repair_succeeded: false,
        timestamp: None,
    }
}

#[test]
fn test_record() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    let entry = make_skill_entry("req-1", "code-review", "success");
    let id = repo.record(&entry).unwrap();
    assert!(id > 0);

    let count: i64 = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM skill_execution_log WHERE skill_id = 'code-review'",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_record_tool() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    let entry = ToolExecutionEntry {
        request_id: Some("req-1".to_string()),
        agent_id: "orchestrator".to_string(),
        tool_name: "web_fetch".to_string(),
        success: true,
        duration_ms: 250,
        ..Default::default()
    };
    let id = repo.record_tool(&entry).unwrap();
    assert!(id > 0);
}

/// 039's six columns are the session event log's tool-call index (§5.4): the
/// payloads live in the JSONL, the row holds previews and the `log_seq`
/// pointer back to the record that carries them.
#[test]
fn record_tool_writes_the_session_index_columns() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    let id = repo
        .record_tool(&ToolExecutionEntry {
            agent_id: "research_agent::a1b2c3d4".to_string(),
            tool_name: "web_fetch".to_string(),
            success: true,
            duration_ms: 250,
            session_id: Some("sess-1".to_string()),
            task_id: Some("task-1".to_string()),
            log_seq: Some(184),
            args_preview: Some("{\"url\":\"https://example.com\"}".to_string()),
            result_preview: Some("hello".to_string()),
            result_ref: Some("log:185".to_string()),
            ..Default::default()
        })
        .unwrap();

    /// The six 039 columns, in the order the query below selects them.
    type IndexRow = (
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row: IndexRow = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT session_id, task_id, log_seq, args_preview, result_preview, result_ref
                   FROM tool_execution_log WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )?)
        })
        .unwrap();

    assert_eq!(row.0.as_deref(), Some("sess-1"));
    assert_eq!(row.1.as_deref(), Some("task-1"));
    assert_eq!(row.2, Some(184));
    assert_eq!(row.3.as_deref(), Some("{\"url\":\"https://example.com\"}"));
    assert_eq!(row.4.as_deref(), Some("hello"));
    assert_eq!(row.5.as_deref(), Some("log:185"));
}

/// The migration documents both preview columns as "≤ 2048 chars". The bound
/// is enforced where the column is written, so no caller can widen it — the
/// full payload is the JSONL's job, not the index row's.
#[test]
fn tool_previews_are_capped_at_the_column_bound() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    // Multi-byte, so a byte-wise cut would panic or split a character.
    let long: String = "é".repeat(PREVIEW_CHARS + 500);
    let id = repo
        .record_tool(&ToolExecutionEntry {
            agent_id: "orchestrator".to_string(),
            tool_name: "shell_execute".to_string(),
            args_preview: Some(long.clone()),
            result_preview: Some(long),
            ..Default::default()
        })
        .unwrap();

    let (args, result): (String, String) = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT args_preview, result_preview FROM tool_execution_log WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();

    assert_eq!(args.chars().count(), PREVIEW_CHARS);
    assert_eq!(result.chars().count(), PREVIEW_CHARS);
}

#[test]
fn test_cleanup_old() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    // Insert entries (they get current timestamp, so cleanup with 0 days should remove them)
    let entry = make_skill_entry("req-cleanup", "test-skill", "success");
    repo.record(&entry).unwrap();

    let tool_entry = ToolExecutionEntry {
        request_id: Some("req-cleanup".to_string()),
        agent_id: "orchestrator".to_string(),
        tool_name: "shell".to_string(),
        success: true,
        duration_ms: 100,
        ..Default::default()
    };
    repo.record_tool(&tool_entry).unwrap();

    // Cleanup with very large retention — nothing deleted
    let (s, t) = repo.cleanup_old(9999, 9999).unwrap();
    assert_eq!(s, 0);
    assert_eq!(t, 0);

    // Verify rows still exist
    let count: i64 = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM skill_execution_log WHERE skill_id = 'test-skill'",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_record_with_error() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    let mut entry = make_skill_entry("req-err", "failing-skill", "error");
    entry.finish_reason = Some("error".to_string());
    entry.error_message = Some("LLM timeout".to_string());
    entry.validation_failures = Some("[\"missing required section\"]".to_string());
    entry.repair_attempted = true;
    entry.repair_succeeded = false;

    let id = repo.record(&entry).unwrap();
    assert!(id > 0);

    let (status, error_message): (String, Option<String>) = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT status, error_message FROM skill_execution_log WHERE skill_id = 'failing-skill'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(status, "error");
    assert_eq!(error_message, Some("LLM timeout".to_string()));
}

#[test]
fn test_all_skill_health() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    // Insert a mix of outcomes for "code-review"
    for i in 0..5 {
        let mut entry = make_skill_entry(&format!("req-cr-{i}"), "code-review", "success");
        entry.finish_reason = Some("complete".to_string());
        repo.record(&entry).unwrap();
    }
    // One degraded
    let mut degraded = make_skill_entry("req-cr-deg", "code-review", "success");
    degraded.finish_reason = Some("max_rounds".to_string());
    repo.record(&degraded).unwrap();
    // One with repair
    let mut repaired = make_skill_entry("req-cr-rep", "code-review", "success");
    repaired.finish_reason = Some("complete".to_string());
    repaired.repair_attempted = true;
    repaired.repair_succeeded = true;
    repo.record(&repaired).unwrap();

    // Insert entries for "summarize"
    let entry = make_skill_entry("req-sum-1", "summarize", "success");
    repo.record(&entry).unwrap();

    let health = repo.all_skill_health().unwrap();
    assert_eq!(health.len(), 2);

    let cr = health.iter().find(|h| h.skill_id == "code-review").unwrap();
    assert_eq!(cr.total_invocations, 7);
    // 5 clean success out of 7 total
    assert!((cr.clean_success_rate - 5.0 / 7.0).abs() < 0.01);
    // 1 degraded out of 7
    assert!((cr.degraded_rate - 1.0 / 7.0).abs() < 0.01);
    // 1 repair attempted out of 7
    assert!((cr.repair_rate - 1.0 / 7.0).abs() < 0.01);
    // repair effectiveness: 1 succeeded / 1 attempted
    assert!((cr.repair_effectiveness - 1.0).abs() < 0.01);
    assert!(cr.last_invoked_at.is_some());
    // Feedback fields default
    assert!(cr.user_satisfaction_rate.is_none());
    assert_eq!(cr.feedback_count, 0);
}

#[test]
fn test_all_skill_health_empty() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    let health = repo.all_skill_health().unwrap();
    assert!(health.is_empty());
}

/// **`invocations_today` counts from an instant, not from a calendar day.**
///
/// `GET /v1/tools` passes local midnight already converted to UTC (the column
/// is `datetime('now')` text, i.e. UTC), so the predicate has to be a plain
/// text comparison against that instant: rows before it are excluded, rows at
/// or after it are counted, and the grouping is per tool name.
#[test]
fn test_tool_invocations_since_counts_from_the_instant() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    // Two rows either side of a known instant, plus one exactly on it and a
    // second tool, all with explicit UTC timestamps in the column's own format.
    let rows = [
        ("web_fetch", "2026-09-04 21:59:59"), // one second before — excluded
        ("web_fetch", "2026-09-04 22:00:00"), // exactly on it — counted
        ("web_fetch", "2026-09-05 03:14:00"),
        ("shell", "2026-09-04 12:00:00"), // yesterday — excluded
        ("shell", "2026-09-05 01:00:00"),
    ];
    db.with_connection(|conn| {
        for (tool, ts) in &rows {
            conn.execute(
                "INSERT INTO tool_execution_log (request_id, agent_id, tool_name, success, duration_ms, timestamp)
                 VALUES ('req', 'orchestrator', ?1, 1, 10, ?2)",
                rusqlite::params![tool, ts],
            )?;
        }
        Ok(())
    })
    .unwrap();

    let counts = repo
        .tool_invocations_since("2026-09-04 22:00:00")
        .unwrap();
    assert_eq!(
        counts.get("web_fetch"),
        Some(&2),
        "the row one second before the instant must not be counted: {counts:?}"
    );
    assert_eq!(
        counts.get("shell"),
        Some(&1),
        "counts are per tool name: {counts:?}"
    );

    // Nothing at all after the newest row: an empty map, not an error.
    let none = repo.tool_invocations_since("2026-09-06 00:00:00").unwrap();
    assert!(none.is_empty(), "{none:?}");
}

// ── R51: one row, two halves ────────────────────────────────────────

fn audit_half(request_id: &str, session_id: &str) -> ToolExecutionEntry {
    ToolExecutionEntry {
        request_id: Some(request_id.to_string()),
        agent_id: "research_agent::a1".to_string(),
        tool_name: "web_fetch".to_string(),
        success: true,
        duration_ms: 250,
        session_id: Some(session_id.to_string()),
        ..Default::default()
    }
}

fn index_half(request_id: &str, session_id: &str, log_seq: i64) -> ToolExecutionEntry {
    ToolExecutionEntry {
        request_id: Some(request_id.to_string()),
        agent_id: "research_agent::a1".to_string(),
        tool_name: "web_fetch".to_string(),
        success: true,
        duration_ms: 250,
        session_id: Some(session_id.to_string()),
        task_id: Some("task-1".to_string()),
        log_seq: Some(log_seq),
        args_preview: Some(r#"{"url":"https://example.com"}"#.to_string()),
        result_preview: Some("the page body".to_string()),
        result_ref: Some(format!("log:{}", log_seq + 1)),
        ..Default::default()
    }
}

fn rows(db: &Database) -> i64 {
    db.with_connection(|conn| {
        Ok(conn.query_row("SELECT COUNT(*) FROM tool_execution_log", [], |r| r.get(0))?)
    })
    .unwrap()
}

/// R51: the daemon's audit insert and the session writer's index update are
/// two halves of **one** row. The writer merges onto the row the daemon
/// already wrote, matched by the call's tool-use id, and adds only its own
/// columns.
#[test]
fn the_index_update_lands_log_seq_on_the_daemons_audit_row() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    repo.record_tool(&audit_half("toolu_1", "sess-1")).unwrap();
    repo.attach_session_index(&index_half("toolu_1", "sess-1", 184))
        .unwrap();

    assert_eq!(rows(&db), 1, "one row, not two");
    let (log_seq, task, result_ref, duration, agent): (i64, String, String, i64, String) = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT log_seq, task_id, result_ref, duration_ms, agent_id
                   FROM tool_execution_log",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )?)
        })
        .unwrap();
    assert_eq!(log_seq, 184, "the update lands log_seq on the audit row");
    assert_eq!(task, "task-1");
    assert_eq!(result_ref, "log:185");
    assert_eq!(duration, 250, "the audit half is left alone");
    assert_eq!(agent, "research_agent::a1");
}

/// The reverse order — the writer's record beats the daemon's event to the
/// table — must not produce a second row and must not clobber the `log_seq`
/// that is already there.
#[test]
fn an_audit_insert_after_the_index_row_merges_instead_of_clobbering() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    repo.attach_session_index(&index_half("toolu_2", "sess-1", 12))
        .unwrap();
    let mut late = audit_half("toolu_2", "sess-1");
    late.success = false;
    late.duration_ms = 999;
    repo.record_tool(&late).unwrap();

    assert_eq!(rows(&db), 1, "the late audit insert merges, it does not add");
    let (log_seq, success, duration): (i64, i64, i64) = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT log_seq, success, duration_ms FROM tool_execution_log",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .unwrap();
    assert_eq!(log_seq, 12, "the writer's log_seq survives the audit insert");
    assert_eq!(success, 0, "the audit half is authoritative for the outcome");
    assert_eq!(duration, 999);
}

/// The merge is keyed on the call, not on the session: two calls are two
/// rows, so `invocations_today` still counts what happened.
#[test]
fn two_calls_in_one_session_stay_two_rows() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    for (id, seq) in [("toolu_a", 3), ("toolu_b", 5)] {
        repo.record_tool(&audit_half(id, "sess-1")).unwrap();
        repo.attach_session_index(&index_half(id, "sess-1", seq))
            .unwrap();
    }
    assert_eq!(rows(&db), 2);
}

/// A call the daemon never reported (its event was lost, or it never
/// executed) still gets the writer's row: the index is not silently empty.
#[test]
fn an_index_row_with_no_audit_row_is_inserted_whole() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    repo.attach_session_index(&index_half("toolu_lonely", "sess-1", 7))
        .unwrap();
    assert_eq!(rows(&db), 1);
    let (tool, log_seq): (String, i64) = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT tool_name, log_seq FROM tool_execution_log",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(tool, "web_fetch");
    assert_eq!(log_seq, 7);
}

/// `error_message` is clamped where the column is written, like both
/// previews: the loop now hands the writer the untruncated error text.
#[test]
fn the_error_message_column_is_clamped_like_the_previews() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    repo.record_tool(&ToolExecutionEntry {
        agent_id: "a".to_string(),
        tool_name: "shell_execute".to_string(),
        success: false,
        duration_ms: 1,
        error_message: Some("e".repeat(40 * 1024)),
        ..Default::default()
    })
    .unwrap();

    let stored: String = db
        .with_connection(|conn| {
            Ok(conn.query_row("SELECT error_message FROM tool_execution_log", [], |r| {
                r.get(0)
            })?)
        })
        .unwrap();
    assert_eq!(stored.chars().count(), PREVIEW_CHARS);
}

/// T42 re-review, Minor 2: when the boot sweep evicts a session's log, the
/// rows that indexed it must stop describing records that no longer exist —
/// and must keep describing the calls that really happened.
#[test]
fn clearing_a_sessions_index_drops_the_pointers_and_keeps_the_audit() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);
    for (request, seq, session) in [("toolu_1", 4, "gone"), ("toolu_2", 9, "gone"), ("toolu_3", 2, "kept")] {
        repo.attach_session_index(&ToolExecutionEntry {
            request_id: Some(request.to_string()),
            agent_id: "lead_agent".to_string(),
            tool_name: "shell_execute".to_string(),
            success: true,
            duration_ms: 5,
            session_id: Some(session.to_string()),
            log_seq: Some(seq),
            args_preview: Some("{}".to_string()),
            result_preview: Some("ok".to_string()),
            result_ref: Some(format!("log:{seq}")),
            ..Default::default()
        })
        .unwrap();
    }

    assert_eq!(repo.clear_session_log_index("gone").unwrap(), 2);

    /// What the assertions below need out of one row.
    struct IndexedRow {
        log_seq: Option<i64>,
        result_ref: Option<String>,
        tool_name: String,
        result_preview: Option<String>,
    }
    let read = |session: &str| -> Vec<IndexedRow> {
        db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT log_seq, result_ref, tool_name, result_preview \
                 FROM tool_execution_log WHERE session_id = ?1 ORDER BY id",
            )?;
            let rows = stmt
                .query_map([session], |r| {
                    Ok(IndexedRow {
                        log_seq: r.get(0)?,
                        result_ref: r.get(1)?,
                        tool_name: r.get(2)?,
                        result_preview: r.get(3)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .unwrap()
    };

    for row in read("gone") {
        assert_eq!(row.log_seq, None);
        assert_eq!(row.result_ref, None);
        // The call still happened; `invocations_today` must not move.
        assert_eq!(row.tool_name, "shell_execute");
        assert_eq!(row.result_preview.as_deref(), Some("ok"));
    }
    assert_eq!(
        read("kept")[0].log_seq,
        Some(2),
        "another session is untouched"
    );

    // Idempotent: a second pass finds nothing left to clear.
    assert_eq!(repo.clear_session_log_index("gone").unwrap(), 0);
}

/// The skill half of the same instant-based count (`GET /v1/skills`'
/// `invocations_today`). `skill_execution_log.timestamp` is the same
/// `datetime('now')` UTC text as the tool log's, so the predicate is the same
/// plain text comparison and the grouping is per `skill_id`.
#[test]
fn test_skill_invocations_since_counts_from_the_instant() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    let rows = [
        ("daily-digest", "2026-09-04 21:59:59"), // one second before — excluded
        ("daily-digest", "2026-09-04 22:00:00"), // exactly on it — counted
        ("daily-digest", "2026-09-05 03:14:00"),
        ("code-review", "2026-09-04 12:00:00"), // yesterday — excluded
        ("code-review", "2026-09-05 01:00:00"),
    ];
    db.with_connection(|conn| {
        for (i, (skill, ts)) in rows.iter().enumerate() {
            conn.execute(
                "INSERT INTO skill_execution_log
                   (request_id, skill_id, status, duration_ms, timestamp)
                 VALUES (?1, ?2, 'success', 10, ?3)",
                rusqlite::params![format!("req-{i}"), skill, ts],
            )?;
        }
        Ok(())
    })
    .unwrap();

    let counts = repo.skill_invocations_since("2026-09-04 22:00:00").unwrap();
    assert_eq!(
        counts.get("daily-digest"),
        Some(&2),
        "the row one second before the instant must not be counted: {counts:?}"
    );
    assert_eq!(
        counts.get("code-review"),
        Some(&1),
        "counts are per skill id: {counts:?}"
    );

    let none = repo.skill_invocations_since("2026-09-06 00:00:00").unwrap();
    assert!(none.is_empty(), "{none:?}");
}

// ── resolve_skill_key — the one copy of the id-then-name rule ─────────────

/// `(id, frontmatter name)` pairs, the shape every caller can produce.
const CATALOG: &[(&str, &str)] = &[
    ("daily-digest", "Daily Digest"),
    ("code-review", "Code Review"),
];

#[test]
fn resolve_skill_key_hits_the_id() {
    assert_eq!(
        resolve_skill_key("Daily-Digest", CATALOG.iter().copied()),
        Some("daily-digest"),
        "a logged key that is already a catalog id resolves to itself, case-folded"
    );
}

#[test]
fn resolve_skill_key_hits_the_frontmatter_name() {
    // What every live writer actually logs: the display name, spaces and all.
    assert_eq!(
        resolve_skill_key("Daily Digest", CATALOG.iter().copied()),
        Some("daily-digest"),
        "the frontmatter name resolves onto the catalog id"
    );
    assert_eq!(
        resolve_skill_key("DAILY DIGEST", CATALOG.iter().copied()),
        Some("daily-digest"),
        "case-folded on both sides"
    );
    assert_eq!(
        resolve_skill_key("Daily", CATALOG.iter().copied()),
        None,
        "a name is matched whole, never by prefix"
    );
}

#[test]
fn resolve_skill_key_misses_a_key_no_entry_claims() {
    assert_eq!(
        resolve_skill_key("deleted-skill", CATALOG.iter().copied()),
        None,
        "a key the catalog does not claim belongs to nobody — never to an arbitrary row"
    );
    assert_eq!(resolve_skill_key("", CATALOG.iter().copied()), None);
    assert_eq!(
        resolve_skill_key("daily-digest", std::iter::empty()),
        None,
        "an empty catalog resolves nothing"
    );
}

#[test]
fn resolve_skill_key_prefers_an_id_over_another_entrys_name() {
    // One entry's id is another's frontmatter name. Ids win, wherever the two
    // sit in the iteration order — the name arm is checked only after the whole
    // catalog has failed to match on id.
    let forward = [("shipping", "Fulfilment"), ("orders", "shipping")];
    let reversed = [("orders", "shipping"), ("shipping", "Fulfilment")];
    assert_eq!(resolve_skill_key("shipping", forward.iter().copied()), Some("shipping"));
    assert_eq!(resolve_skill_key("shipping", reversed.iter().copied()), Some("shipping"));
}

#[test]
fn resolve_skill_key_breaks_a_name_collision_on_the_lowest_id() {
    // Two entries can share a frontmatter name — across scopes, or between a
    // file skill and a plugin one. The count has to land somewhere; it must at
    // least land in the *same* place on two reads, whatever order the caller's
    // `HashMap` hands the entries over in.
    let one = [("a-digest", "Digest"), ("z-digest", "Digest")];
    let other = [("z-digest", "Digest"), ("a-digest", "Digest")];
    assert_eq!(resolve_skill_key("digest", one.iter().copied()), Some("a-digest"));
    assert_eq!(resolve_skill_key("digest", other.iter().copied()), Some("a-digest"));
}
