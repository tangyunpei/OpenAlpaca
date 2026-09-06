use super::*;
use super::persona::{ensure_soul_file, ensure_soul_template_file};
use openalpaca_core::middleware::prompt::SystemPersona;
use std::path::PathBuf;

fn make_temp_dir(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("{}-{}", prefix, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("temp dir should be creatable");
    dir
}

#[test]
fn test_bootstrap_system_persona_creates_template_and_soul() {
    let dir = make_temp_dir("openalpaca-soul-bootstrap");
    let (persona, soul_path) = bootstrap_system_persona(&dir);

    assert_eq!(persona.name, "OpenAlpaca");
    assert!(soul_path.exists());
    assert!(
        dir.join("orchestrator")
            .join("templates")
            .join("SOUL_temp.md")
            .exists()
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_bootstrap_system_persona_falls_back_on_invalid_soul() {
    let dir = make_temp_dir("openalpaca-soul-invalid");
    let template_path = ensure_soul_template_file(&dir).expect("template should bootstrap");
    let soul_path = ensure_soul_file(&dir, &template_path).expect("soul file should bootstrap");

    std::fs::write(&soul_path, "invalid").expect("test should write invalid soul");
    let (persona, loaded_path) = bootstrap_system_persona(&dir);

    assert_eq!(loaded_path, soul_path);
    assert_eq!(persona.name, SystemPersona::default().name);
    assert_eq!(persona.core_values, SystemPersona::default().core_values);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_is_same_file_path_matches_identical_files() {
    let dir = make_temp_dir("openalpaca-soul-path");
    let file = dir.join("SOUL.md");
    std::fs::write(&file, "x").expect("test file should be writable");

    let canonical = std::fs::canonicalize(&file).expect("file should canonicalize");
    assert!(is_same_file_path(&file, &canonical));

    let _ = std::fs::remove_dir_all(dir);
}

/// GAP-09 — a daemon killed mid-run leaves a task `running` and its lanes
/// open. The boot sequence sweeps the task first, which is what makes the
/// span sweep's condition true; running them in that order reports every
/// abandoned lane as `cancelled` / `"interrupted"`, and a second boot
/// (the idempotence the sweep promises) changes nothing.
#[test]
fn the_boot_sweeps_report_abandoned_lanes_as_interrupted() {
    use openalpaca_storage::{Database, NewSubagentSpan, SubagentSpanRepository};

    let dir = make_temp_dir("openalpaca-span-sweep");
    let db = Database::open(&dir.join("test.db")).expect("open db");
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO task (id, title, status, priority, created_by, source_lane)
             VALUES ('t1', 'a run', 'running', 0, 'tester', 'user:cli')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    SubagentSpanRepository::new(&db)
        .open(NewSubagentSpan {
            id: "n1",
            task_id: "t1",
            template_id: "review_agent",
            agent_instance_id: "review_agent::a",
            objective: None,
        })
        .expect("open span");

    // The span sweep alone must not touch a lane whose task is still running.
    close_orphaned_spans(&db);
    let spans = SubagentSpanRepository::new(&db)
        .list_for_task("t1")
        .unwrap();
    assert_eq!(spans[0].state, "running");

    // The real boot order: tasks first, then their lanes.
    sweep_interrupted_runs(&db, None, "instance-under-test");
    // §5.6b — the run is `interrupted`, not `failed`, and the detail names the
    // incarnation that found it.
    let task = openalpaca_storage::repository::TaskRepository::new(&db)
        .get("t1")
        .unwrap()
        .unwrap();
    assert_eq!(task.status, openalpaca_storage::TaskStatus::Interrupted);
    assert!(
        task.result_summary
            .as_deref()
            .is_some_and(|s| s.contains("instance-under-test")),
        "{:?}",
        task.result_summary
    );
    close_orphaned_spans(&db);
    let spans = SubagentSpanRepository::new(&db)
        .list_for_task("t1")
        .unwrap();
    assert_eq!(spans[0].state, "cancelled");
    assert_eq!(spans[0].detail.as_deref(), Some("interrupted"));
    assert!(spans[0].ended_at.is_some());

    // Idempotent: the next boot leaves the row exactly as it is.
    let ended_at = spans[0].ended_at.clone();
    close_orphaned_spans(&db);
    let spans = SubagentSpanRepository::new(&db)
        .list_for_task("t1")
        .unwrap();
    assert_eq!(spans[0].ended_at, ended_at);

    drop(db);
    let _ = std::fs::remove_dir_all(dir);
}

// ── §5.6b: recovery, ordering, idempotence ──────────────────────────

use openalpaca_storage::repository::{FollowupRepository, TaskRepository};
use openalpaca_storage::{Database, TaskStatus};

/// One `running` run in `session`, with the shape the dispatcher writes.
fn seed_run(db: &Database, task_id: &str, session: &str, lane: &str) {
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO session (id, lane_key, source, status) VALUES (?1, ?2, 'gui', 'archived')",
            (session, lane),
        )?;
        conn.execute(
            "INSERT INTO task (id, title, status, priority, created_by, source_lane, session_id)
             VALUES (?1, 'a run', 'running', 0, 'tester', ?2, ?3)",
            (task_id, lane, session),
        )?;
        Ok(())
    })
    .unwrap();
}

/// The session log a `kill -9` leaves behind: two pushes, one of them drained
/// at the round boundary, and no `workflow_done`.
fn seed_crashed_log(root: &std::path::Path, session: &str, task_id: &str) {
    let dir = root.join(session);
    std::fs::create_dir_all(&dir).unwrap();
    let steering = |seq: u64, id: &str, text: &str| {
        serde_json::json!({
            "v": 1, "seq": seq, "ts": "2026-09-06T10:00:00.000Z",
            "type": "steering", "task_id": task_id,
            "data": {
                "request_id": id, "lane_key": "user:cli", "text": text,
                "received_at": "2026-09-06T10:00:00+00:00", "queue_depth": 1,
                "principal": {"User": {"global_id": "u-42"}},
                "workspace_path": "/repo",
            }
        })
        .to_string()
    };
    let drained = serde_json::json!({
        "v": 1, "seq": 3, "ts": "2026-09-06T10:00:01.000Z",
        "type": "steering_drained", "task_id": task_id,
        "data": {"at": "round_boundary", "round": 1, "count": 1, "request_ids": ["r-1"]}
    })
    .to_string();
    let body = format!(
        "{}\n{}\n{}\n",
        steering(1, "r-1", "delivered"),
        steering(2, "r-2", "never seen"),
        drained
    );
    std::fs::write(dir.join("log.jsonl"), body).unwrap();
}

fn queued_contents(db: &Database, lane: &str) -> Vec<String> {
    FollowupRepository::new(db)
        .list_queued_by_lane(lane)
        .unwrap()
        .into_iter()
        .map(|r| r.content)
        .collect()
}

/// §5.6b end to end: a run left `running` becomes `interrupted`, and the
/// interjection no `steering_drained` names is re-queued as an
/// `unprocessed_steering` follow-up. The one that *was* drained is not — the
/// model already saw it.
#[test]
fn the_boot_sweep_marks_the_run_interrupted_and_recovers_its_undrained_steering() {
    let dir = make_temp_dir("openalpaca-interrupted");
    let db = Database::open(&dir.join("test.db")).unwrap();
    let sessions = dir.join("sessions");
    seed_run(&db, "t1", "s1", "user:cli");
    seed_crashed_log(&sessions, "s1", "t1");

    let runs = sweep_interrupted_runs(&db, Some(&sessions), "instance-7");

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].task_id, "t1");
    assert_eq!(runs[0].session_id.as_deref(), Some("s1"));
    assert_eq!(runs[0].recovered_steering, 1);

    let task = TaskRepository::new(&db).get("t1").unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Interrupted);
    assert!(task.status.is_terminal());
    assert!(
        task.result_summary
            .as_deref()
            .is_some_and(|s| s.contains("instance-7"))
    );

    assert_eq!(queued_contents(&db, "user:cli"), vec!["never seen"]);
    let row = &FollowupRepository::new(&db)
        .list_queued_by_lane("user:cli")
        .unwrap()[0];
    assert_eq!(row.kind, "unprocessed_steering");
    assert_eq!(row.source_task_id.as_deref(), Some("t1"));
    assert_eq!(row.session_id.as_deref(), Some("s1"));

    drop(db);
    let _ = std::fs::remove_dir_all(dir);
}

/// Twice, and only once. The second boot's `list_non_terminal` is empty
/// because `interrupted` is terminal — and even a *third* pass driven straight
/// at the same log (the crash window between the recovery and the flip) adds
/// nothing, because the row's own presence is the marker.
#[test]
fn a_second_boot_recovers_the_same_interjection_no_second_time() {
    let dir = make_temp_dir("openalpaca-interrupted-twice");
    let db = Database::open(&dir.join("test.db")).unwrap();
    let sessions = dir.join("sessions");
    seed_run(&db, "t1", "s1", "user:cli");
    seed_crashed_log(&sessions, "s1", "t1");

    assert_eq!(
        sweep_interrupted_runs(&db, Some(&sessions), "boot-1")[0].recovered_steering,
        1
    );
    // Boot two: nothing is in flight any more, so nothing is even read.
    assert!(sweep_interrupted_runs(&db, Some(&sessions), "boot-2").is_empty());
    assert_eq!(queued_contents(&db, "user:cli"), vec!["never seen"]);

    // The crash window: the recovery ran, the flip did not. Put the row back
    // to `running` and sweep again — still one follow-up.
    db.with_connection(|conn| {
        conn.execute("UPDATE task SET status = 'running' WHERE id = 't1'", [])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        sweep_interrupted_runs(&db, Some(&sessions), "boot-3")[0].recovered_steering,
        0
    );
    assert_eq!(queued_contents(&db, "user:cli"), vec!["never seen"]);

    drop(db);
    let _ = std::fs::remove_dir_all(dir);
}

/// The order the brief fixes: **recovery before the byte-cap eviction**. The
/// interjection exists only in the JSONL, and R54 lets the sweep take an
/// archived session's live segment — so running the sweep first loses it.
///
/// Both halves are asserted on the same fixture, twice, so the assertion is
/// about the order and not about the fixture.
#[test]
fn the_recovery_runs_before_the_byte_cap_eviction_or_the_interjection_is_gone() {
    use openalpaca_core::session_log::sweep;

    let empty = std::collections::HashSet::new();

    // (a) The boot order as it ships: recover, then evict.
    let dir = make_temp_dir("openalpaca-order-right");
    let db = Database::open(&dir.join("test.db")).unwrap();
    let sessions = dir.join("sessions");
    seed_run(&db, "t1", "s1", "user:cli");
    seed_crashed_log(&sessions, "s1", "t1");

    sweep_interrupted_runs(&db, Some(&sessions), "boot-1");
    // A cap of 0 bytes: the session is archived, so R54 lets its live segment
    // go, and the log is gone.
    sweep::enforce_total_cap(&sessions, 0, &empty).unwrap();
    assert!(!sessions.join("s1").join("log.jsonl").exists());
    assert_eq!(queued_contents(&db, "user:cli"), vec!["never seen"]);

    drop(db);
    let _ = std::fs::remove_dir_all(dir);

    // (b) The same fixture with the two passes swapped: the eviction takes the
    // only copy of the interjection, and the recovery finds nothing.
    let dir = make_temp_dir("openalpaca-order-wrong");
    let db = Database::open(&dir.join("test.db")).unwrap();
    let sessions = dir.join("sessions");
    seed_run(&db, "t1", "s1", "user:cli");
    seed_crashed_log(&sessions, "s1", "t1");

    sweep::enforce_total_cap(&sessions, 0, &empty).unwrap();
    assert!(
        !sessions.join("s1").join("log.jsonl").exists(),
        "the eviction really took the only copy"
    );
    let runs = sweep_interrupted_runs(&db, Some(&sessions), "boot-1");
    assert_eq!(runs[0].recovered_steering, 0);
    assert!(queued_contents(&db, "user:cli").is_empty());
    // The run is still honestly marked — the crash happened either way.
    assert_eq!(
        TaskRepository::new(&db).get("t1").unwrap().unwrap().status,
        TaskStatus::Interrupted
    );

    drop(db);
    let _ = std::fs::remove_dir_all(dir);
}

/// A run with no session (a lane that never had one, or a pre-039 row) is
/// still marked, has nothing to recover, and is not announced.
#[test]
fn a_run_with_no_session_is_marked_but_announces_nothing() {
    let dir = make_temp_dir("openalpaca-interrupted-nosession");
    let db = Database::open(&dir.join("test.db")).unwrap();
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO task (id, title, status, priority, created_by, source_lane)
             VALUES ('t1', 'a run', 'queued', 0, 'tester', 'user:cli')",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let runs = sweep_interrupted_runs(&db, Some(&dir.join("sessions")), "boot-1");
    assert_eq!(runs.len(), 1);
    assert!(runs[0].session_id.is_none());

    let bus = openalpaca_core::bus::EventBus::default();
    let mut rx = bus.subscribe();
    announce_interrupted(&bus, &runs);
    assert!(rx.try_recv().is_err(), "no session, nothing to announce in");

    drop(db);
    let _ = std::fs::remove_dir_all(dir);
}

/// The frame §5.7 gained: `SessionChanged` with `status = "interrupted"` and
/// the run named, so a window open across the restart learns that the card it
/// was watching will never finish.
#[test]
fn an_interrupted_run_is_announced_on_its_session() {
    let dir = make_temp_dir("openalpaca-interrupted-announce");
    let db = Database::open(&dir.join("test.db")).unwrap();
    seed_run(&db, "t1", "s1", "user:cli");
    let runs = sweep_interrupted_runs(&db, None, "boot-1");

    let bus = openalpaca_core::bus::EventBus::default();
    let mut rx = bus.subscribe();
    announce_interrupted(&bus, &runs);

    match rx.try_recv().expect("one frame") {
        openalpaca_core::events::SystemEvent::SessionChanged {
            session_id,
            lane_key,
            status,
            task_id,
            ..
        } => {
            assert_eq!(session_id, "s1");
            assert_eq!(lane_key, "user:cli");
            assert_eq!(status, "interrupted");
            assert_eq!(task_id.as_deref(), Some("t1"));
        }
        other => panic!("expected SessionChanged, got {other:?}"),
    }

    drop(db);
    let _ = std::fs::remove_dir_all(dir);
}
