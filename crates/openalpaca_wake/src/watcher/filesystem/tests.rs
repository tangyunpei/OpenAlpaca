use super::*;
use std::fs::File;
use std::time::Duration;
use tempfile::tempdir;
use tokio::time::timeout;

#[tokio::test]
async fn test_filesystem_watcher_robust() {
    let dir = tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let file_path = dir_path.join("test_trigger.txt");

    let (tx, mut rx) = mpsc::channel(10);
    let watcher = FilesystemWatcher::new(vec![dir_path.clone()]);

    watcher.start(tx).await.unwrap();

    // Create file to trigger event
    // Give watcher a tiny bit of time to spin up? (notify usually sync setup)
    tokio::time::sleep(Duration::from_millis(50)).await;

    let _f = File::create(&file_path).unwrap();

    // Retry / Wait loop logic handled by timeout on channel
    // notify might batch events or delay slightly
    let result = timeout(Duration::from_secs(2), async {
        loop {
            match rx.recv().await {
                Some(WakeEvent::FileChanged { path, .. }) => {
                    if path.contains("test_trigger.txt") {
                        return true;
                    }
                    // Ignore other temp files if any (unlikely in tempdir)
                }
                None => return false,
                _ => continue,
            }
        }
    })
    .await;

    assert!(result.is_ok(), "Timed out waiting for file event");
    assert!(result.unwrap(), "Stream closed or event not found");

    watcher.stop().await.unwrap();
}

#[tokio::test]
async fn test_custom_poll_interval() {
    let dir = tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let file_path = dir_path.join("custom_poll_trigger.txt");

    let (tx, mut rx) = mpsc::channel(10);
    let watcher =
        FilesystemWatcher::with_poll_interval(vec![dir_path.clone()], Duration::from_millis(500));

    watcher.start(tx).await.unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    let _f = File::create(&file_path).unwrap();

    // Longer timeout since poll interval is 500ms
    let result = timeout(Duration::from_secs(3), async {
        loop {
            match rx.recv().await {
                Some(WakeEvent::FileChanged { path, .. }) => {
                    if path.contains("custom_poll_trigger.txt") {
                        return true;
                    }
                }
                None => return false,
                _ => continue,
            }
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "Timed out waiting for file event with custom poll interval"
    );
    assert!(result.unwrap(), "Stream closed or event not found");

    watcher.stop().await.unwrap();
}
