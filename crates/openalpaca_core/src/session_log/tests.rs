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

/// A payload no cut can shrink (a huge *structure* rather than a huge value)
/// falls back to a marker plus a preview — but the identity fields come with
/// it. An anonymous stub would take the record's `tool_use_id` with it and
/// leave the index row and §5.4's replay without a key.
#[tokio::test]
async fn an_over_cap_payload_with_no_single_big_field_keeps_its_identity() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let handle = svc.handle_for("sess-stub");

    let many: Vec<String> = (0..40_000).map(|i| format!("row-{i}")).collect();
    handle.emit(Record::new(RecordType::Round).with_data(serde_json::json!({
        "tool_use_id": "tu-structural",
        "name": "shell_execute",
        "tool_use": many,
    })));
    assert!(handle.flush().await);

    let rows = lines(&log_path(dir.path(), "sess-stub"));
    let data = &rows[0]["data"];
    assert!(serde_json::to_string(data).unwrap().len() <= ENVELOPE_DATA_CAP_BYTES);
    assert_eq!(data["_truncated"]["spilled_pending"], true);
    assert!(data["preview"].as_str().unwrap().chars().count() <= PREVIEW_CHARS);
    assert_eq!(data["tool_use_id"], "tu-structural");
    assert_eq!(data["name"], "shell_execute");
}

/// Critical 1: a record is over the bound because of a **nested** value far
/// more often than a top-level string — `tool_call`'s `input` is an object and
/// `round`'s `tool_use` an array. The cut must reach inside them, and it must
/// never touch the fields that identify the call: without `tool_use_id` the
/// writer's `PendingCalls` never sees the call and the index row loses both
/// `log_seq` and `args_preview`.
#[tokio::test]
async fn an_oversized_nested_input_keeps_its_identity_and_its_index_row() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let db = Database::open(&db_dir.path().join("t.db")).unwrap();
    let svc = service_with(&dir, Some(db.clone()), SessionLogLimits::default());
    let handle = svc.handle_for("sess-nested");

    let huge = "c".repeat(200 * 1024);
    handle.emit(
        Record::new(RecordType::ToolCall)
            .task(Some("task-1"))
            .agent(Some("lead_agent::a1"))
            .with_data(serde_json::json!({
                "tool_use_id": "tu-42",
                "name": "artifact_write",
                "input": {"path": "report.md", "content": huge},
            })),
    );
    handle.emit(
        Record::new(RecordType::ToolResult)
            .task(Some("task-1"))
            .agent(Some("lead_agent::a1"))
            .with_data(serde_json::json!({
                "tool_use_id": "tu-42",
                "name": "artifact_write",
                "ok": true,
                "duration_ms": 3,
                "result": "written",
            })),
    );
    assert!(handle.flush().await);

    let rows = lines(&log_path(dir.path(), "sess-nested"));
    let call = &rows[0]["data"];
    assert!(serde_json::to_string(call).unwrap().len() <= ENVELOPE_DATA_CAP_BYTES);
    assert_eq!(call["tool_use_id"], "tu-42", "the identity survives the cut");
    assert_eq!(call["name"], "artifact_write");
    assert_eq!(call["input"]["path"], "report.md", "only the oversized leaf is cut");
    assert!(
        call["input"]["content"].as_str().unwrap().contains("spill pending"),
        "{call}"
    );
    assert_eq!(call["_truncated"]["fields"][0], "input.content");
    assert!(
        call.get("preview").is_none(),
        "the record kept its shape rather than collapsing: {call}"
    );

    let (log_seq, args): (i64, String) = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT log_seq, args_preview FROM tool_execution_log",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .expect("a truncated call still indexes");
    assert_eq!(log_seq, 1, "log_seq points at the tool_call record");
    assert!(
        args.starts_with(r#"{"content":"#),
        "args_preview is the capped input, not NULL: {args}"
    );
    assert!(args.chars().count() <= openalpaca_storage::PREVIEW_CHARS);
}

/// The cut walks arrays as well as objects: three oversized elements are
/// trimmed one at a time until the envelope fits, and each element keeps its
/// own `id`/`name`.
#[tokio::test]
async fn a_nested_array_of_large_strings_is_trimmed_element_wise() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let handle = svc.handle_for("sess-arr");

    let big = "y".repeat(30 * 1024);
    handle.emit(Record::new(RecordType::Round).with_data(serde_json::json!({
        "round": 2,
        "tool_use": [
            {"id": "tu-a", "name": "shell_execute", "input": {"cmd": big.clone()}},
            {"id": "tu-b", "name": "shell_execute", "input": {"cmd": big.clone()}},
            {"id": "tu-c", "name": "shell_execute", "input": {"cmd": big}},
        ],
    })));
    assert!(handle.flush().await);

    let rows = lines(&log_path(dir.path(), "sess-arr"));
    let data = &rows[0]["data"];
    assert!(serde_json::to_string(data).unwrap().len() <= ENVELOPE_DATA_CAP_BYTES);
    assert_eq!(data["round"], 2);
    let uses = data["tool_use"]
        .as_array()
        .unwrap_or_else(|| panic!("the array survives element-wise trimming: {data}"));
    assert_eq!(uses.len(), 3);
    for (i, id) in ["tu-a", "tu-b", "tu-c"].iter().enumerate() {
        assert_eq!(uses[i]["id"], *id, "every element keeps its identity");
        assert_eq!(uses[i]["name"], "shell_execute");
    }
    assert!(
        uses.iter()
            .any(|u| u["input"]["cmd"].as_str().unwrap().contains("spill pending")),
        "at least one element was trimmed: {data}"
    );
    let fields = data["_truncated"]["fields"].as_array().unwrap();
    assert!(!fields.is_empty());
    assert!(
        fields
            .iter()
            .all(|f| f.as_str().unwrap().starts_with("tool_use[")
                && f.as_str().unwrap().ends_with("].input.cmd")),
        "the cut names the element it trimmed: {fields:?}"
    );
}

/// P-14: `preserved_from_seq` is by definition the last seq the log already
/// holds, so the writer — the only party that assigns a seq — stamps it,
/// exactly as it does `log_seq`. §5.4's replay rule ("replay only rounds with
/// `seq > preserved_from_seq`") is defined in terms of it.
#[tokio::test]
async fn a_compaction_record_carries_the_seq_of_the_record_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let handle = svc.handle_for("sess-compact");

    handle.emit(Record::new(RecordType::Round).with_data(serde_json::json!({"round": 1})));
    handle.emit(Record::new(RecordType::Round).with_data(serde_json::json!({"round": 2})));
    handle.emit(Record::new(RecordType::Compaction).with_data(serde_json::json!({
        "tier": "LlmSummary",
        "trigger": "auto",
        "cumulative_dropped_tokens": 900,
        "dropped_from_seq": null,
        "summary_msg_id": null,
    })));
    assert!(handle.flush().await);

    let rows = lines(&log_path(dir.path(), "sess-compact"));
    assert_eq!(rows[2]["seq"], 3);
    assert_eq!(
        rows[2]["data"]["preserved_from_seq"], 2,
        "the boundary is the prior record's seq"
    );
    assert_eq!(rows[2]["data"]["cumulative_dropped_tokens"], 900);
    assert!(rows[2]["data"]["dropped_from_seq"].is_null());
    assert!(rows[2]["data"]["summary_msg_id"].is_null());
}

/// Nothing precedes the first record, so there is no boundary to name — an
/// explicit null rather than a seq that does not exist.
#[tokio::test]
async fn a_compaction_record_with_no_prior_record_has_a_null_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let handle = svc.handle_for("sess-compact-first");
    handle.emit(Record::new(RecordType::Compaction).with_data(serde_json::json!({"tier": "x"})));
    assert!(handle.flush().await);

    let rows = lines(&log_path(dir.path(), "sess-compact-first"));
    assert!(rows[0]["data"]["preserved_from_seq"].is_null(), "{}", rows[0]);
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

/// A provider that issues no tool-call id (Ollama leaves it
/// `unwrap_or_default()`) must not get a second `tool_execution_log` row: the
/// daemon's audit path already wrote one, an empty `request_id` is excluded
/// from the merge on purpose, and the writer inserting anyway double-counts
/// the call in `GET /v1/tools`' `invocations_today`.
#[tokio::test]
async fn an_empty_tool_use_id_is_not_indexed() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let db = Database::open(&db_dir.path().join("t.db")).unwrap();
    let svc = service_with(&dir, Some(db.clone()), SessionLogLimits::default());
    let handle = svc.handle_for("sess-anon");

    handle.emit(Record::new(RecordType::ToolCall).with_data(serde_json::json!({
        "tool_use_id": "", "name": "shell_execute", "input": {"cmd": "ls"},
    })));
    handle.emit(Record::new(RecordType::ToolResult).with_data(serde_json::json!({
        "tool_use_id": "", "name": "shell_execute", "ok": true,
        "duration_ms": 1, "result": "a\nb\n",
    })));
    assert!(handle.flush().await);

    let rows: i64 = db
        .with_connection(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM tool_execution_log", [], |r| r.get(0))?)
        })
        .unwrap();
    assert_eq!(
        rows, 0,
        "an id-less call is left to the daemon's audit row — indexing it makes two"
    );
    // The records themselves are still written: only the index row stands down.
    assert_eq!(lines(&log_path(dir.path(), "sess-anon")).len(), 2);
}

/// The identity skip protects the keys that **pair** records — `tool_use_id`,
/// and the `id`/`name` directly under `tool_use[i]`. A key that happens to be
/// called `name` inside a tool's own `input` is payload, and refusing to cut
/// it collapses the whole record into the fallback stub for no reason.
#[tokio::test]
async fn a_big_value_inside_a_tools_input_is_cut_even_when_its_key_is_name() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let handle = svc.handle_for("sess-inner-name");

    let huge = "n".repeat(200 * 1024);
    handle.emit(Record::new(RecordType::ToolCall).with_data(serde_json::json!({
        "tool_use_id": "tu-inner",
        "name": "artifact_write",
        "input": {"name": huge, "path": "report.md"},
    })));
    assert!(handle.flush().await);

    let rows = lines(&log_path(dir.path(), "sess-inner-name"));
    let data = &rows[0]["data"];
    assert_eq!(data["_truncated"]["fields"][0], "input.name");
    assert_eq!(data["tool_use_id"], "tu-inner", "the pairing key survives");
    assert_eq!(data["name"], "artifact_write", "so does the tool's own name");
    assert_eq!(data["input"]["path"], "report.md");
    assert!(
        data.get("preview").is_none(),
        "cutting the inner value kept the record's shape: {data}"
    );
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

// ── The `results/` spill ────────────────────────────────────────────

/// §5.4's "Spill, don't truncate": the payload is written **once**, to
/// `results/`, and the record keeps the reference, the hash and a preview.
/// `tool_execution_log.result_ref` points at the same file — no second copy
/// anywhere.
#[tokio::test]
async fn a_large_tool_result_spills_to_results_and_the_record_keeps_the_reference() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let db = Database::open(&db_dir.path().join("t.db")).unwrap();
    let svc = service_with(&dir, Some(db.clone()), SessionLogLimits::default());
    let handle = svc.handle_for("sess-spill");

    let payload = "s".repeat(200 * 1024);
    let rel = handle.reserve_spill("shell_execute", "toolu_01ABCDEF");
    assert!(rel.starts_with("results/"), "the reference is session-relative: {rel}");
    assert!(rel.ends_with("-shell_execute.txt"), "{rel}");

    handle.emit(Record::new(RecordType::ToolCall).with_data(serde_json::json!({
        "tool_use_id": "toolu_01ABCDEF", "name": "shell_execute", "input": {"cmd": "cargo test"},
    })));
    handle.emit(
        Record::new(RecordType::ToolResult)
            .with_data(serde_json::json!({
                "tool_use_id": "toolu_01ABCDEF",
                "name": "shell_execute",
                "ok": true,
                "duration_ms": 9,
                "result": serde_json::Value::Null,
            }))
            .with_spill(rel.clone(), payload.clone()),
    );
    assert!(handle.flush().await);

    // The file holds the whole result, once.
    let spilled = dir.path().join("sess-spill").join(&rel);
    assert_eq!(fs::read_to_string(&spilled).unwrap(), payload);
    // No temporary file survives the rename.
    let leftovers: Vec<String> = fs::read_dir(dir.path().join("sess-spill").join("results"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.') || n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "tmp → rename leaves nothing behind: {leftovers:?}");

    let rows = lines(&log_path(dir.path(), "sess-spill"));
    let data = &rows[1]["data"];
    assert_eq!(data["result_ref"], format!("file:{rel}"));
    assert_eq!(data["result"]["spill"]["rel"], rel);
    assert_eq!(data["result"]["spill"]["bytes"], payload.len());
    assert_eq!(data["result"]["spill"]["mime"], "text/plain; charset=utf-8");
    assert_eq!(
        data["result"]["spill"]["sha256"].as_str().unwrap().len(),
        64,
        "the stub carries a full sha256"
    );
    assert_eq!(
        data["result"]["preview"].as_str().unwrap().chars().count(),
        PREVIEW_CHARS,
        "the record keeps the first 2 KB and nothing more"
    );
    assert!(
        serde_json::to_string(data).unwrap().len() <= ENVELOPE_DATA_CAP_BYTES,
        "a spilled result never trips the envelope cap"
    );
    assert!(
        data.get("_truncated").is_none(),
        "nothing was truncated — it was spilled: {data}"
    );

    let (preview, result_ref): (String, String) = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT result_preview, result_ref FROM tool_execution_log",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(result_ref, format!("file:{rel}"), "the index row names the same file");
    assert_eq!(
        preview,
        data["result"]["preview"].as_str().unwrap(),
        "the row's preview is the identical bytes the record kept"
    );
}

/// The reference names one call, and the model may already be paging the bytes
/// behind it. A second arrival must not clobber them.
#[tokio::test]
async fn a_spill_never_overwrites_an_existing_result_file() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let handle = svc.handle_for("sess-once");

    let rel = handle.reserve_spill("dump", "tu-once");
    for body in ["first", "second"] {
        handle.emit(
            Record::new(RecordType::ToolResult)
                .with_data(serde_json::json!({"tool_use_id": "tu-once", "name": "dump", "ok": true}))
                .with_spill(rel.clone(), body.to_string()),
        );
    }
    assert!(handle.flush().await);

    assert_eq!(
        fs::read_to_string(dir.path().join("sess-once").join(&rel)).unwrap(),
        "first"
    );
}

/// The spill's number tracks the writer's watermark, so a session reopened
/// after a restart keeps climbing instead of restarting at 1 and colliding
/// with the files a previous boot left.
#[tokio::test]
async fn a_reopened_sessions_spill_numbering_resumes_from_the_log() {
    let dir = tempfile::tempdir().unwrap();
    {
        let svc = service(&dir);
        let handle = svc.handle_for("sess-resume");
        for i in 0..5 {
            handle.emit(Record::new(RecordType::Round).with_data(serde_json::json!({"round": i})));
        }
        assert!(handle.flush().await);
    }

    let svc = service(&dir);
    let handle = svc.handle_for("sess-resume");
    // The writer publishes its watermark when it opens the existing log, so
    // the first reservation of the new boot has to wait for that to happen.
    handle.emit(Record::new(RecordType::Round).with_data(serde_json::json!({"round": 5})));
    assert!(handle.flush().await);

    let rel = handle.reserve_spill("dump", "tu-resume");
    let number: u64 = rel
        .trim_start_matches("results/")
        .split('-')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(number > 5, "the numbering resumed from the log, got {rel}");
}

/// §5.4: a trim drops "the spill files they reference" — read out of the
/// segment being dropped, never guessed from a filename. The live segment's
/// spill survives.
#[tokio::test]
async fn a_trim_drops_exactly_the_spill_files_the_dropped_segment_referenced() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service_with(
        &dir,
        None,
        SessionLogLimits {
            rotate_bytes: 700,
            max_session_bytes: 2_000,
            ..SessionLogLimits::default()
        },
    );
    let handle = svc.handle_for("sess-trim");
    let session_dir = dir.path().join("sess-trim");

    let mut rels = Vec::new();
    for i in 0..30 {
        let rel = handle.reserve_spill("dump", &format!("tu-{i}"));
        rels.push(rel.clone());
        handle.emit(
            Record::new(RecordType::ToolResult)
                .with_data(serde_json::json!({
                    "tool_use_id": format!("tu-{i}"), "name": "dump", "ok": true,
                }))
                .with_spill(rel, "p".repeat(200)),
        );
    }
    assert!(handle.flush().await);

    let surviving: Vec<&String> = rels
        .iter()
        .filter(|rel| session_dir.join(rel).exists())
        .collect();
    assert!(!surviving.is_empty(), "the newest spills survive");
    assert!(surviving.len() < rels.len(), "the oldest spills were evicted");

    // Every surviving file is still referenced by a surviving record, and
    // every surviving record's reference still resolves.
    let referenced: Vec<String> = read_records(&session_dir)
        .unwrap()
        .into_iter()
        .filter_map(|r| {
            r.data
                .get("result_ref")
                .and_then(|v| v.as_str())
                .map(|s| s.trim_start_matches("file:").to_string())
        })
        .collect();
    for rel in &referenced {
        assert!(
            session_dir.join(rel).exists(),
            "a surviving record's spill was evicted under it: {rel}"
        );
    }
    for rel in &surviving {
        assert!(
            referenced.contains(rel),
            "an evicted record left its spill file behind: {rel}"
        );
    }
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

/// Retired writers' drops must not vanish from the total: `GET /v1/status`
/// documents `dropped_records` as a per-boot count, so an idle respawn that
/// resets a live handle's own counter to zero must not let the reported
/// number go down (Important #4, T44 fix round 1).
#[tokio::test(flavor = "multi_thread")]
async fn dropped_total_survives_an_idle_writer_being_respawned() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service_with(
        &dir,
        None,
        SessionLogLimits {
            channel_capacity: 2,
            idle_close: Duration::from_millis(50),
            ..SessionLogLimits::default()
        },
    );

    let handle = svc.handle_for("sess-retire");
    for _ in 0..64 {
        handle.emit(Record::new(RecordType::Round).with_data(serde_json::json!({})));
    }
    let first_round_drops = handle.dropped();
    assert!(first_round_drops > 0, "an over-full channel drops");
    assert!(handle.flush().await);

    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(handle.is_closed(), "the idle writer exited");

    // Asking for a handle on the same session respawns the writer — and must
    // fold the retiring handle's count in before its slot is overwritten.
    let fresh = svc.handle_for("sess-retire");
    assert_eq!(
        svc.dropped_total(),
        first_round_drops,
        "the retired writer's drops must not disappear when it is replaced"
    );

    for _ in 0..64 {
        fresh.emit(Record::new(RecordType::Round).with_data(serde_json::json!({})));
    }
    let second_round_drops = fresh.dropped();
    assert!(
        second_round_drops > 0,
        "the fresh writer's own channel also overflows"
    );

    assert_eq!(
        svc.dropped_total(),
        first_round_drops + second_round_drops,
        "the total accumulates across a respawn instead of resetting"
    );
}

/// R52: the writer must never block a runtime thread. Its file appends, its
/// `sync_data` and its SQLite update all run inside `spawn_blocking`, so a
/// slow disk — or, as here, the daemon's single connection mutex held by
/// somebody else — stalls one blocking thread and nothing else.
///
/// The probe is a timer task on the same current-thread runtime: if the
/// writer did its work inline, the runtime would be occupied for the whole
///300 ms the connection is held and the ticker could not advance.
#[tokio::test(flavor = "current_thread")]
async fn the_writer_does_its_io_off_the_runtime_thread() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let db = Database::open(&db_dir.path().join("t.db")).unwrap();
    let svc = service_with(&dir, Some(db.clone()), SessionLogLimits::default());
    let handle = svc.handle_for("sess-blocking");

    // Somebody else holds the process-wide connection mutex.
    let hold = db.clone();
    let holder = std::thread::spawn(move || {
        hold.with_connection(|_| {
            std::thread::sleep(Duration::from_millis(300));
            Ok(())
        })
        .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;

    let ticks = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ticker = ticks.clone();
    let counting = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(20)).await;
            ticker.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    });

    handle.emit(Record::new(RecordType::ToolCall).with_data(serde_json::json!({
        "tool_use_id": "tu-block", "name": "web_fetch", "input": {"url": "u"},
    })));
    handle.emit(Record::new(RecordType::ToolResult).with_data(serde_json::json!({
        "tool_use_id": "tu-block", "name": "web_fetch", "ok": true,
        "duration_ms": 1, "result": "body",
    })));
    assert!(handle.flush().await);
    counting.abort();
    holder.join().unwrap();

    assert!(
        ticks.load(std::sync::atomic::Ordering::Relaxed) >= 3,
        "the runtime kept running while the writer waited on the DB: {} ticks",
        ticks.load(std::sync::atomic::Ordering::Relaxed)
    );

    let rows = lines(&log_path(dir.path(), "sess-blocking"));
    assert_eq!(rows.len(), 2);
    let log_seq: i64 = db
        .with_connection(|conn| {
            Ok(conn.query_row("SELECT log_seq FROM tool_execution_log", [], |r| r.get(0))?)
        })
        .unwrap();
    assert_eq!(log_seq, 1, "the index row landed all the same");
}

/// The shutdown barrier: `emit` is a non-blocking `try_send` and the writer
/// syncs only on boundaries and a 5 s timer, so on SIGTERM everything still
/// queued would go with the runtime. `flush_all` drains every live writer and
/// syncs it — the daemon awaits it before its writer tasks are dropped, and
/// the session routes await it before archiving or deleting a transcript.
#[tokio::test]
async fn flush_all_drains_every_live_writer_before_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let a = svc.handle_for("sess-a");
    let b = svc.handle_for("sess-b");

    // Nothing has awaited yet, so on a current-thread runtime these are all
    // still sitting in their channels.
    for i in 0..64 {
        assert!(a.emit(Record::new(RecordType::Round).with_data(serde_json::json!({"round": i}))));
        assert!(b.emit(Record::new(RecordType::Round).with_data(serde_json::json!({"round": i}))));
    }

    svc.flush_all().await;

    for session in ["sess-a", "sess-b"] {
        let rows = lines(&log_path(dir.path(), session));
        assert_eq!(rows.len(), 64, "{session} was drained, not dropped");
        assert_eq!(rows[63]["data"]["round"], 63);
    }
    assert_eq!(svc.dropped_total(), 0);
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

// ── Reading back (the cursor) ───────────────────────────────────────

/// A cursor past a rotated segment must not pay for that segment.
///
/// The reader deserialised **every** line from seq 1 and only then discarded
/// what the cursor had already seen — O(records so far) per page, i.e. O(n²)
/// to drain a log, with a full `serde_json` parse per skipped line. Now an
/// archived segment whose range ends at or before the cursor is skipped
/// whole, and inside the segment it does read, each line's seq is found with a
/// byte search before anything is parsed.
#[test]
fn paging_past_rotated_segments_parses_none_of_their_lines() {
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("sess-page");
    fs::create_dir_all(&session).unwrap();

    let line = |seq: u64| {
        format!(
            r#"{{"v":1,"seq":{seq},"ts":"2026-09-05T10:22:03.114Z","type":"round","data":{{"n":{seq}}}}}"#
        )
    };
    let segment = |first: u64, last: u64| -> String {
        (first..=last).map(|s| format!("{}\n", line(s))).collect()
    };
    fs::write(session.join("log.1-100.jsonl"), segment(1, 100)).unwrap();
    fs::write(session.join("log.101-200.jsonl"), segment(101, 200)).unwrap();
    fs::write(session.join(LIVE_SEGMENT), segment(201, 210)).unwrap();

    reader::reset_parse_count();
    let page = read_records_after(&session, Some(205), 10).unwrap();

    assert_eq!(page.len(), 5, "the cursor's tail: {page:?}");
    assert_eq!(page[0].seq, 206);
    assert_eq!(
        reader::parses_on_this_thread(),
        5,
        "only the five records the page returns are parsed — the 200 records \
         of the rotated segments and the five skipped live lines are not"
    );
}

/// The page also carries a byte budget, so a client asking for 500 records of
/// 64 KB envelopes is not handed a ~32 MB response — it gets a smaller page
/// and a cursor to come back with.
#[test]
fn a_page_stops_at_its_byte_budget_and_still_answers_a_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("sess-fat");
    fs::create_dir_all(&session).unwrap();
    let fat: String = (1..=40)
        .map(|seq| {
            format!(
                r#"{{"v":1,"seq":{seq},"ts":"2026-09-05T10:22:03.114Z","type":"round","data":{{"text":"{}"}}}}"#,
                "x".repeat(10_000)
            ) + "\n"
        })
        .collect();
    fs::write(session.join(LIVE_SEGMENT), fat).unwrap();

    let page = read_records_page(&session, None, 500, 100_000).unwrap();
    assert!(
        (1..40).contains(&page.len()),
        "the budget stopped the page short of the 40 records asked for: {}",
        page.len()
    );
    assert_eq!(page[0].seq, 1);
    // And the cursor is usable: the next page continues where this stopped.
    let cursor = page.last().unwrap().seq;
    let next = read_records_page(&session, Some(cursor), 500, 100_000).unwrap();
    assert_eq!(next[0].seq, cursor + 1);

    // No budget, no truncation.
    assert_eq!(read_records_page(&session, None, 500, usize::MAX).unwrap().len(), 40);
}

// ── The cursor's cheap seq (R55) ────────────────────────────────────

/// A record whose payload carries a `seq` of its own — the shape the model
/// produces whenever it calls a tool with a `seq` argument (a cursor, a page,
/// a message id), which no schema forbids.
fn tool_call_with_a_payload_seq(payload_seq: u64) -> Record {
    Record::new(RecordType::ToolCall)
        .task(Some("task-1"))
        .span(Some("span-1"))
        .agent(Some("lead_agent::a1b2c3d4"))
        .with_data(serde_json::json!({
            "tool_use_id": "toolu_01",
            "name": "list_messages",
            "input": {"seq": payload_seq, "limit": 20},
        }))
}

/// The same envelope, serialised the way a `serde_json::Map` used to serialise
/// it — lexicographically, `agent` … `data` … `seq` — which is what every line
/// already on disk looks like.
fn legacy_ordered_line(seq: u64, data: serde_json::Value) -> String {
    serde_json::to_string(&serde_json::json!({
        "v": 1,
        "seq": seq,
        "ts": "2026-09-05T10:22:03.114Z",
        "type": "tool_call",
        "task_id": "task-1",
        "span_id": "span-1",
        "agent": "lead_agent::a1b2c3d4",
        "data": data,
    }))
    .expect("an object of owned values cannot fail to serialise")
}

/// R55(a): the envelope's field order is the writer's, not the key names'.
///
/// It was a `serde_json::Map` — a `BTreeMap` without `preserve_order` — so the
/// line came out `{"agent":…,"data":…,"seq":…}` and `data` preceded the
/// record's own `seq`. A `#[derive(Serialize)]` struct keeps the declared
/// order, so `seq` is always the second key.
#[test]
fn the_envelope_puts_seq_second_whatever_the_key_names_sort_to() {
    let line = tool_call_with_a_payload_seq(7).to_line(42);
    assert!(
        line.starts_with(r#"{"v":1,"seq":42,"ts":"#),
        "the envelope must open with v then seq: {line}"
    );
    // And the shape is unchanged otherwise — same keys, same values.
    let parsed: LoggedRecord = serde_json::from_str(&line).expect("one JSON object");
    assert_eq!(parsed.seq, 42);
    assert_eq!(parsed.kind, "tool_call");
    assert_eq!(parsed.task_id.as_deref(), Some("task-1"));
    assert_eq!(parsed.span_id.as_deref(), Some("span-1"));
    assert_eq!(parsed.agent.as_deref(), Some("lead_agent::a1b2c3d4"));
    assert_eq!(parsed.data["input"]["seq"], 7);
    assert_eq!(parsed.v, 1);
}

/// R55(b): the scan is depth-aware, so a `seq` in the payload is never read as
/// the envelope's — under the writer's order *and* under the lexicographic
/// order the log already holds.
#[test]
fn a_seq_inside_the_payload_is_never_read_as_the_envelopes() {
    let record = tool_call_with_a_payload_seq(7);

    let written = record.to_line(42);
    assert_eq!(reader::scan_seq(written.as_bytes()), Some(42), "{written}");

    let legacy = legacy_ordered_line(42, record.data.clone());
    assert!(
        legacy.starts_with(r#"{"agent":"#) && legacy.find(r#""data""#) < legacy.find(r#","seq":"#),
        "the fixture must be the old lexicographic shape: {legacy}"
    );
    assert_eq!(reader::scan_seq(legacy.as_bytes()), Some(42), "{legacy}");

    // A payload seq nested deeper, and one in an array, are equally ignored.
    let deep = legacy_ordered_line(
        99,
        serde_json::json!({"input": {"page": [{"seq": 1}, {"seq": 2}], "cursor": {"a": {"seq": 3}}}}),
    );
    assert_eq!(reader::scan_seq(deep.as_bytes()), Some(99), "{deep}");
}

/// A `"seq":` that lives inside a JSON *string* — escaped quotes and all — is
/// text, not a key. So is a brace or bracket in prose, which must not move the
/// scanner's depth.
#[test]
fn an_escaped_quote_in_a_string_does_not_fool_the_seq_scan() {
    let prose = serde_json::json!({
        "input": {"text": r#"the log said "seq": 7 next to a } and a ] and a trailing \"#},
    });
    let legacy = legacy_ordered_line(42, prose.clone());
    assert!(legacy.contains(r#"\"seq\":"#), "the fixture must escape its quotes: {legacy}");
    assert_eq!(reader::scan_seq(legacy.as_bytes()), Some(42), "{legacy}");

    let written = Record::new(RecordType::ToolCall).with_data(prose).to_line(42);
    assert_eq!(reader::scan_seq(written.as_bytes()), Some(42), "{written}");
}

/// A tiny deterministic PRNG: a property test must fail the same way twice and
/// must not cost the workspace a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a, T>(&mut self, from: &'a [T]) -> &'a T {
        &from[self.below(from.len() as u64) as usize]
    }
}

/// Payload values built out of exactly the things that can fool a byte scan:
/// `seq` keys at every depth, brace/bracket characters inside strings, escaped
/// quotes, a lone trailing backslash, and non-ASCII.
fn any_payload(rng: &mut Rng, depth: u32) -> serde_json::Value {
    const KEYS: &[&str] =
        &["seq", "data", "input", "log_seq", "from_seq", "name", "v", "type", "\"seq\":", "{"];
    const STRINGS: &[&str] = &[
        "",
        "plain",
        r#"{"seq": 9}"#,
        r#"he said "seq": 9"#,
        r"back\slash",
        r"trailing\",
        "brace } and bracket ]",
        "unicode ✓ ünï",
    ];
    match rng.below(if depth >= 3 { 4 } else { 6 }) {
        0 => serde_json::Value::from(rng.below(10_000)),
        1 => serde_json::Value::Bool(rng.below(2) == 0),
        2 => serde_json::Value::Null,
        3 => serde_json::Value::from(*rng.pick(STRINGS)),
        4 => {
            let n = rng.below(4);
            serde_json::Value::Array((0..n).map(|_| any_payload(rng, depth + 1)).collect())
        }
        _ => {
            let n = rng.below(5);
            let mut map = serde_json::Map::new();
            for _ in 0..n {
                map.insert(rng.pick(KEYS).to_string(), any_payload(rng, depth + 1));
            }
            serde_json::Value::Object(map)
        }
    }
}

/// R55's property: for every line the writer itself produces, the cheap scan
/// and the authoritative parse agree on the seq. If they ever disagree the
/// cursor drops records, silently and permanently.
#[test]
fn the_seq_scan_agrees_with_the_parser_on_every_line_the_writer_writes() {
    let mut rng = Rng(0x5EED_5EED_5EED_5EED);
    for i in 0..1_000u64 {
        let seq = rng.below(u64::from(u32::MAX)) + 1;
        let mut data = serde_json::Map::new();
        let n = rng.below(5);
        for _ in 0..n {
            data.insert(
                rng.pick(&["seq", "input", "tool_use_id", "name", "result", "ext"]).to_string(),
                any_payload(&mut rng, 1),
            );
        }
        let optional = ["task-1", r#"a "seq": 3 in an id"#, "{}"];
        let record = Record::new(*rng.pick(&RecordType::ALL))
            .with_data(serde_json::Value::Object(data))
            .task((rng.below(2) == 0).then(|| *rng.pick(&optional)))
            .span((rng.below(2) == 0).then(|| *rng.pick(&optional)))
            .agent((rng.below(2) == 0).then(|| *rng.pick(&optional)));

        let line = record.to_line(seq);
        let parsed: LoggedRecord =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("case {i}: {e}: {line}"));
        assert_eq!(parsed.seq, seq, "case {i}: {line}");
        assert_eq!(
            reader::scan_seq(line.as_bytes()),
            Some(parsed.seq),
            "case {i}: the scan disagreed with the parse: {line}"
        );
    }
}

/// The consequence the scan exists to avoid: paging a real, writer-written log
/// must return every record exactly once. A payload `seq` below the cursor
/// used to make the record vanish from the page it belonged to and from every
/// later page — a permanent, silent hole in the events stream.
#[test]
fn paging_a_writer_written_log_never_drops_a_record() {
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("sess-payload-seq");
    fs::create_dir_all(&session).unwrap();

    let body: String =
        (1..=30).map(|seq| tool_call_with_a_payload_seq(3).to_line(seq)).collect();
    fs::write(session.join(LIVE_SEGMENT), body).unwrap();

    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = read_records_page(&session, cursor, 7, usize::MAX).unwrap();
        if page.is_empty() {
            break;
        }
        cursor = Some(page.last().unwrap().seq);
        seen.extend(page.iter().map(|r| r.seq));
    }
    assert_eq!(seen, (1..=30).collect::<Vec<_>>(), "records went missing from the stream");
}

/// The same, for the lines written before the field order was fixed: they are
/// on disk already and the depth-aware scan is what keeps them readable.
#[test]
fn paging_a_log_written_before_the_order_was_fixed_never_drops_a_record() {
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("sess-legacy-order");
    fs::create_dir_all(&session).unwrap();

    let body: String = (1..=30)
        .map(|seq| legacy_ordered_line(seq, serde_json::json!({"input": {"seq": 3}})) + "\n")
        .collect();
    fs::write(session.join(LIVE_SEGMENT), body).unwrap();

    let page = read_records_page(&session, Some(10), 100, usize::MAX).unwrap();
    assert_eq!(
        page.iter().map(|r| r.seq).collect::<Vec<_>>(),
        (11..=30).collect::<Vec<_>>(),
        "a legacy-ordered line must page by its envelope seq, not its payload's"
    );
}

// ── The boot sweep (the global cap) ─────────────────────────────────

/// Give a session's files a known age so "oldest-touched" is testable.
fn age(root: &Path, session: &str, secs_ago: u64) {
    let when = std::time::SystemTime::now() - Duration::from_secs(secs_ago);
    let dir = root.join(session);
    let mut stack = vec![dir];
    while let Some(next) = stack.pop() {
        for entry in fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(file) = fs::OpenOptions::new().write(true).open(&path) {
                let _ = file.set_modified(when);
            }
        }
    }
}

/// Lay down a session with a live segment, one rotated segment and `spills`
/// spill files, each `bytes` long.
fn seed_session(root: &Path, session: &str, spills: usize, bytes: usize) {
    let dir = root.join(session);
    fs::create_dir_all(dir.join("results")).unwrap();
    fs::write(dir.join(LIVE_SEGMENT), "x".repeat(bytes)).unwrap();
    fs::write(dir.join("log.1-9.jsonl"), "x".repeat(bytes)).unwrap();
    for i in 0..spills {
        fs::write(dir.join(format!("results/00000{i}-t-dump.txt")), "y".repeat(bytes)).unwrap();
    }
}

fn total_bytes(root: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

/// §5.4's `log_max_total_bytes`: "Across all sessions. Evict oldest-touched
/// **archived** sessions' logs first, LRU; an active session's log is never
/// evicted."
///
/// Inside one archived session the order is least-destructive first:
/// `results/` before segments, and the live segment last of all.
#[test]
fn the_boot_sweep_evicts_the_oldest_archived_sessions_first() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for (session, age_secs) in [("oldest", 9_000), ("middle", 6_000), ("newest", 100)] {
        seed_session(root, session, 4, 1_000);
        age(root, session, age_secs);
    }
    // 3 sessions x (live + rotated + 4 spills) x 1 000 bytes.
    let before = total_bytes(root);
    assert_eq!(before, 18_000);

    let active: std::collections::HashSet<String> = std::collections::HashSet::new();
    // 4 000 to free: exactly the oldest session's four spill files.
    let report = sweep::enforce_total_cap(root, 14_000, &active, None).unwrap();

    assert!(report.bytes_freed > 0);
    assert!(total_bytes(root) <= 14_000, "the sweep brought the root under its cap");
    assert!(!report.over_cap_after);
    for i in 0..4 {
        assert!(
            !root.join("oldest").join(format!("results/00000{i}-t-dump.txt")).exists(),
            "the oldest session's spills went first"
        );
    }
    // Least destructive first: the payloads went, the narrative stayed.
    assert!(
        root.join("oldest").join("log.1-9.jsonl").exists(),
        "segments are only reached once results/ is exhausted"
    );
    assert!(
        root.join("oldest").join(LIVE_SEGMENT).exists(),
        "and the live segment is the very last thing in a session to go"
    );
    assert!(
        root.join("newest").join("results/000000-t-dump.txt").exists(),
        "the newest session was not reached"
    );
}

/// R54: an **archived** session gives up everything, its live segment
/// included — §5.3's source-of-truth split keeps the chat content in SQLite,
/// so the JSONL is loop detail. Without this the cap is unenforceable in the
/// common shape: §5.4 says most sessions never rotate, so most sessions have
/// exactly one segment — the live one — and nothing under `results/`.
#[test]
fn the_boot_sweep_converges_on_a_root_of_single_segment_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // What a real root looks like: sessions that never rotated and never
    // spilled. Under the old rule this list of candidates was empty.
    for (i, session) in ["s1", "s2", "s3", "s4", "s5", "s6"].iter().enumerate() {
        let session_dir = root.join(session);
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join(LIVE_SEGMENT), "x".repeat(1_000)).unwrap();
        age(root, session, (6 - i as u64) * 1_000);
    }
    assert_eq!(total_bytes(root), 6_000);

    let active = std::collections::HashSet::new();
    let report = sweep::enforce_total_cap(root, 2_000, &active, None).unwrap();

    assert!(total_bytes(root) <= 2_000, "the root converges under its cap");
    assert!(!report.over_cap_after, "and the report says so");
    assert_eq!(report.files_removed, 4);
    // LRU: the four oldest gave up their logs, the two newest kept theirs.
    for gone in ["s1", "s2", "s3", "s4"] {
        assert!(!root.join(gone).join(LIVE_SEGMENT).exists(), "{gone} was evicted");
    }
    for kept in ["s5", "s6"] {
        assert!(root.join(kept).join(LIVE_SEGMENT).exists(), "{kept} was not reached");
    }
}

/// "An active session's log is never evicted" — even when it is the oldest
/// thing on disk and the cap cannot be met without it. The line R54 draws is
/// between an **active** session and an archived one, not between a live
/// segment and a rotated one.
#[test]
fn the_boot_sweep_never_touches_a_live_session() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed_session(root, "live", 6, 2_000);
    age(root, "live", 100_000);
    seed_session(root, "archived", 2, 1_000);
    age(root, "archived", 10);

    let active: std::collections::HashSet<String> = ["live".to_string()].into_iter().collect();
    let report = sweep::enforce_total_cap(root, 1_000, &active, None).unwrap();

    for i in 0..6 {
        assert!(
            root.join("live").join(format!("results/00000{i}-t-dump.txt")).exists(),
            "an active session's spill is never evicted"
        );
    }
    assert!(root.join("live").join("log.1-9.jsonl").exists());
    assert!(
        root.join("live").join(LIVE_SEGMENT).exists(),
        "and least of all an active session's live segment"
    );
    // The cap could not be met, and the sweep says so rather than pretending.
    assert!(report.bytes_after > 1_000);
    assert!(report.over_cap_after, "the report admits the cap was not met");
    assert!(
        !root.join("archived").join("results/000000-t-dump.txt").exists(),
        "everything evictable went"
    );
    assert!(
        !root.join("archived").join(LIVE_SEGMENT).exists(),
        "an archived session's live segment is evictable — that is R54"
    );
}

/// The sweep's only record is the files it removed, so a crash between two
/// deletions is a re-run, not a repair. Re-running it must free nothing more
/// and must not error on what is already gone.
#[test]
fn the_boot_sweep_is_idempotent_across_a_crash_mid_eviction() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed_session(root, "a", 4, 1_000);
    age(root, "a", 9_000);
    seed_session(root, "b", 4, 1_000);
    age(root, "b", 100);

    let active = std::collections::HashSet::new();
    // The crash: a file the sweep was about to remove is already gone,
    // exactly as a half-finished previous run would have left it.
    fs::remove_file(root.join("a").join("results/000001-t-dump.txt")).unwrap();

    let first = sweep::enforce_total_cap(root, 8_000, &active, None).unwrap();
    assert!(first.bytes_freed > 0);
    let after_first = total_bytes(root);

    let second = sweep::enforce_total_cap(root, 8_000, &active, None).unwrap();
    assert_eq!(second.bytes_freed, 0, "a second pass has nothing left to do");
    assert_eq!(total_bytes(root), after_first, "and changes nothing");
}

/// §1.3 rule 3: the store deletes only what it created. A stray name at the
/// sessions root is counted (it is taking disk) but never removed.
#[test]
fn the_boot_sweep_leaves_names_it_did_not_create_alone() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed_session(root, "a", 4, 1_000);
    age(root, "a", 9_000);
    fs::write(root.join("NOTES.md"), "not the store's").unwrap();
    fs::create_dir_all(root.join("a").join("snapshots")).unwrap();
    fs::write(root.join("a").join("snapshots/keep.png"), "reserved").unwrap();

    let active = std::collections::HashSet::new();
    sweep::enforce_total_cap(root, 1, &active, None).unwrap();

    assert!(root.join("NOTES.md").exists(), "an unknown name is left alone");
    assert!(
        root.join("a").join("snapshots/keep.png").exists(),
        "snapshots/ is reserved for Phase 8, not the sweep's to empty"
    );
    // Only the names this store writes are ever removed — and `a` is
    // archived, so under R54 that now includes its live segment.
    assert!(!root.join("a").join(LIVE_SEGMENT).exists());
}

/// The pass's report outlives it on the service, so T44's `GET /v1/status`
/// can say the log is over its cap instead of that fact living only in a boot
/// log line.
#[test]
fn the_service_carries_the_boot_sweeps_report() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed_session(root, "a", 4, 1_000);
    age(root, "a", 9_000);

    // Active, so nothing in it is a candidate and the cap cannot be met.
    let active: std::collections::HashSet<String> = ["a".to_string()].into_iter().collect();
    let report = sweep::enforce_total_cap(root, 1, &active, None).unwrap();
    assert!(report.over_cap_after, "only protected bytes are left");

    let plain = service(&dir);
    assert!(plain.last_sweep().is_none(), "a service that swept nothing says nothing");
    let swept = service(&dir).with_last_sweep(report.clone());
    assert_eq!(swept.last_sweep(), Some(&report));
    assert!(swept.last_sweep().is_some_and(|r| r.over_cap_after));
}

// ── §5.6b: reading a crashed run's undelivered interjections ─────────

/// Write `records` (`(kind, task_id, data)`) as a live segment for `session`,
/// numbering them from 1 the way the writer does.
fn seed_records(root: &Path, session: &str, records: &[(&str, &str, serde_json::Value)]) {
    let dir = root.join(session);
    fs::create_dir_all(&dir).unwrap();
    let mut out = String::new();
    for (seq, (kind, task_id, data)) in records.iter().enumerate() {
        let line = serde_json::json!({
            "v": 1,
            "seq": seq as u64 + 1,
            "ts": "2026-09-06T10:00:00.000Z",
            "type": kind,
            "task_id": task_id,
            "data": data,
        });
        out.push_str(&serde_json::to_string(&line).unwrap());
        out.push('\n');
    }
    fs::write(dir.join(LIVE_SEGMENT), out).unwrap();
}

fn steering(request_id: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "request_id": request_id,
        "lane_key": "u:gui",
        "text": text,
        "received_at": "2026-09-06T10:00:00+00:00",
        "queue_depth": 1,
        "principal": {"User": {"global_id": "u-42"}},
        "workspace_path": "/repo",
    })
}

fn drained(ids: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "at": "round_boundary",
        "round": 2,
        "count": ids.len(),
        "request_ids": ids,
    })
}

/// §5.6b's rule, literally: a `steering` record with no later
/// `steering_drained` naming its request id is an interjection the workflow
/// never delivered. One that *is* named was seen by the model and must never
/// be re-queued — the user would be answered twice.
#[test]
fn an_undrained_steering_record_is_found_and_a_drained_one_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed_records(
        root,
        "s1",
        &[
            ("steering", "t1", steering("r-1", "delivered")),
            ("steering", "t1", steering("r-2", "never seen")),
            ("steering_drained", "t1", drained(&["r-1"])),
        ],
    );

    let scan = recovery::undrained_steering(&root.join("s1"), "t1").unwrap();
    assert_eq!(scan.unrecoverable, 0);
    assert_eq!(scan.undrained.len(), 1);
    assert_eq!(scan.undrained[0].request_id, "r-2");
    assert_eq!(scan.undrained[0].text, "never seen");
    assert_eq!(scan.undrained[0].workspace_path.as_deref(), Some("/repo"));
    // The principal round-trips as the JSON `lane_followups.principal_json`
    // holds — the same string the graceful path writes.
    let principal: serde_json::Value =
        serde_json::from_str(&scan.undrained[0].principal_json).unwrap();
    assert_eq!(principal["User"]["global_id"], "u-42");
}

/// A session holds every run started from that conversation (§5.1), so the
/// scan must not hand one crash the interjections of the run beside it.
#[test]
fn the_scan_is_scoped_to_one_run_inside_a_shared_session() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed_records(
        root,
        "s1",
        &[
            ("steering", "t1", steering("r-1", "for t1")),
            ("steering", "t2", steering("r-2", "for t2")),
            // A drain on the *other* run must not clear t1's push.
            ("steering_drained", "t2", drained(&["r-1", "r-2"])),
        ],
    );

    let t1 = recovery::undrained_steering(&root.join("s1"), "t1").unwrap();
    assert_eq!(
        t1.undrained.iter().map(|u| u.text.as_str()).collect::<Vec<_>>(),
        vec!["for t1"]
    );
    let t2 = recovery::undrained_steering(&root.join("s1"), "t2").unwrap();
    assert!(t2.undrained.is_empty(), "t2's own drain named it");
}

/// A record written before the principal was carried cannot be filed:
/// `lane_followups.principal_json` is NOT NULL and an identity nobody
/// asserted must not be invented. It is counted, not dropped in silence.
#[test]
fn a_record_without_a_principal_is_counted_as_unrecoverable() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut old = steering("r-1", "from an older build");
    old.as_object_mut().unwrap().remove("principal");
    seed_records(root, "s1", &[("steering", "t1", old)]);

    let scan = recovery::undrained_steering(&root.join("s1"), "t1").unwrap();
    assert!(scan.undrained.is_empty());
    assert_eq!(scan.unrecoverable, 1);
}

/// P-22: the session directory is the writer's to create. A run that emitted
/// nothing has no directory, and asking about it is not an error.
#[test]
fn a_session_with_no_log_recovers_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let scan = recovery::undrained_steering(&dir.path().join("never-written"), "t1").unwrap();
    assert_eq!(scan, recovery::SteeringScan::default());
}

/// The scan pages the log rather than reading it whole — a session may hold
/// 256 MB and this runs at boot. The page size is 512, so a log longer than
/// one page must still find a push in its first page and a drain in its last.
#[test]
fn the_scan_pages_a_log_longer_than_one_page() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut records: Vec<(&str, &str, serde_json::Value)> =
        vec![("steering", "t1", steering("r-1", "early push"))];
    let filler: Vec<serde_json::Value> = (0..1200)
        .map(|i| serde_json::json!({ "round": i }))
        .collect();
    for value in &filler {
        records.push(("round", "t1", value.clone()));
    }
    records.push(("steering", "t1", steering("r-2", "late push")));
    records.push(("steering_drained", "t1", drained(&["r-1"])));
    seed_records(root, "s1", &records);

    let scan = recovery::undrained_steering(&root.join("s1"), "t1").unwrap();
    assert_eq!(
        scan.undrained.iter().map(|u| u.request_id.as_str()).collect::<Vec<_>>(),
        vec!["r-2"],
        "the drain in the last page cleared the push in the first"
    );
}

// ── The eviction's write-first de-indexing (T42 re-review, Minor 2) ──

/// One `tool_execution_log` row for `session`, with both halves: the daemon's
/// audit columns and the session writer's index pointers.
fn seed_index_row(db: &Database, session: &str, seq: i64, tool: &str) {
    use openalpaca_storage::models::skill_execution::ToolExecutionEntry;
    use openalpaca_storage::repository::SkillExecutionRepository;
    SkillExecutionRepository::new(db)
        .attach_session_index(&ToolExecutionEntry {
            request_id: Some(format!("toolu_{seq}")),
            agent_id: "lead_agent".to_string(),
            tool_name: tool.to_string(),
            success: true,
            duration_ms: 12,
            session_id: Some(session.to_string()),
            log_seq: Some(seq),
            args_preview: Some("{}".to_string()),
            result_preview: Some("ok".to_string()),
            result_ref: Some(format!("log:{seq}")),
            ..Default::default()
        })
        .unwrap();
}

fn index_pointers(db: &Database, session: &str) -> Vec<(Option<i64>, Option<String>)> {
    db.with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT log_seq, result_ref FROM tool_execution_log \
             WHERE session_id = ?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map([session], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .unwrap()
}

fn open_db(dir: &TempDir) -> Database {
    Database::open(&dir.path().join("index.db")).unwrap()
}

/// Losing an archived session's live segment loses its whole log, and a
/// reopened session restarts its `seq` at 1 — so a surviving `log_seq` would
/// name a *different* generation's record. The eviction clears the pointers
/// and leaves the audit half of the row alone.
#[test]
fn evicting_a_live_segment_clears_that_sessions_index_pointers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sessions");
    fs::create_dir_all(&root).unwrap();
    let db = open_db(&dir);
    seed_session(&root, "gone", 0, 1_000);
    seed_session(&root, "kept", 0, 1_000);
    // `gone` is the older, so the LRU takes it first.
    age(&root, "gone", 9_000);
    seed_index_row(&db, "gone", 7, "shell_execute");
    seed_index_row(&db, "kept", 3, "file_read");

    let active = std::collections::HashSet::new();
    // Enough to force `gone` out entirely and leave `kept` alone.
    let report = sweep::enforce_total_cap(&root, 2_000, &active, Some(&db)).unwrap();

    assert!(!root.join("gone").join(LIVE_SEGMENT).exists());
    assert!(root.join("kept").join(LIVE_SEGMENT).exists());
    assert_eq!(report.index_rows_cleared, 1);
    assert_eq!(index_pointers(&db, "gone"), vec![(None, None)]);
    assert_eq!(
        index_pointers(&db, "kept"),
        vec![(Some(3), Some("log:3".to_string()))],
        "a session that kept its live segment keeps its pointers"
    );

    // The audit half survives: that the tool ran is still true, and
    // `invocations_today` must not move because the disk filled up.
    let (agent, tool, preview): (String, String, Option<String>) = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT agent_id, tool_name, result_preview FROM tool_execution_log \
                 WHERE session_id = 'gone'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .unwrap();
    assert_eq!(agent, "lead_agent");
    assert_eq!(tool, "shell_execute");
    assert_eq!(preview.as_deref(), Some("ok"));
}

/// A session that only gives up its `results/` and rotated segments keeps its
/// live segment, so its `seq` never restarts and its pointers stay true.
#[test]
fn evicting_only_the_older_files_leaves_the_index_alone() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sessions");
    fs::create_dir_all(&root).unwrap();
    let db = open_db(&dir);
    seed_session(&root, "a", 2, 1_000);
    seed_index_row(&db, "a", 5, "web_fetch");

    let active = std::collections::HashSet::new();
    // 4 000 of the session's 5 000 bytes: the two spills and one rotated
    // segment go; the live segment stays.
    let report = sweep::enforce_total_cap(&root, 1_000, &active, Some(&db)).unwrap();
    assert!(root.join("a").join(LIVE_SEGMENT).exists());
    assert_eq!(report.index_rows_cleared, 0);
    assert_eq!(
        index_pointers(&db, "a"),
        vec![(Some(5), Some("log:5".to_string()))]
    );
}

/// **Write-first.** The de-indexing runs before the removal, and an eviction
/// that cannot de-index is abandoned: a row pointing at a record that is not
/// there is worse than a root that is still over its cap.
#[test]
fn a_live_segment_that_cannot_be_de_indexed_is_not_evicted() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sessions");
    fs::create_dir_all(&root).unwrap();
    let db = open_db(&dir);
    seed_session(&root, "a", 0, 1_000);
    seed_index_row(&db, "a", 7, "shell_execute");
    // The `UPDATE` can no longer run.
    db.with_connection(|conn| {
        conn.execute("DROP TABLE tool_execution_log", [])?;
        Ok(())
    })
    .unwrap();

    let active = std::collections::HashSet::new();
    let report = sweep::enforce_total_cap(&root, 1, &active, Some(&db)).unwrap();

    assert!(
        root.join("a").join(LIVE_SEGMENT).exists(),
        "the file the rows still describe must stay"
    );
    assert!(!root.join("a").join("log.1-9.jsonl").exists(), "the rotated one still goes");
    assert_eq!(report.index_rows_cleared, 0);
    assert!(report.over_cap_after, "and the pass says so rather than pretending");
}

// ── §5.6c: replaying an interrupted run's history ────────────────────

/// A `round` record as the loop writes one: the assistant's text plus its
/// `tool_use` blocks verbatim.
fn round_record(round: u32, text: &str, calls: &[(&str, &str, serde_json::Value)]) -> serde_json::Value {
    serde_json::json!({
        "round": round,
        "model": "stub-model",
        "input_tokens": 10,
        "output_tokens": 5,
        "cache_read_input_tokens": 0,
        "stop_reason": "ToolUse",
        "text": text,
        "tool_use": calls
            .iter()
            .map(|(id, name, input)| serde_json::json!({
                "id": id, "name": name, "input": input,
            }))
            .collect::<Vec<_>>(),
        "context": serde_json::Value::Null,
    })
}

/// A `tool_result` record with the result sitting inline.
fn tool_result_record(tool_use_id: &str, name: &str, ok: bool, result: &str) -> serde_json::Value {
    serde_json::json!({
        "tool_use_id": tool_use_id,
        "name": name,
        "ok": ok,
        "duration_ms": 12,
        "error": (!ok).then(|| result.to_string()),
        "result": result,
        "ext": serde_json::Value::Null,
    })
}

/// A `tool_result` record whose payload went to `results/` (T42's spill).
fn spilled_result_record(
    tool_use_id: &str,
    name: &str,
    rel: &str,
    bytes: usize,
    preview: &str,
) -> serde_json::Value {
    serde_json::json!({
        "tool_use_id": tool_use_id,
        "name": name,
        "ok": true,
        "duration_ms": 900,
        "error": serde_json::Value::Null,
        "result": {
            "spill": {"rel": rel, "bytes": bytes, "sha256": "deadbeef", "mime": "text/plain"},
            "preview": preview,
        },
        "result_ref": format!("file:{rel}"),
        "ext": serde_json::Value::Null,
    })
}

/// §5.6c's core: the loop's alternation comes back out of the log — the
/// assistant message with its `tool_use` blocks verbatim, then one
/// `tool_result` message per call, in the order the round made them.
#[test]
fn a_replay_rebuilds_the_rounds_and_their_results() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed_records(
        root,
        "s1",
        &[
            ("session_start", "t1", serde_json::json!({"boot_id": "b1"})),
            (
                "round",
                "t1",
                round_record(1, "let me look", &[("tu-1", "file_read", serde_json::json!({"path": "a.rs"}))]),
            ),
            ("tool_call", "t1", serde_json::json!({"tool_use_id": "tu-1", "name": "file_read"})),
            ("tool_result", "t1", tool_result_record("tu-1", "file_read", true, "fn main() {}")),
            (
                "round",
                "t1",
                round_record(2, "and now the tests", &[("tu-2", "shell_execute", serde_json::json!({"cmd": "cargo test"}))]),
            ),
            ("tool_result", "t1", tool_result_record("tu-2", "shell_execute", false, "[tool_error] boom")),
        ],
    );

    let plan = replay::rebuild(&root.join("s1"), "t1", 4).unwrap();

    assert_eq!(plan.rounds, 2);
    assert_eq!(plan.tool_results, 2);
    assert_eq!(plan.dropped_incomplete_rounds, 0);
    assert_eq!(plan.from_seq, Some(2), "the first round record");
    assert_eq!(plan.to_seq, Some(6), "the last result it consumed");

    let m = &plan.messages;
    assert_eq!(m.len(), 4, "assistant/result ×2: {m:?}");
    assert_eq!(m[0].role, openalpaca_llm::Role::Assistant);
    assert_eq!(m[0].content, "let me look");
    let calls = m[0].tool_calls.as_ref().expect("the round's tool_use, verbatim");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "tu-1");
    assert_eq!(calls[0].name, "file_read");
    assert_eq!(calls[0].arguments["path"], "a.rs");
    assert_eq!(m[1].role, openalpaca_llm::Role::Tool);
    assert_eq!(m[1].tool_call_id.as_deref(), Some("tu-1"));
    assert_eq!(m[1].content, "fn main() {}");
    assert_eq!(m[2].content, "and now the tests");
    assert_eq!(m[3].tool_call_id.as_deref(), Some("tu-2"));
    assert_eq!(m[3].content, "[tool_error] boom", "an Err comes back as it was");
}

/// A spilled result is replayed as the **stub the model actually saw** — the
/// bytes never went into the context the first time and must not now. When
/// the `results/` file is still there the stub's `read_result` promise still
/// holds; when the sweep has taken it, the replay says so instead.
#[test]
fn a_spilled_result_is_replayed_as_the_stub_the_model_saw() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let rel = "results/000004-shell_execute-tu-1.txt";
    seed_records(
        root,
        "s1",
        &[
            (
                "round",
                "t1",
                round_record(1, "", &[("tu-1", "shell_execute", serde_json::json!({"cmd": "cargo test"}))]),
            ),
            ("tool_result", "t1", spilled_result_record("tu-1", "shell_execute", rel, 204_812, "test result: FAILED")),
        ],
    );
    let session = root.join("s1");
    fs::create_dir_all(session.join(RESULTS_DIR)).unwrap();
    fs::write(session.join(rel), "the whole 200 KB").unwrap();

    let plan = replay::rebuild(&session, "t1", 4).unwrap();
    assert_eq!(plan.spills_referenced, 1);
    assert_eq!(plan.missing_spills, 0);
    let replayed = &plan.messages[1].content;
    assert_eq!(
        replayed,
        &spill_stub(204_812, "test result: FAILED", rel),
        "byte-for-byte the stub the loop handed the model"
    );

    // The sweep takes the file; the replay must not promise a page of it.
    fs::remove_file(session.join(rel)).unwrap();
    let plan = replay::rebuild(&session, "t1", 4).unwrap();
    assert_eq!(plan.missing_spills, 1);
    let replayed = &plan.messages[1].content;
    assert!(replayed.contains("test result: FAILED"), "the preview survives: {replayed}");
    assert!(!replayed.contains("read_result"), "but nothing to page: {replayed}");
}

/// §5.6c: "stop at the last *complete* round (all its results present — the
/// model re-does at most one round)". A crash between the call and its result
/// is exactly what a round with a missing `tool_result` is.
#[test]
fn an_incomplete_final_round_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed_records(
        root,
        "s1",
        &[
            ("round", "t1", round_record(1, "one", &[("tu-1", "file_read", serde_json::json!({}))])),
            ("tool_result", "t1", tool_result_record("tu-1", "file_read", true, "ok")),
            (
                "round",
                "t1",
                round_record(2, "two", &[
                    ("tu-2", "file_read", serde_json::json!({})),
                    ("tu-3", "shell_execute", serde_json::json!({})),
                ]),
            ),
            // Only one of the two results made it to disk before the crash.
            ("tool_result", "t1", tool_result_record("tu-2", "file_read", true, "half")),
        ],
    );

    let plan = replay::rebuild(&root.join("s1"), "t1", 4).unwrap();

    assert_eq!(plan.rounds, 1, "only the complete round is replayed");
    assert_eq!(plan.dropped_incomplete_rounds, 1);
    assert_eq!(plan.messages.len(), 2);
    assert!(
        plan.messages.iter().all(|m| m.tool_call_id.as_deref() != Some("tu-2")),
        "half a round is no round: {:?}",
        plan.messages
    );
}

/// **R67.** A resumed run appends to the *same* log under the *same* id, so
/// the round the first crash tore is an **interior** incomplete round by the
/// time a second resume reads the log. Truncating the history there threw away
/// every round the first resume completed — the precise failure this feature
/// exists to prevent, on the normal path, because every successful resume
/// leaves a torn round behind for the next one.
///
/// A resumed log is a sequence of incarnations separated by `resume` records:
/// the `resume` record closes the torn round before it, so the rebuild keeps
/// what came earlier and drops only the rounds nothing ever answered.
#[test]
fn two_resumes_in_a_row_replay_the_first_resumes_rounds() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed_records(
        root,
        "s1",
        &[
            // Incarnation 1: one complete round, then the crash's torn one.
            ("round", "t1", round_record(1, "one", &[("tu-1", "file_read", serde_json::json!({}))])),
            ("tool_result", "t1", tool_result_record("tu-1", "file_read", true, "ok")),
            ("round", "t1", round_record(2, "two", &[("tu-2", "shell_execute", serde_json::json!({}))])),
            // Resume #1 re-entered the run under the same id, in this log.
            (
                "resume",
                "t1",
                serde_json::json!({"from_seq": 1, "to_seq": 2, "rounds": 1}),
            ),
            // Incarnation 2: two complete rounds of real work…
            ("round", "t1", round_record(3, "three", &[("tu-3", "file_read", serde_json::json!({}))])),
            ("tool_result", "t1", tool_result_record("tu-3", "file_read", true, "ok")),
            ("round", "t1", round_record(4, "four", &[("tu-4", "artifact_write", serde_json::json!({}))])),
            ("tool_result", "t1", tool_result_record("tu-4", "artifact_write", true, "wrote report.md")),
            // …and its own torn round, the one this replay must re-do.
            ("round", "t1", round_record(5, "five", &[("tu-5", "shell_execute", serde_json::json!({}))])),
        ],
    );

    let plan = replay::rebuild(&root.join("s1"), "t1", 4).unwrap();

    let texts: Vec<&str> = plan
        .messages
        .iter()
        .filter(|m| m.role == openalpaca_llm::Role::Assistant)
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        texts,
        vec!["one", "three", "four"],
        "the first resume's own rounds must survive the second resume"
    );
    assert_eq!(plan.rounds, 3);
    assert_eq!(plan.tool_results, 3);
    assert_eq!(
        plan.dropped_incomplete_rounds, 2,
        "both tears are counted, so the gap is stated rather than inferred"
    );
    assert_eq!(plan.from_seq, Some(1));
    assert_eq!(plan.to_seq, Some(8), "the last result it consumed");
    // Provider validity survives: no assistant `tool_use` is left unanswered.
    assert!(
        plan.messages
            .iter()
            .filter_map(|m| m.tool_calls.as_ref())
            .flatten()
            .all(|call| plan
                .messages
                .iter()
                .any(|m| m.tool_call_id.as_deref() == Some(call.id.as_str()))),
        "every replayed call keeps its result: {:?}",
        plan.messages
    );
}

/// The same root cause from the other side: the session-log writer drops
/// records under backpressure (`sessions.dropped_records`), so one missing
/// `tool_result` in the middle of a long run leaves that round permanently
/// incomplete. It must cost that round, not every round after it — and if it
/// is the *first* round, a log full of work must not rebuild to nothing.
#[test]
fn a_round_whose_result_never_reached_the_log_costs_only_that_round() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed_records(
        root,
        "s1",
        &[
            // The dropped record is round 1's result.
            ("round", "t1", round_record(1, "one", &[("tu-1", "file_read", serde_json::json!({}))])),
            ("round", "t1", round_record(2, "two", &[("tu-2", "file_read", serde_json::json!({}))])),
            ("tool_result", "t1", tool_result_record("tu-2", "file_read", true, "ok")),
            ("round", "t1", round_record(3, "three", &[("tu-3", "file_read", serde_json::json!({}))])),
            ("tool_result", "t1", tool_result_record("tu-3", "file_read", true, "ok")),
        ],
    );

    let plan = replay::rebuild(&root.join("s1"), "t1", 4).unwrap();

    let texts: Vec<&str> = plan
        .messages
        .iter()
        .filter(|m| m.role == openalpaca_llm::Role::Assistant)
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(texts, vec!["two", "three"]);
    assert_eq!(plan.dropped_incomplete_rounds, 1);
}

/// The T55 hand-off note, honoured: `preserved_from_seq` is the last seq
/// before the compaction record, **not** the boundary of the tail compaction
/// retained — so the replay adds the retained tail explicitly, and says how
/// much of the head it dropped instead of silently re-inflating it.
#[test]
fn a_compaction_keeps_the_retained_tail_and_every_round_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut records: Vec<(&str, &str, serde_json::Value)> = Vec::new();
    let ids = ["tu-1", "tu-2", "tu-3", "tu-4"];
    let texts = ["one", "two", "three", "four"];
    for (i, (id, text)) in ids.iter().zip(texts).enumerate() {
        records.push((
            "round",
            "t1",
            round_record(i as u32 + 1, text, &[(id, "file_read", serde_json::json!({}))]),
        ));
        records.push(("tool_result", "t1", tool_result_record(id, "file_read", true, text)));
    }
    // seq 9: the compaction. `preserved_from_seq` = 8, the last seq written.
    records.push((
        "compaction",
        "t1",
        serde_json::json!({
            "tier": "HeuristicSummary",
            "trigger": "auto",
            "pre_tokens": 9000, "post_tokens": 3000,
            "messages_before": 12, "messages_after": 6,
            "messages_discarded": 6, "memories_extracted": 0,
            "tiers_applied": "[DiscardSocial, HeuristicSummary]",
            "cumulative_dropped_tokens": 6000,
            "dropped_from_seq": serde_json::Value::Null,
            "summary_msg_id": serde_json::Value::Null,
            "preserved_from_seq": 8,
        }),
    ));
    records.push((
        "round",
        "t1",
        round_record(5, "five", &[("tu-5", "file_read", serde_json::json!({}))]),
    ));
    records.push(("tool_result", "t1", tool_result_record("tu-5", "file_read", true, "five")));

    seed_records(root, "s1", &records);

    // tail_keep = 1: one round from before the boundary, plus everything after.
    let plan = replay::rebuild(&root.join("s1"), "t1", 1).unwrap();
    assert_eq!(plan.compacted_from_seq, Some(8));
    assert_eq!(plan.rounds, 2, "the retained tail (round 4) plus round 5");
    let texts: Vec<&str> = plan
        .messages
        .iter()
        .filter(|m| m.role == openalpaca_llm::Role::Assistant)
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(texts, vec!["four", "five"]);
    // The head that compaction dropped is named, never re-inflated and never
    // invented: the compaction record carries no summary text.
    let note = &plan.messages[0];
    assert_eq!(note.role, openalpaca_llm::Role::User);
    assert!(note.content.contains("context_compacted"), "{}", note.content);
    assert!(note.content.contains("3 earlier round"), "{}", note.content);

    // tail_keep large enough to cover everything: no head was dropped, so no
    // note is needed.
    let plan = replay::rebuild(&root.join("s1"), "t1", 8).unwrap();
    assert_eq!(plan.rounds, 5);
    assert!(
        !plan.messages[0].content.contains("context_compacted"),
        "nothing was dropped, so nothing is announced"
    );
}

/// A session holds every run started from that conversation (§5.1) — a
/// replay must never hand one run the rounds of the run beside it.
#[test]
fn the_replay_is_scoped_to_one_run_inside_a_shared_session() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed_records(
        root,
        "s1",
        &[
            ("round", "t1", round_record(1, "mine", &[("tu-1", "file_read", serde_json::json!({}))])),
            ("round", "t2", round_record(1, "theirs", &[("tu-2", "file_read", serde_json::json!({}))])),
            ("tool_result", "t2", tool_result_record("tu-2", "file_read", true, "theirs")),
            ("tool_result", "t1", tool_result_record("tu-1", "file_read", true, "mine")),
        ],
    );

    let plan = replay::rebuild(&root.join("s1"), "t1", 4).unwrap();
    assert_eq!(plan.rounds, 1);
    assert_eq!(plan.messages[0].content, "mine");
    assert_eq!(plan.messages[1].content, "mine");
}

/// §5.6c's "a gutted log is a clean 409": a replay of a run the log holds
/// nothing for rebuilds nothing, and says so, rather than inventing a start.
#[test]
fn a_log_with_nothing_for_this_run_rebuilds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed_records(
        root,
        "s1",
        &[("round", "t2", round_record(1, "theirs", &[]))],
    );

    let plan = replay::rebuild(&root.join("s1"), "t1", 4).unwrap();
    assert_eq!(plan.rounds, 0);
    assert!(plan.messages.is_empty());
    assert_eq!(plan.from_seq, None);

    // And a session directory that is not there at all is the same answer.
    let plan = replay::rebuild(&root.join("nope"), "t1", 4).unwrap();
    assert_eq!(plan.rounds, 0);
}

/// **Replay re-primes context; it never re-runs anything.** The recorded
/// calls come back as message history and nothing dispatches them — the
/// tool's own counter is the proof, because a replay that executed
/// `shell_execute` twice would be the worst bug this feature could have.
#[tokio::test]
async fn rebuilding_executes_no_tool() {
    use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counter(std::sync::Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl BuiltInTool for Counter {
        async fn execute(&self, _args: &serde_json::Value) -> Result<String, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok("ran".to_string())
        }
    }

    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let registry = crate::tools::ToolRegistry::default();
    registry
        .register(RegisteredTool {
            definition: openalpaca_llm::ToolDefinition {
                name: "shell_execute".to_string(),
                description: "counts".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                ..Default::default()
            },
            backend: ToolBackend::BuiltIn(std::sync::Arc::new(Counter(calls.clone()))),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test".to_string(),
            author: "test".to_string(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed_records(
        root,
        "s1",
        &[
            (
                "round",
                "t1",
                round_record(1, "", &[("tu-1", "shell_execute", serde_json::json!({"cmd": "rm -rf /"}))]),
            ),
            ("tool_result", "t1", tool_result_record("tu-1", "shell_execute", true, "done")),
        ],
    );

    let plan = replay::rebuild(&root.join("s1"), "t1", 4).unwrap();

    assert_eq!(plan.rounds, 1);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a replay re-primes the context — it must never dispatch a recorded call"
    );
    // The call is *described* to the model, which is the whole point.
    assert_eq!(plan.messages[0].tool_calls.as_ref().unwrap()[0].name, "shell_execute");
}
