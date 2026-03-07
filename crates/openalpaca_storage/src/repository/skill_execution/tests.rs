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
fn test_record_and_get_by_skill() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    let entry = make_skill_entry("req-1", "code-review", "success");
    let id = repo.record(&entry).unwrap();
    assert!(id > 0);

    let results = repo.get_by_skill("code-review", 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].request_id, "req-1");
    assert_eq!(results[0].skill_id, "code-review");
    assert_eq!(results[0].status, "success");
    assert_eq!(results[0].duration_ms, 1500);
    assert_eq!(results[0].rounds_used, Some(3));
    assert!(results[0].was_auto_selected);
    assert!(!results[0].repair_attempted);
}

#[test]
fn test_record_tool() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    let entry = ToolExecutionEntry {
        id: None,
        request_id: Some("req-1".to_string()),
        agent_id: "orchestrator".to_string(),
        tool_name: "web_fetch".to_string(),
        success: true,
        duration_ms: 250,
        error_message: None,
        timestamp: None,
    };
    let id = repo.record_tool(&entry).unwrap();
    assert!(id > 0);
}

#[test]
fn test_get_by_skill_limit() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    for i in 0..5 {
        let entry = make_skill_entry(&format!("req-{i}"), "summarize", "success");
        repo.record(&entry).unwrap();
    }

    let results = repo.get_by_skill("summarize", 3).unwrap();
    assert_eq!(results.len(), 3);
}

#[test]
fn test_get_by_skill_empty() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    let results = repo.get_by_skill("nonexistent", 10).unwrap();
    assert!(results.is_empty());
}

#[test]
fn test_cleanup_old() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    // Insert entries (they get current timestamp, so cleanup with 0 days should remove them)
    let entry = make_skill_entry("req-cleanup", "test-skill", "success");
    repo.record(&entry).unwrap();

    let tool_entry = ToolExecutionEntry {
        id: None,
        request_id: Some("req-cleanup".to_string()),
        agent_id: "orchestrator".to_string(),
        tool_name: "shell".to_string(),
        success: true,
        duration_ms: 100,
        error_message: None,
        timestamp: None,
    };
    repo.record_tool(&tool_entry).unwrap();

    // Cleanup with very large retention — nothing deleted
    let (s, t) = repo.cleanup_old(9999, 9999).unwrap();
    assert_eq!(s, 0);
    assert_eq!(t, 0);

    // Verify rows still exist
    let results = repo.get_by_skill("test-skill", 10).unwrap();
    assert_eq!(results.len(), 1);
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

    let results = repo.get_by_skill("failing-skill", 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, "error");
    assert_eq!(results[0].error_message, Some("LLM timeout".to_string()));
    assert!(results[0].repair_attempted);
    assert!(!results[0].repair_succeeded);
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
fn test_skill_health_single() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    let entry = make_skill_entry("req-h1", "test-skill", "success");
    repo.record(&entry).unwrap();

    let health = repo.skill_health("test-skill").unwrap();
    assert!(health.is_some());
    let h = health.unwrap();
    assert_eq!(h.skill_id, "test-skill");
    assert_eq!(h.total_invocations, 1);

    // Nonexistent skill
    let none = repo.skill_health("nonexistent").unwrap();
    assert!(none.is_none());
}

#[test]
fn test_all_skill_health_empty() {
    let db = setup_db();
    let repo = SkillExecutionRepository::new(&db);

    let health = repo.all_skill_health().unwrap();
    assert!(health.is_empty());
}
