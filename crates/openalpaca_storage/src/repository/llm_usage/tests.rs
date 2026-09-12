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
fn test_query_daily_usage_date_filter() {
    let db = setup_db();
    let repo = LlmUsageRepository::new(&db);

    let day1 = LlmUsageDaily {
        date: "2026-08-29".to_string(),
        agent_id: "agent1".to_string(),
        model: "claude-sonnet-4-5-20250929".to_string(),
        total_requests: 5,
        total_input_tokens: 1000,
        total_output_tokens: 500,
        total_cost_usd: 0.01,
    };
    let day2 = LlmUsageDaily {
        date: "2026-08-30".to_string(),
        total_requests: 7,
        ..day1.clone()
    };
    repo.upsert_daily_usage(&day1).unwrap();
    repo.upsert_daily_usage(&day2).unwrap();

    // No date filter: both rows, newest first.
    let all = repo.query_daily_usage(None, None, 10).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].date, "2026-08-30");

    // Date filter selects exactly the matching day.
    let d1 = repo.query_daily_usage(None, Some("2026-08-29"), 10).unwrap();
    assert_eq!(d1.len(), 1);
    assert_eq!(d1[0].date, "2026-08-29");
    assert_eq!(d1[0].total_requests, 5);

    let d2 = repo.query_daily_usage(None, Some("2026-08-30"), 10).unwrap();
    assert_eq!(d2.len(), 1);
    assert_eq!(d2[0].total_requests, 7);

    // Combined agent + date filter.
    let combined = repo
        .query_daily_usage(Some("agent1"), Some("2026-08-30"), 10)
        .unwrap();
    assert_eq!(combined.len(), 1);
    assert_eq!(combined[0].date, "2026-08-30");

    // Non-matching filters return nothing.
    assert!(repo
        .query_daily_usage(None, Some("2026-01-01"), 10)
        .unwrap()
        .is_empty());
    assert!(repo
        .query_daily_usage(Some("other-agent"), Some("2026-08-30"), 10)
        .unwrap()
        .is_empty());
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
    assert_eq!(db.schema_version().unwrap(), 41);
}

fn call_log_for_task(task_id: &str, cost_usd: f64) -> LlmCallLog {
    LlmCallLog {
        id: None,
        timestamp: Utc::now(),
        agent_id: Some("orchestrator".to_string()),
        task_id: Some(task_id.to_string()),
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-5-20250929".to_string(),
        key_id: None,
        input_tokens: 10,
        output_tokens: 5,
        cost_usd,
        status: "success".to_string(),
        latency_ms: Some(100),
        error_message: None,
    }
}

#[test]
fn test_cost_for_tasks_sums_multiple_calls_per_task() {
    let db = setup_db();
    let repo = LlmUsageRepository::new(&db);

    repo.insert_call_log(&call_log_for_task("task1", 0.01)).unwrap();
    repo.insert_call_log(&call_log_for_task("task1", 0.02)).unwrap();
    repo.insert_call_log(&call_log_for_task("task2", 0.05)).unwrap();
    // A call with no task_id must not leak into either total.
    repo.insert_call_log(&LlmCallLog {
        task_id: None,
        ..call_log_for_task("unused", 99.0)
    })
    .unwrap();

    let costs = repo
        .cost_for_tasks(&["task1".to_string(), "task2".to_string()])
        .unwrap();

    assert_eq!(costs.len(), 2);
    assert!((costs["task1"] - 0.03).abs() < 1e-9);
    assert!((costs["task2"] - 0.05).abs() < 1e-9);
}

#[test]
fn test_cost_for_tasks_omits_tasks_with_no_logged_cost() {
    let db = setup_db();
    let repo = LlmUsageRepository::new(&db);

    repo.insert_call_log(&call_log_for_task("task1", 0.01)).unwrap();

    let costs = repo
        .cost_for_tasks(&["task1".to_string(), "task-with-no-calls".to_string()])
        .unwrap();

    assert_eq!(costs.len(), 1);
    assert!(costs.contains_key("task1"));
    assert!(!costs.contains_key("task-with-no-calls"));
}

#[test]
fn test_cost_for_tasks_empty_input_returns_empty_map_without_querying() {
    let db = setup_db();
    let repo = LlmUsageRepository::new(&db);
    repo.insert_call_log(&call_log_for_task("task1", 0.01)).unwrap();

    let costs = repo.cost_for_tasks(&[]).unwrap();
    assert!(costs.is_empty());
}

// ── today's per-provider figures (GAP-08c, T50) ─────────────────────────────

fn call(provider: &str, cost_usd: f64, input: i32, output: i32) -> LlmCallLog {
    LlmCallLog {
        id: None,
        timestamp: Utc::now(),
        agent_id: Some("agent1".to_string()),
        task_id: None,
        provider: provider.to_string(),
        model: "m".to_string(),
        key_id: None,
        input_tokens: input,
        output_tokens: output,
        cost_usd,
        status: "success".to_string(),
        latency_ms: Some(1),
        error_message: None,
    }
}

/// `GET /v1/usage/summary`'s `by_provider`: today's call rows grouped once,
/// never the lifetime `all_provider_usage()` the Settings panel used to show.
#[test]
fn provider_usage_since_groups_todays_calls_by_provider() {
    let db = setup_db();
    let repo = LlmUsageRepository::new(&db);

    repo.insert_call_log(&call("anthropic", 0.01, 100, 50)).unwrap();
    repo.insert_call_log(&call("anthropic", 0.02, 200, 25)).unwrap();
    repo.insert_call_log(&call("openai", 0.005, 10, 5)).unwrap();

    let rows = repo.provider_usage_since("2000-01-01 00:00:00").unwrap();
    assert_eq!(rows.len(), 2);
    // Stable order, so the wire shape does not depend on the hash seed.
    assert_eq!(rows[0].provider, "anthropic");
    assert_eq!(rows[0].calls, 2);
    assert_eq!(rows[0].tokens, 375);
    assert!((rows[0].cost_usd - 0.03).abs() < 1e-9);
    assert_eq!(rows[1].provider, "openai");
    assert_eq!(rows[1].calls, 1);
    assert_eq!(rows[1].tokens, 15);
}

/// Yesterday's spend is not today's: the cutoff is the whole point of the row.
#[test]
fn provider_usage_since_excludes_calls_before_the_cutoff() {
    let db = setup_db();
    let repo = LlmUsageRepository::new(&db);

    let mut old = call("anthropic", 9.99, 1, 1);
    old.timestamp = Utc::now() - chrono::Duration::days(2);
    repo.insert_call_log(&old).unwrap();
    repo.insert_call_log(&call("anthropic", 0.01, 1, 1)).unwrap();

    let since = (Utc::now() - chrono::Duration::days(1))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let rows = repo.provider_usage_since(&since).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].calls, 1);
    assert!((rows[0].cost_usd - 0.01).abs() < 1e-9);
}

/// A day with no calls yet is an empty list, not a row of zeroes for a
/// provider that has not been used.
#[test]
fn provider_usage_since_is_empty_before_the_first_call() {
    let db = setup_db();
    let repo = LlmUsageRepository::new(&db);

    assert!(
        repo.provider_usage_since("2000-01-01 00:00:00")
            .unwrap()
            .is_empty()
    );
}

/// R63: `llm_call_log` had no index leading on `timestamp`, so this query —
/// re-run on every `llm_call_completed` event while Settings is open —
/// full-scanned the whole append-only log under the daemon's single
/// connection lock. Migration 040 adds `idx_llm_call_log_timestamp`; this
/// proves the query plan actually uses it, not merely that the index exists
/// unused next to the table.
#[test]
fn provider_usage_since_query_plan_uses_the_timestamp_index() {
    let db = setup_db();

    let plan_lines: Vec<String> = db
        .with_connection(|conn| {
            let mut stmt = conn.prepare(
                "EXPLAIN QUERY PLAN SELECT provider, SUM(cost_usd), COUNT(*), \
                 SUM(input_tokens + output_tokens) FROM llm_call_log WHERE timestamp >= ?1 \
                 GROUP BY provider ORDER BY provider",
            )?;
            let rows = stmt.query_map(rusqlite::params!["2000-01-01 00:00:00"], |row| {
                row.get::<_, String>(3)
            })?;
            let mut lines = Vec::new();
            for row in rows {
                lines.push(row?);
            }
            Ok(lines)
        })
        .unwrap();

    let plan = plan_lines.join(" | ");
    assert!(
        plan.contains("idx_llm_call_log_timestamp"),
        "expected the summary query's plan to use idx_llm_call_log_timestamp, got: {plan}"
    );
    assert!(
        !plan.contains("SCAN llm_call_log"),
        "the timestamp index should turn the WHERE clause into a SEARCH, not a full SCAN: {plan}"
    );
}
