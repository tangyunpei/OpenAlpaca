//! Writer-side tests for the session event log (§5.4).

use super::*;
use openalpaca_storage::Database;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

fn service(dir: &TempDir) -> SessionLogService {
    SessionLogService::new(
        dir.path().to_path_buf(),
        None,
        SessionLogLimits::default(),
        "test".to_string(),
    )
}

fn service_with(dir: &TempDir, db: Option<Database>, limits: SessionLogLimits) -> SessionLogService {
    SessionLogService::new(dir.path().to_path_buf(), db, limits, "test".to_string())
}

fn log_path(root: &Path, session: &str) -> PathBuf {
    root.join(session).join(LIVE_SEGMENT)
}

fn lines(path: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("every line is one JSON object"))
        .collect()
}

// ── Envelope ────────────────────────────────────────────────────────

/// §5.4's envelope: one JSON object per line, `v`/`seq`/`ts`/`type`, a
/// per-session `seq` that is strictly monotonic and gap-free, and `task_id`
/// present on workflow-interior records and absent on main-loop ones.
#[tokio::test]
async fn records_are_one_json_object_per_line_with_a_gap_free_seq() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let handle = svc.handle_for("sess-1");

    handle.emit(Record::new(RecordType::UserMsg).with_data(serde_json::json!({"msg_id": 1})));
    handle.emit(
        Record::new(RecordType::Round)
            .task(Some("task-1"))
            .span(Some("lead::task-1"))
            .agent(Some("lead_agent::a1"))
            .with_data(serde_json::json!({"round": 1})),
    );
    handle.emit(Record::new(RecordType::WorkflowDone).with_data(serde_json::json!({"ok": true})));
    assert!(handle.flush().await);

    let rows = lines(&log_path(dir.path(), "sess-1"));
    assert_eq!(rows.len(), 3, "{rows:?}");
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row["v"], 1);
        assert_eq!(row["seq"], (i + 1) as u64, "seq is gap-free from 1");
        assert!(row["ts"].as_str().unwrap().ends_with('Z'), "{row}");
        assert!(row["data"].is_object());
    }
    assert_eq!(rows[0]["type"], "user_msg");
    assert!(
        rows[0].get("task_id").is_none(),
        "a main-loop record carries no task_id: {}",
        rows[0]
    );
    assert_eq!(rows[1]["type"], "round");
    assert_eq!(rows[1]["task_id"], "task-1");
    assert_eq!(rows[1]["span_id"], "lead::task-1");
    assert_eq!(rows[1]["agent"], "lead_agent::a1");
}

/// Every `RecordType` serialises as the catalog name `as_str()` reports —
/// the two must not drift, because the reader matches on the string.
#[test]
fn record_type_strings_match_their_serialised_names() {
    for kind in RecordType::ALL {
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, format!("\"{}\"", kind.as_str()), "{kind:?}");
        assert_eq!(RecordType::parse(kind.as_str()), Some(kind));
    }
}

/// §5.4 caps `data` at 64 KB. Until the `results/` spill lands (T42) an
/// over-cap payload is truncated in place and flagged `spilled_pending`, so
/// the sites that must spill are findable.
#[tokio::test]
async fn the_envelope_caps_data_and_flags_the_pending_spill() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let handle = svc.handle_for("sess-cap");

    let huge = "x".repeat(200 * 1024);
    handle.emit(Record::new(RecordType::ToolResult).with_data(serde_json::json!({
        "tool_use_id": "tu-1",
        "name": "shell_execute",
        "ok": true,
        "result": huge,
    })));
    assert!(handle.flush().await);

    let rows = lines(&log_path(dir.path(), "sess-cap"));
    assert_eq!(rows.len(), 1);
    let data = &rows[0]["data"];
    let serialized = serde_json::to_string(data).unwrap();
    assert!(
        serialized.len() <= ENVELOPE_DATA_CAP_BYTES,
        "data must fit the 64 KB envelope bound, got {}",
        serialized.len()
    );
    assert_eq!(data["_truncated"]["spilled_pending"], true);
    assert_eq!(data["_truncated"]["fields"][0], "result");
    assert!(data["_truncated"]["original_bytes"].as_u64().unwrap() > 200_000);
    // The scalar fields survive — only the oversized string is cut.
    assert_eq!(data["tool_use_id"], "tu-1");
    assert_eq!(data["name"], "shell_execute");
    assert!(data["result"].as_str().unwrap().starts_with(&"x".repeat(64)));
    assert!(data["result"].as_str().unwrap().contains("spill pending"));
}

/// A payload with no oversized string field of its own still cannot exceed
/// the bound: the whole object is replaced by the stub.
#[tokio::test]
async fn an_over_cap_payload_with_no_single_big_field_becomes_a_stub() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let handle = svc.handle_for("sess-stub");

    let many: Vec<String> = (0..40_000).map(|i| format!("row-{i}")).collect();
    handle.emit(
        Record::new(RecordType::Round).with_data(serde_json::json!({ "tool_use": many })),
    );
    assert!(handle.flush().await);

    let rows = lines(&log_path(dir.path(), "sess-stub"));
    let data = &rows[0]["data"];
    assert!(serde_json::to_string(data).unwrap().len() <= ENVELOPE_DATA_CAP_BYTES);
    assert_eq!(data["_truncated"]["spilled_pending"], true);
    assert!(data["preview"].as_str().unwrap().chars().count() <= PREVIEW_CHARS);
}

// ── Durability ──────────────────────────────────────────────────────

/// §5.4: "the writer truncates a torn tail before appending on reopen", and
/// the seq continues from the last **complete** record — never re-using one.
#[tokio::test]
async fn a_torn_tail_is_truncated_before_the_next_append() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path().join("sess-torn");
    fs::create_dir_all(&session_dir).unwrap();
    let path = session_dir.join(LIVE_SEGMENT);
    {
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"v":1,"seq":1,"ts":"2026-09-05T10:00:00.000Z","type":"session_start","data":{{}}}}"#).unwrap();
        writeln!(f, r#"{{"v":1,"seq":2,"ts":"2026-09-05T10:00:01.000Z","type":"user_msg","data":{{}}}}"#).unwrap();
        // A `kill -9` mid-write: the last line has no newline and no closing brace.
        write!(f, r#"{{"v":1,"seq":3,"ts":"2026-09-05T10:00:02.000Z","type":"rou"#).unwrap();
    }

    let svc = service(&dir);
    let handle = svc.handle_for("sess-torn");
    handle.emit(Record::new(RecordType::AssistantMsg).with_data(serde_json::json!({"msg_id": 9})));
    assert!(handle.flush().await);

    let rows = lines(&path);
    assert_eq!(rows.len(), 3, "the torn line is gone, the new one appended");
    assert_eq!(rows[2]["seq"], 3, "seq continues from the last complete record");
    assert_eq!(rows[2]["type"], "assistant_msg");
}

/// A reader treats an unparseable final line as end-of-log rather than an
/// error — the crash-recovery contract T42's `/events` reader inherits.
#[test]
fn a_reader_treats_an_unparseable_final_line_as_end_of_log() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path().join("sess-read");
    fs::create_dir_all(&session_dir).unwrap();
    let path = session_dir.join(LIVE_SEGMENT);
    let mut f = fs::File::create(&path).unwrap();
    writeln!(f, r#"{{"v":1,"seq":1,"ts":"2026-09-05T10:00:00.000Z","type":"session_start","data":{{}}}}"#).unwrap();
    writeln!(f, r#"{{"v":1,"seq":2,"ts":"2026-09-05T10:00:01.000Z","type":"round","data":{{"round":1}}}}"#).unwrap();
    write!(f, r#"{{"v":1,"seq":3,"ts":"2026-09-0"#).unwrap();
    drop(f);

    let records = read_records(&session_dir).expect("a torn log is readable, not an error");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].kind, "session_start");
    assert_eq!(records[1].seq, 2);
    assert_eq!(records[1].data["round"], 1);

    // The cursor form T42 pages with.
    let after = read_records_after(&session_dir, Some(1), 10).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].seq, 2);
}

/// The per-session directory is created by the writer's first record, never
/// by asking for a handle (P-22 — Claude Code's 334 empty session dirs).
#[tokio::test]
async fn the_session_directory_is_created_by_the_first_record_not_the_handle() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let handle = svc.handle_for("sess-lazy");
    // Give the writer task a chance to run before checking.
    assert!(handle.flush().await);
    assert!(
        !dir.path().join("sess-lazy").exists(),
        "a handle alone must not create a directory"
    );

    handle.emit(Record::new(RecordType::UserMsg).with_data(serde_json::json!({})));
    assert!(handle.flush().await);
    assert!(dir.path().join("sess-lazy").join(LIVE_SEGMENT).exists());
}

/// `session_start` is a boot boundary (P-13): once per boot per session, no
/// matter how many callers open the session.
#[tokio::test]
async fn session_start_is_written_once_per_boot() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);

    let a = svc.open("sess-boot", Some("u:gui"), Some("gui"), Some("/repo"));
    let b = svc.open("sess-boot", Some("u:gui"), Some("gui"), Some("/repo"));
    b.emit(Record::new(RecordType::UserMsg).with_data(serde_json::json!({})));
    assert!(a.flush().await);

    let rows = lines(&log_path(dir.path(), "sess-boot"));
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0]["type"], "session_start");
    assert_eq!(rows[0]["data"]["lane_key"], "u:gui");
    assert_eq!(rows[0]["data"]["source"], "gui");
    assert_eq!(rows[0]["data"]["workspace_id"], "/repo");
    assert_eq!(rows[0]["data"]["daemon_version"], "test");
    assert_eq!(rows[0]["data"]["boot_id"], svc.boot_id());
    assert_eq!(rows[1]["type"], "user_msg");
}

/// The channel is bounded and `emit` never blocks: on a full channel the
/// record is dropped and counted rather than stalling the caller (§5.5).
/// A current-thread runtime never polls the writer inside this loop, so the
/// overflow is deterministic.
#[tokio::test]
async fn a_full_channel_drops_and_counts_instead_of_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service_with(
        &dir,
        None,
        SessionLogLimits {
            channel_capacity: 2,
            ..SessionLogLimits::default()
        },
    );
    let handle = svc.handle_for("sess-full");
    for _ in 0..64 {
        handle.emit(Record::new(RecordType::Round).with_data(serde_json::json!({})));
    }
    assert!(handle.dropped() > 0, "an over-full channel drops");
    assert!(handle.flush().await);
    let rows = lines(&log_path(dir.path(), "sess-full"));
    assert!(!rows.is_empty() && rows.len() < 64);
    // Gap-free is a property of what was *written*, not of what was offered.
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row["seq"], (i + 1) as u64);
    }
}

// ── Rotation and the size cap ───────────────────────────────────────

/// §5.4: rotation at the segment cap produces `log.<first>-<last>.jsonl`,
/// and the per-session byte cap drops whole oldest segments, announcing the
/// dropped range in a `log_trimmed` record.
#[tokio::test]
async fn rotation_segments_the_log_and_the_session_cap_trims_the_oldest() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service_with(
        &dir,
        None,
        SessionLogLimits {
            rotate_bytes: 512,
            max_session_bytes: 1_500,
            ..SessionLogLimits::default()
        },
    );
    let handle = svc.handle_for("sess-rot");
    for i in 0..40 {
        handle.emit(
            Record::new(RecordType::Round).with_data(serde_json::json!({"round": i, "pad": "y".repeat(100)})),
        );
    }
    assert!(handle.flush().await);

    let session_dir = dir.path().join("sess-rot");
    let archived: Vec<String> = fs::read_dir(&session_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != LIVE_SEGMENT)
        .collect();
    assert!(!archived.is_empty(), "the log rotated: {archived:?}");
    for name in &archived {
        assert!(
            name.starts_with("log.") && name.ends_with(".jsonl") && name.contains('-'),
            "segment names carry their seq range: {name}"
        );
    }

    let total: u64 = fs::read_dir(&session_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
        .sum();
    assert!(total <= 1_500 + 512, "the session cap bounds the log: {total}");

    let trimmed: Vec<_> = read_records(&session_dir)
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == "log_trimmed")
        .collect();
    assert!(!trimmed.is_empty(), "a trim names the dropped seq range");
    assert!(trimmed[0].data["from_seq"].as_u64().unwrap() >= 1);
    assert!(trimmed[0].data["to_seq"].as_u64().unwrap() >= trimmed[0].data["from_seq"].as_u64().unwrap());

    // Reading across segments keeps the global order.
    let all = read_records(&session_dir).unwrap();
    let mut last = 0;
    for r in &all {
        assert!(r.seq > last, "segments read in seq order: {} after {last}", r.seq);
        last = r.seq;
    }
}

/// A reopened session continues its seq — including across a rotation, where
/// the live segment is empty and the last seq lives in an archived name.
#[tokio::test]
async fn seq_continues_across_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    {
        let svc = service(&dir);
        let handle = svc.handle_for("sess-reopen");
        for _ in 0..3 {
            handle.emit(Record::new(RecordType::Round).with_data(serde_json::json!({})));
        }
        assert!(handle.flush().await);
    }
    let svc = service(&dir);
    let handle = svc.handle_for("sess-reopen");
    handle.emit(Record::new(RecordType::WorkflowDone).with_data(serde_json::json!({})));
    assert!(handle.flush().await);

    let rows = lines(&log_path(dir.path(), "sess-reopen"));
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[3]["seq"], 4);
}

// ── The tool-call index row ─────────────────────────────────────────

/// §5.4's split: the payloads live in the JSONL, and `tool_execution_log`
/// carries previews plus the `log_seq` pointer — written by the same writer
/// that assigned the seq, so the two can never disagree.
#[tokio::test]
async fn a_tool_call_and_its_result_write_one_index_row() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let db = Database::open(&db_dir.path().join("t.db")).unwrap();
    let svc = service_with(&dir, Some(db.clone()), SessionLogLimits::default());
    let handle = svc.handle_for("sess-idx");

    handle.emit(
        Record::new(RecordType::ToolCall)
            .task(Some("task-7"))
            .agent(Some("research_agent::a1"))
            .with_data(serde_json::json!({
                "tool_use_id": "tu-1",
                "name": "web_fetch",
                "input": {"url": "https://example.com"},
            })),
    );
    handle.emit(
        Record::new(RecordType::ToolResult)
            .task(Some("task-7"))
            .agent(Some("research_agent::a1"))
            .with_data(serde_json::json!({
                "tool_use_id": "tu-1",
                "name": "web_fetch",
                "ok": true,
                "duration_ms": 42,
                "result": "the page body",
            })),
    );
    assert!(handle.flush().await);

    let row: (String, String, i64, i64, String, String, String, String, i64) = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT session_id, task_id, log_seq, success, agent_id, tool_name,
                        args_preview, result_preview, duration_ms
                   FROM tool_execution_log",
                [],
                |r| {
                    Ok((
                        r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?,
                        r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?,
                    ))
                },
            )?)
        })
        .expect("exactly one index row");

    assert_eq!(row.0, "sess-idx");
    assert_eq!(row.1, "task-7");
    assert_eq!(row.2, 1, "log_seq points at the tool_call record");
    assert_eq!(row.3, 1);
    assert_eq!(row.4, "research_agent::a1");
    assert_eq!(row.5, "web_fetch");
    assert!(row.6.contains("example.com"));
    assert_eq!(row.7, "the page body");
    assert_eq!(row.8, 42);

    let result_ref: String = db
        .with_connection(|conn| {
            Ok(conn.query_row("SELECT result_ref FROM tool_execution_log", [], |r| r.get(0))?)
        })
        .unwrap();
    assert_eq!(result_ref, "log:2", "result_ref names the tool_result record");
}

/// A result that hits the envelope cap still gets an index row, and its
/// preview is the bytes that actually landed inline.
#[tokio::test]
async fn a_capped_result_still_indexes_its_inline_preview() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let db = Database::open(&db_dir.path().join("t.db")).unwrap();
    let svc = service_with(&dir, Some(db.clone()), SessionLogLimits::default());
    let handle = svc.handle_for("sess-big");

    handle.emit(Record::new(RecordType::ToolCall).with_data(serde_json::json!({
        "tool_use_id": "tu-9", "name": "shell_execute", "input": {"cmd": "cargo test"},
    })));
    handle.emit(Record::new(RecordType::ToolResult).with_data(serde_json::json!({
        "tool_use_id": "tu-9", "name": "shell_execute", "ok": false,
        "duration_ms": 1, "error": "boom", "result": "z".repeat(300 * 1024),
    })));
    assert!(handle.flush().await);

    let (preview, err, success): (String, String, i64) = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT result_preview, error_message, success FROM tool_execution_log",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .unwrap();
    assert!(preview.starts_with("zzz"));
    assert!(preview.chars().count() <= openalpaca_storage::PREVIEW_CHARS);
    assert_eq!(err, "boom");
    assert_eq!(success, 0);
}

// ── Lifecycle ───────────────────────────────────────────────────────

/// An idle writer closes its file and exits; the next emit transparently
/// respawns it and keeps the numbering (§5.5's idle-close).
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_writer_closes_and_the_next_emit_respawns_it() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service_with(
        &dir,
        None,
        SessionLogLimits {
            idle_close: Duration::from_millis(50),
            ..SessionLogLimits::default()
        },
    );
    let handle = svc.handle_for("sess-idle");
    handle.emit(Record::new(RecordType::UserMsg).with_data(serde_json::json!({})));
    assert!(handle.flush().await);

    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(handle.is_closed(), "the idle writer exited");

    let fresh = svc.handle_for("sess-idle");
    fresh.emit(Record::new(RecordType::AssistantMsg).with_data(serde_json::json!({})));
    assert!(fresh.flush().await);

    let rows = lines(&log_path(dir.path(), "sess-idle"));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["seq"], 2);
}

/// Session ids reach the filesystem as directory names; nothing may escape
/// the sessions root.
#[test]
fn a_session_id_can_never_escape_the_sessions_root() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    for id in ["../escape", "a/b", "..", ".", "", "ok-id_1.2"] {
        let path = svc.session_dir(id);
        assert!(
            path.starts_with(dir.path()) && path.parent() == Some(dir.path()),
            "{id} escaped: {}",
            path.display()
        );
    }
}
