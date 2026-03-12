use super::*;
use tempfile::tempdir;

fn setup_db() -> Database {
    let dir = tempdir().unwrap();
    Database::open(&dir.path().join("test.db")).unwrap()
}

#[test]
fn test_insert_and_get_call_log() {
    let db = setup_db();
    let repo = LlmUsageRepository::new(&db);

    let log = LlmCallLog {
        id: None,
        timestamp: Utc::now(),
        agent_id: Some("agent1".to_string()),
        task_id: Some("task1".to_string()),
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-5-20250929".to_string(),
        key_id: Some("key1".to_string()),
        input_tokens: 100,
        output_tokens: 50,
        cost_usd: 0.001,
        status: "success".to_string(),
        latency_ms: Some(250),
        error_message: None,
    };

    let id = repo.insert_call_log(&log).unwrap();
    assert!(id > 0);

    let logs = repo.get_agent_usage("agent1", 10).unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].model, "claude-sonnet-4-5-20250929");
    assert_eq!(logs[0].input_tokens, 100);
    assert_eq!(logs[0].cost_usd, 0.001);
}

#[test]
fn test_get_task_usage() {
    let db = setup_db();
    let repo = LlmUsageRepository::new(&db);

    let log = LlmCallLog {
        id: None,
        timestamp: Utc::now(),
        agent_id: Some("agent1".to_string()),
        task_id: Some("task1".to_string()),
        provider: "openai".to_string(),
        model: "gpt-4o".to_string(),
        key_id: None,
        input_tokens: 200,
        output_tokens: 100,
        cost_usd: 0.002,
        status: "success".to_string(),
        latency_ms: Some(500),
        error_message: None,
    };

    repo.insert_call_log(&log).unwrap();

    let logs = repo.get_task_usage("task1", 10).unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].provider, "openai");
}

#[test]
fn test_daily_usage_upsert() {
    let db = setup_db();
    let repo = LlmUsageRepository::new(&db);

    let usage = LlmUsageDaily {
        date: "2025-01-15".to_string(),
        agent_id: "agent1".to_string(),
        model: "claude-sonnet-4-5-20250929".to_string(),
        total_requests: 5,
        total_input_tokens: 1000,
        total_output_tokens: 500,
        total_cost_usd: 0.01,
    };

    repo.upsert_daily_usage(&usage).unwrap();

    // Upsert again — should accumulate
    repo.upsert_daily_usage(&usage).unwrap();

    let daily = repo.get_daily_usage("agent1", 10).unwrap();
    assert_eq!(daily.len(), 1);
    assert_eq!(daily[0].total_requests, 10); // 5 + 5
    assert_eq!(daily[0].total_input_tokens, 2000); // 1000 + 1000
}

#[test]
fn test_daily_usage_replace() {
    let db = setup_db();
    let repo = LlmUsageRepository::new(&db);

    let usage = LlmUsageDaily {
        date: "2025-01-15".to_string(),
        agent_id: "agent1".to_string(),
        model: "claude-sonnet-4-5-20250929".to_string(),
        total_requests: 5,
        total_input_tokens: 1000,
        total_output_tokens: 500,
        total_cost_usd: 0.01,
    };

    // Upsert twice — additive: 5+5=10
    repo.upsert_daily_usage(&usage).unwrap();
    repo.upsert_daily_usage(&usage).unwrap();

    // Replace with 3 — should overwrite, not accumulate
    let replacement = LlmUsageDaily {
        total_requests: 3,
        total_input_tokens: 600,
        total_output_tokens: 300,
        total_cost_usd: 0.006,
        ..usage
    };
    repo.replace_daily_usage(&replacement).unwrap();

    let daily = repo.get_daily_usage("agent1", 10).unwrap();
    assert_eq!(daily.len(), 1);
    assert_eq!(daily[0].total_requests, 3);
    assert_eq!(daily[0].total_input_tokens, 600);
    assert_eq!(daily[0].total_output_tokens, 300);
    assert!((daily[0].total_cost_usd - 0.006).abs() < 1e-9);
}

#[test]
fn test_empty_results() {
    let db = setup_db();
    let repo = LlmUsageRepository::new(&db);

    let logs = repo.get_agent_usage("nonexistent", 10).unwrap();
    assert!(logs.is_empty());

    let daily = repo.get_daily_usage("nonexistent", 10).unwrap();
    assert!(daily.is_empty());
}

#[test]
fn test_schema_version() {
    let db = setup_db();
    assert_eq!(db.schema_version().unwrap(), 32);
}
