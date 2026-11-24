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

/// M55 (c): the watcher set is a closed enum. The filesystem watcher the
/// manager adds is its `Filesystem` variant, and `start` / `stop` on the
/// enum reach that watcher: a file created under the watched folder after
/// `start` arrives as a `FileChanged` event.
#[tokio::test]
async fn an_added_filesystem_watcher_is_the_filesystem_variant_and_dispatches() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = mpsc::channel::<WakeEvent>(16);
    let mut wake_manager = WakeManager::new(tx.clone()).await.unwrap();
    wake_manager.add_filesystem_watcher_with_interval(
        vec![dir.path().to_path_buf()],
        std::time::Duration::from_millis(100),
    );

    assert_eq!(wake_manager.watchers.len(), 1);
    let watcher: &EventWatcher = &wake_manager.watchers[0];
    let EventWatcher::Filesystem(_) = watcher;

    watcher.start(tx).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    std::fs::File::create(dir.path().join("m55c_trigger.txt")).unwrap();

    let seen = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            match rx.recv().await {
                Some(WakeEvent::FileChanged { path, .. }) if path.contains("m55c_trigger.txt") => {
                    return true;
                }
                Some(_) => continue,
                None => return false,
            }
        }
    })
    .await;
    assert_eq!(
        seen.ok(),
        Some(true),
        "the variant's start reached the watcher"
    );

    watcher.stop().await.unwrap();
}
