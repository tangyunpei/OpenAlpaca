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

/// L14. The poll watcher's own report that a watched file has gone is not a
/// fault: deleting `config/orchestrator/BOOTSTRAP.md` is how onboarding ends,
/// and the exact error the live daemon logged for it was an
/// `ErrorKind::Io(NotFound)` carrying that path.
#[test]
fn a_deleted_watched_path_is_not_an_error() {
    let bootstrap = std::path::PathBuf::from("/tmp/config/orchestrator/BOOTSTRAP.md");

    let observed = notify::Error::io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!(
            "IO error for operation on {}: No such file or directory (os error 2)",
            bootstrap.display()
        ),
    ))
    .add_path(bootstrap);

    assert_eq!(watch_error_level(&observed), tracing::Level::INFO);
    assert_eq!(
        watch_error_level(&notify::Error::path_not_found()),
        tracing::Level::INFO
    );
}

/// …and everything else the watcher cannot do still is one.
#[test]
fn a_watcher_that_cannot_do_its_job_is_still_an_error() {
    assert_eq!(
        watch_error_level(&notify::Error::generic("the backend gave up")),
        tracing::Level::ERROR
    );
    assert_eq!(
        watch_error_level(&notify::Error::io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        ))),
        tracing::Level::ERROR
    );
}
