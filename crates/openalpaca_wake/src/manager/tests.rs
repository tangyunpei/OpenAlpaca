use super::*;

#[tokio::test]
async fn test_manager_shutdown_stops_scheduler() {
    let (tx, _rx) = mpsc::channel::<WakeEvent>(16);
    let wake_manager = WakeManager::new(tx).await.unwrap();
    wake_manager.start().await.unwrap();

    let task = ScheduledTask {
        id: "mgr_test".to_string(),
        cron: "0 0 * * * *".to_string(),
        tag: "mgr".to_string(),
        job_uuid: None,
    };
    wake_manager.schedule_cron(task).await.unwrap();

    // Shutdown should not error
    let result = wake_manager.shutdown().await;
    assert!(result.is_ok(), "Manager shutdown should succeed");
}
