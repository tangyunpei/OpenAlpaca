use anyhow::Result;
use async_trait::async_trait;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::{error, info};

use super::EventWatcher;
use openalpaca_api::events::WakeEvent;

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

        // Setup notify watcher
        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                match res {
                    Ok(event) => {
                        // Filter for only Create/Modify events roughly
                        // notify events can be complex, but for wake purposes any change is a signal
                        if let Some(path) = event.paths.first() {
                            let path_str = path.to_string_lossy().to_string();
                            let kind = format!("{:?}", event.kind);

                            let wake_event = WakeEvent::FileChanged {
                                path: path_str,
                                kind,
                            };

                            // Use try_send to avoid blocking the watcher thread
                            if let Err(e) = tx_clone.try_send(wake_event) {
                                // It's expected to fail if channel is full or closed, just log debug/warn
                                // tracing might also block, so be careful.
                                // But here we just log error if strictly needed.
                                // For high throughput, we might drop logs.
                                // However, try_send error usually means receiver dropped or full.
                                // We ignore Full error based on architectural decision (drop if system overloaded)
                                // But let's log debug.
                                // Note: we can't easily use tracing in sync callback if it blocks?
                                // Usually it's fine.
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
