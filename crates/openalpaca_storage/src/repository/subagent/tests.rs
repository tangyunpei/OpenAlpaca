use super::*;
use crate::models::task::{Task, TaskStatus};
use crate::repository::task::TaskRepository;
use tempfile::tempdir;

fn setup_db() -> Database {
    let dir = tempdir().unwrap();
    Database::open(&dir.path().join("test.db")).unwrap()
}

fn make_config(id: &str, name: &str) -> SubAgentConfig {
    SubAgentConfig {
        id: id.to_string(),
        template_id: id.to_string(),
        name: name.to_string(),
        description: Some("A test agent".to_string()),
        icon: None,
        status: "idle".to_string(),
        current_task_id: None,
        skills_json: r#"[{"name":"web_search","category":"research","proficiency":0.9}]"#
            .to_string(),
        preset_json: r#"{"persona":"test","temperature":0.5,"verbosity":"normal"}"#.to_string(),
        constraints_json: None,
        llm_config_json: None,
        persona: Some("Test persona".to_string()),
        created_at: Utc::now(),
        updated_at: None,
    }
}

fn make_task(id: &str) -> Task {
    let now = Utc::now();
    Task {
        id: id.to_string(),
        title: "Test Task".to_string(),
        description: None,
        status: TaskStatus::Queued,
        priority: 0,
        progress_current: None,
        progress_total: None,
        result_summary: None,
        created_by: "test".to_string(),
        source_lane: "cli".to_string(),
        created_at: now,
        updated_at: now,
        completed_at: None,
        state_json: None,
        state_version: 0,
        outcome_json: None,
        outcome_kind: None,
        artifact_count: 0,
    }
}

#[test]
fn test_upsert_and_get() {
    let db = setup_db();
    let repo = SubAgentRepository::new(&db);

    let config = make_config("sa1", "Research Agent");
    repo.upsert(&config).unwrap();

    let fetched = repo.get("sa1").unwrap().unwrap();
    assert_eq!(fetched.id, "sa1");
    assert_eq!(fetched.template_id, "sa1");
    assert_eq!(fetched.name, "Research Agent");
    assert_eq!(fetched.status, "idle");
    assert_eq!(fetched.description.as_deref(), Some("A test agent"));
}

#[test]
fn test_upsert_updates_existing() {
    let db = setup_db();
    let repo = SubAgentRepository::new(&db);

    let mut config = make_config("sa1", "V1");
    repo.upsert(&config).unwrap();

    config.name = "V2".to_string();
    config.description = Some("Updated".to_string());
    repo.upsert(&config).unwrap();

    let fetched = repo.get("sa1").unwrap().unwrap();
    assert_eq!(fetched.name, "V2");
    assert_eq!(fetched.description.as_deref(), Some("Updated"));
}

#[test]
fn test_get_nonexistent() {
    let db = setup_db();
    let repo = SubAgentRepository::new(&db);
    assert!(repo.get("nope").unwrap().is_none());
}

#[test]
fn test_list() {
    let db = setup_db();
    let repo = SubAgentRepository::new(&db);

    repo.upsert(&make_config("sa1", "Agent 1")).unwrap();
    repo.upsert(&make_config("sa2", "Agent 2")).unwrap();

    let all = repo.list(10).unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn test_list_by_status() {
    let db = setup_db();
    let repo = SubAgentRepository::new(&db);

    repo.upsert(&make_config("sa1", "Agent 1")).unwrap();
    repo.upsert(&make_config("sa2", "Agent 2")).unwrap();
    repo.update_status("sa2", "busy", Some("task-1")).unwrap();

    let idle = repo.list_by_status("idle", 10).unwrap();
    assert_eq!(idle.len(), 1);
    assert_eq!(idle[0].id, "sa1");

    let busy = repo.list_by_status("busy", 10).unwrap();
    assert_eq!(busy.len(), 1);
    assert_eq!(busy[0].id, "sa2");
}

#[test]
fn test_update_status() {
    let db = setup_db();
    let repo = SubAgentRepository::new(&db);

    repo.upsert(&make_config("sa1", "Agent")).unwrap();

    assert!(repo.update_status("sa1", "busy", Some("task-1")).unwrap());
    let fetched = repo.get("sa1").unwrap().unwrap();
    assert_eq!(fetched.status, "busy");
    assert_eq!(fetched.current_task_id.as_deref(), Some("task-1"));

    assert!(!repo.update_status("nope", "idle", None).unwrap());
}

#[test]
fn test_delete() {
    let db = setup_db();
    let repo = SubAgentRepository::new(&db);

    repo.upsert(&make_config("sa1", "Agent")).unwrap();
    repo.delete("sa1").unwrap();
    assert!(repo.get("sa1").unwrap().is_none());
}

#[test]
fn test_metrics_upsert_and_get() {
    let db = setup_db();
    let repo = SubAgentRepository::new(&db);

    repo.upsert(&make_config("sa1", "Agent")).unwrap();

    let metrics = AgentMetrics::new_empty("sa1");
    repo.upsert_metrics(&metrics).unwrap();

    let fetched = repo.get_metrics("sa1").unwrap().unwrap();
    assert_eq!(fetched.agent_id, "sa1");
    assert_eq!(fetched.tasks_completed, 0);
    assert_eq!(fetched.success_rate, 1.0);
}

#[test]
fn test_metrics_increment_completed() {
    let db = setup_db();
    let repo = SubAgentRepository::new(&db);

    repo.upsert(&make_config("sa1", "Agent")).unwrap();
    repo.upsert_metrics(&AgentMetrics::new_empty("sa1"))
        .unwrap();

    repo.increment_completed("sa1", 120).unwrap();

    let fetched = repo.get_metrics("sa1").unwrap().unwrap();
    assert_eq!(fetched.tasks_completed, 1);
    assert_eq!(fetched.total_runtime_seconds, 120);
    assert_eq!(fetched.average_runtime_seconds, 120.0);
    assert_eq!(fetched.success_rate, 1.0);
}

#[test]
fn test_metrics_increment_failed() {
    let db = setup_db();
    let repo = SubAgentRepository::new(&db);

    repo.upsert(&make_config("sa1", "Agent")).unwrap();
    repo.upsert_metrics(&AgentMetrics::new_empty("sa1"))
        .unwrap();

    // Complete one, fail one
    repo.increment_completed("sa1", 60).unwrap();
    repo.increment_failed("sa1").unwrap();

    let fetched = repo.get_metrics("sa1").unwrap().unwrap();
    assert_eq!(fetched.tasks_completed, 1);
    assert_eq!(fetched.tasks_failed, 1);
    assert_eq!(fetched.success_rate, 0.5);
}

#[test]
fn test_history_add_and_get() {
    let db = setup_db();
    let repo = SubAgentRepository::new(&db);
    let task_repo = TaskRepository::new(&db);

    repo.upsert(&make_config("sa1", "Agent")).unwrap();
    task_repo.create(&make_task("t1")).unwrap();

    let entry = AgentTaskHistory {
        id: "h1".to_string(),
        agent_id: "sa1".to_string(),
        task_id: "t1".to_string(),
        role: "executor".to_string(),
        status: "completed".to_string(),
        runtime_seconds: Some(45),
        completed_at: Utc::now(),
    };
    repo.add_history(&entry).unwrap();

    let history = repo.get_history("sa1", 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, "executor");
    assert_eq!(history[0].runtime_seconds, Some(45));
}
