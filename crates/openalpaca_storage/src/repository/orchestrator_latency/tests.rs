use super::*;

fn setup_db() -> crate::Database {
    let dir = tempfile::tempdir().unwrap();
    crate::Database::open(&dir.path().join("test.db")).unwrap()
}

#[test]
fn test_percentile_single_record() {
    assert_eq!(percentile(&[42], 50), 42);
    assert_eq!(percentile(&[42], 95), 42);
    assert_eq!(percentile(&[42], 99), 42);
}

#[test]
fn test_percentile_empty() {
    assert_eq!(percentile(&[], 50), 0);
}

#[test]
fn test_percentile_multiple() {
    // 10 values: 1..=10
    let sorted: Vec<u64> = (1..=10).collect();
    assert_eq!(percentile(&sorted, 50), 5);
    assert_eq!(percentile(&sorted, 95), 10);
    assert_eq!(percentile(&sorted, 99), 10);
}

#[test]
fn test_percentile_100_values() {
    let sorted: Vec<u64> = (1..=100).collect();
    assert_eq!(percentile(&sorted, 50), 50);
    assert_eq!(percentile(&sorted, 95), 95);
    assert_eq!(percentile(&sorted, 99), 99);
}

#[test]
fn test_aggregate_empty() {
    let db = setup_db();
    let repo = OrchestratorLatencyRepository::new(&db);
    let result = repo.aggregate_by_mode(None, None).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_aggregate_single_mode() {
    let db = setup_db();
    let repo = OrchestratorLatencyRepository::new(&db);

    for i in 0..10 {
        repo.record(&OrchestratorLatencyRecord {
            id: None,
            request_id: format!("req-{}", i),
            mode: "main_loop".to_string(),
            ack_ms: 2,
            fallback_reason: None,
            auto_promotion_reason: None,
            timestamp: None,
        })
        .unwrap();
    }

    let aggs = repo.aggregate_by_mode(None, None).unwrap();
    assert_eq!(aggs.len(), 1);
    assert_eq!(aggs[0].mode, "main_loop");
    assert_eq!(aggs[0].count, 10);
    assert!(aggs[0].p50_total_ms > 0);
    assert!(aggs[0].p95_total_ms >= aggs[0].p50_total_ms);
    assert!(aggs[0].mean_ack_ms > 0.0);
}

#[test]
fn test_aggregate_multiple_modes() {
    let db = setup_db();
    let repo = OrchestratorLatencyRepository::new(&db);

    for mode in &["main_loop", "task_ops", "steered"] {
        for i in 0..5 {
            repo.record(&OrchestratorLatencyRecord {
                id: None,
                request_id: format!("req-{}-{}", mode, i),
                mode: mode.to_string(),
                ack_ms: 2,
                fallback_reason: if i == 0 {
                    Some("test".to_string())
                } else {
                    None
                },
                auto_promotion_reason: if i == 1 {
                    Some("auto".to_string())
                } else {
                    None
                },
                timestamp: None,
            })
            .unwrap();
        }
    }

    let aggs = repo.aggregate_by_mode(None, None).unwrap();
    assert_eq!(aggs.len(), 3);
    // Sorted by mode name
    assert_eq!(aggs[0].mode, "main_loop");
    assert_eq!(aggs[1].mode, "steered");
    assert_eq!(aggs[2].mode, "task_ops");
    // Each has 5 records
    for agg in &aggs {
        assert_eq!(agg.count, 5);
        assert_eq!(agg.fallback_count, 1);
        assert_eq!(agg.auto_promotion_count, 1);
    }
}
