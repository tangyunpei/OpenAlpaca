use anyhow::Result;
use async_trait::async_trait;
use notify::{Config, Event, PollWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use super::EventWatcher;
use openalpaca_api::events::WakeEvent;

/// Debounce window in milliseconds
const DEBOUNCE_MS: u128 = 100;
/// Poll interval for `notify::PollWatcher`.
///
/// We prefer polling here because native backends (e.g. FSEvents) can be unavailable or blocked
/// under sandboxed environments. Polling is slower but predictable and testable.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Watcher for filesystem changes
pub struct FilesystemWatcher {
    paths: Vec<PathBuf>,
    poll_interval: Duration,
    watcher: Arc<Mutex<Option<PollWatcher>>>,
}

impl FilesystemWatcher {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self::with_poll_interval(paths, POLL_INTERVAL)
    }

    pub fn with_poll_interval(paths: Vec<PathBuf>, poll_interval: Duration) -> Self {
        Self {
            paths,
            poll_interval,
            watcher: Arc::new(Mutex::new(None)),
        }
    }

    /// Returns a cloneable handle that can unwatch individual paths.
    pub fn unwatch_handle(&self) -> FileWatchHandle {
        FileWatchHandle {
            inner: Arc::clone(&self.watcher),
        }
    }
}

/// A cloneable handle for unwatching individual paths from a running [`FilesystemWatcher`].
#[derive(Clone)]
pub struct FileWatchHandle {
    inner: Arc<Mutex<Option<PollWatcher>>>,
}

impl FileWatchHandle {
    /// Stop polling a specific path.
    pub fn unwatch_path(&self, path: &Path) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        if let Some(ref mut watcher) = *guard {
            watcher.unwatch(path)?;
            info!("Unwatched path: {:?}", path);
        }
        Ok(())
    }
}

#[async_trait]
impl EventWatcher for FilesystemWatcher {
    async fn start(&self, tx: mpsc::Sender<WakeEvent>) -> Result<()> {
        let tx_clone = tx.clone();
        // Simple debounce: track last event time per path
        let last_event: Arc<Mutex<HashMap<String, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
        let last_event_clone = last_event.clone();

        // Setup notify watcher
        let mut watcher = PollWatcher::new(
            move |res: Result<Event, notify::Error>| {
                match res {
                    Ok(event) => {
                        let Event { kind, paths, .. } = event;

                        // Ignore access-only events (open/close). For wake purposes we treat
                        // everything else (including `Any`) as a relevant change signal.
                        if kind.is_access() {
                            return;
                        }

                        // `notify` can return multiple paths for a single event (e.g., renames),
                        // and on some platforms the "interesting" path is not necessarily first.
                        let change_type = format!("{:?}", kind);
                        for path in paths {
                            let path_str = path.to_string_lossy().to_string();

                            // Simple debounce: skip if same path within DEBOUNCE_MS
                            {
                                let mut last = last_event_clone.lock().unwrap();
                                let now = Instant::now();
                                if let Some(last_time) = last.get(&path_str)
                                    && now.duration_since(*last_time).as_millis() < DEBOUNCE_MS
                                {
                                    debug!("Debounced event for: {}", path_str);
                                    continue;
                                }
                                last.insert(path_str.clone(), now);
                            }

                            let wake_event = WakeEvent::FileChanged {
                                path: path_str,
                                change_type: change_type.clone(),
                            };

                            // Use try_send to avoid blocking the watcher thread
                            if let Err(e) = tx_clone.try_send(wake_event) {
                                // Drop if channel full (backpressure)
                                debug!(
                                    "Filesystem wake event dropped (channel full or closed): {e}"
                                );
                            }
                        }
                    }
                    Err(e) => error!("Watch error: {:?}", e),
                }
            },
            Config::default().with_poll_interval(self.poll_interval),
        )?;

        // Add paths to watch
        for path in &self.paths {
            if path.exists() {
                let mode = if path.is_dir() {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };
                watcher.watch(path, mode)?;
                info!("Watching path: {:?}", path);
            } else {
                // Try creating if implementation allows, but here we just warn or skip
                warn!("Path does not exist, cannot watch: {:?}", path);
            }
        }

        // Store watcher to keep it alive
        let mut w = self.watcher.lock().unwrap();
        *w = Some(watcher);

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut w = self.watcher.lock().unwrap();
        // Dropping the watcher stops it
        *w = None;
        info!("FilesystemWatcher stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
        let watcher = FilesystemWatcher::with_poll_interval(
            vec![dir_path.clone()],
            Duration::from_millis(500),
        );

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
}
