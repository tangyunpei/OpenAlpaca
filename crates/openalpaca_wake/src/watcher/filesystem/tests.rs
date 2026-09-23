use super::*;
use std::fs::File;
use std::path::PathBuf;
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

// ── The handler itself, driven without a poll thread or a real clock wait ──

fn event(kind: notify::EventKind, paths: &[&str]) -> Result<Event, notify::Error> {
    let mut e = Event::new(kind);
    for p in paths {
        e = e.add_path(PathBuf::from(p));
    }
    Ok(e)
}

fn modified() -> notify::EventKind {
    notify::EventKind::Modify(notify::event::ModifyKind::Any)
}

fn drain(rx: &mut mpsc::Receiver<WakeEvent>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        match ev {
            WakeEvent::FileChanged { path, change_type } => out.push((path, change_type)),
            other => panic!("expected FileChanged, got {other:?}"),
        }
    }
    out
}

/// One event naming several paths wakes once per path, in the order `notify`
/// listed them, each carrying the event kind's `Debug` text — and a path named
/// twice in the same event is debounced like any other repeat.
#[test]
fn one_event_with_several_paths_wakes_once_per_path_in_order() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut handle = debounced_handler(tx);

    handle(event(modified(), &["/w/from", "/w/to", "/w/from"]));

    let kind = format!("{:?}", modified());
    assert_eq!(
        drain(&mut rx),
        vec![
            ("/w/from".to_string(), kind.clone()),
            ("/w/to".to_string(), kind)
        ]
    );
}

/// Opening or closing a file is not a change; nothing wakes and nothing is
/// recorded, so the next real change to that path is not debounced away.
#[test]
fn an_access_only_event_wakes_nothing_and_debounces_nothing() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut handle = debounced_handler(tx);

    handle(event(
        notify::EventKind::Access(notify::event::AccessKind::Any),
        &["/w/a"],
    ));
    assert!(drain(&mut rx).is_empty());

    handle(event(modified(), &["/w/a"]));
    assert_eq!(drain(&mut rx).len(), 1);
}

/// A repeat of the same path inside the window is dropped; another path is
/// not; and once the window has passed the same path wakes again.
#[test]
fn a_repeat_inside_the_window_is_debounced_per_path() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut handle = debounced_handler(tx);

    handle(event(modified(), &["/w/a"]));
    handle(event(modified(), &["/w/a"]));
    handle(event(modified(), &["/w/b"]));
    let paths: Vec<String> = drain(&mut rx).into_iter().map(|(p, _)| p).collect();
    assert_eq!(paths, vec!["/w/a", "/w/b"]);

    std::thread::sleep(Duration::from_millis(DEBOUNCE_MS as u64 + 20));
    handle(event(modified(), &["/w/a"]));
    assert_eq!(drain(&mut rx).len(), 1, "the window is over");
}

/// A full queue drops the wake instead of blocking the watcher thread — and
/// the dropped path's time is still recorded, so an immediate repeat of it is
/// debounced even after the queue has room again.
#[test]
fn a_full_queue_drops_the_wake_without_blocking() {
    let (tx, mut rx) = mpsc::channel(1);
    let mut handle = debounced_handler(tx);

    handle(event(modified(), &["/w/a", "/w/b"]));
    let paths: Vec<String> = drain(&mut rx).into_iter().map(|(p, _)| p).collect();
    assert_eq!(paths, vec!["/w/a"], "the second wake found the queue full");

    handle(event(modified(), &["/w/b"]));
    assert!(
        drain(&mut rx).is_empty(),
        "the dropped path was stamped before the send"
    );
}

/// A receiver that has gone away, or an error from the watcher, is logged and
/// never panics the watcher thread.
#[test]
fn a_closed_queue_or_a_watch_error_does_not_panic() {
    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    let mut handle = debounced_handler(tx);

    handle(event(modified(), &["/w/a"]));
    handle(Err(notify::Error::path_not_found()));
    handle(Err(notify::Error::generic("the backend gave up")));
}
