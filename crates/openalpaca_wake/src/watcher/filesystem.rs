use anyhow::Result;
use async_trait::async_trait;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

use super::EventWatcher;
use openalpaca_api::events::WakeEvent;

/// Debounce window in milliseconds
const DEBOUNCE_MS: u128 = 100;

/// Watcher for filesystem changes
pub struct FilesystemWatcher {
    paths: Vec<PathBuf>,
    watcher: Arc<Mutex<Option<RecommendedWatcher>>>,
}

impl FilesystemWatcher {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self {
            paths,
            watcher: Arc::new(Mutex::new(None)),
        }
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
        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                match res {
                    Ok(event) => {
                        // Filter: Only handle Create/Modify/Remove events
                        let is_relevant = matches!(
                            event.kind,
                            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                        );

                        if !is_relevant {
                            return;
                        }

                        if let Some(path) = event.paths.first() {
                            let path_str = path.to_string_lossy().to_string();

                            // Simple debounce: skip if same path within DEBOUNCE_MS
                            {
                                let mut last = last_event_clone.lock().unwrap();
                                let now = Instant::now();
                                if let Some(last_time) = last.get(&path_str)
                                    && now.duration_since(*last_time).as_millis() < DEBOUNCE_MS
                                {
                                    debug!("Debounced event for: {}", path_str);
                                    return;
                                }
                                last.insert(path_str.clone(), now);
                            }

                            let change_type = format!("{:?}", event.kind);

                            let wake_event = WakeEvent::FileChanged {
                                path: path_str,
                                change_type,
                            };

                            // Use try_send to avoid blocking the watcher thread
                            if let Err(_e) = tx_clone.try_send(wake_event) {
                                // Drop if channel full (backpressure)
                            }
                        }
                    }
                    Err(e) => error!("Watch error: {:?}", e),
                }
            },
            Config::default(),
        )?;

        // Add paths to watch
        for path in &self.paths {
            if path.exists() {
                watcher.watch(path, RecursiveMode::NonRecursive)?;
                info!("Watching path: {:?}", path);
            } else {
                // Try creating if implementation allows, but here we just warn or skip
                error!("Path does not exist, cannot watch: {:?}", path);
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
}
