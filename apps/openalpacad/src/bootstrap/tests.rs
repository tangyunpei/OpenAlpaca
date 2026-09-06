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
    sweep_orphaned_tasks(&db, "instance-under-test");
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
