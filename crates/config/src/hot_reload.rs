use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use openalpaca_core::CoreError;

use crate::io::load_config;
use crate::types::OpenAlpacaConfig;

/// Watches a config file and atomically swaps the config on changes.
/// Uses `ArcSwap` for lock-free reads.
pub struct ConfigWatcher {
    config: Arc<ArcSwap<OpenAlpacaConfig>>,
    _watcher: RecommendedWatcher,
    change_rx: tokio::sync::watch::Receiver<()>,
}

impl ConfigWatcher {
    /// Start watching `path` for changes. `debounce_ms` controls the
    /// coalesce window (pass `None` for the default 300ms).
    pub fn new(
        path: PathBuf,
        initial: OpenAlpacaConfig,
        debounce_ms: Option<u64>,
    ) -> Result<Self, CoreError> {
        let config = Arc::new(ArcSwap::new(Arc::new(initial)));
        let (change_tx, change_rx) = tokio::sync::watch::channel(());
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();

        let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, _>| {
            if let Ok(event) = res
                && matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                )
            {
                let _ = event_tx.send(());
            }
        })
        .map_err(|e| CoreError::Io(std::io::Error::other(format!("watcher error: {e}"))))?;

        let watch_path = if path.is_file() {
            path.parent().unwrap_or(&path).to_path_buf()
        } else {
            path.clone()
        };
        watcher
            .watch(&watch_path, RecursiveMode::NonRecursive)
            .map_err(|e| CoreError::Io(std::io::Error::other(format!("watch error: {e}"))))?;

        let debounce = Duration::from_millis(debounce_ms.unwrap_or(300));
        let config_clone = config.clone();
        let path_clone = path.clone();
        tokio::spawn(async move {
            debounce_loop(
                &mut event_rx,
                &config_clone,
                &change_tx,
                &path_clone,
                debounce,
            )
            .await;
        });

        Ok(Self {
            config,
            _watcher: watcher,
            change_rx,
        })
    }

    /// Get the shared ArcSwap handle so other subsystems can share this
    /// config reference and see updates without polling.
    pub fn config_arc(&self) -> Arc<ArcSwap<OpenAlpacaConfig>> {
        self.config.clone()
    }

    /// Get current config (lock-free read).
    pub fn load(&self) -> arc_swap::Guard<Arc<OpenAlpacaConfig>> {
        self.config.load()
    }

    /// Subscribe to config change notifications.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<()> {
        self.change_rx.clone()
    }
}

async fn debounce_loop(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<()>,
    config: &Arc<ArcSwap<OpenAlpacaConfig>>,
    change_tx: &tokio::sync::watch::Sender<()>,
    path: &Path,
    debounce: Duration,
) {
    loop {
        // Wait for first event
        if rx.recv().await.is_none() {
            break;
        }

        // Drain events during debounce window
        let deadline = tokio::time::Instant::now() + debounce;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(())) => {}
                _ => break,
            }
        }

        // Reload config
        match load_config(path) {
            Ok(snapshot) => {
                if !snapshot.issues.is_empty() {
                    for issue in &snapshot.issues {
                        tracing::warn!("config validation: {issue}");
                    }
                }

                // Compute diff to determine what actually changed
                let old_value = serde_yml::to_value(&**config.load()).unwrap_or_default();
                let new_value = serde_yml::to_value(&snapshot.config).unwrap_or_default();
                let changed = crate::merge::diff_paths(&old_value, &new_value, "");

                if changed.is_empty() {
                    tracing::debug!(
                        "config file changed on disk but no config values differ, skipping reload"
                    );
                } else {
                    tracing::info!("config reloaded, changed paths: {}", changed.join(", "));
                    config.store(Arc::new(snapshot.config));
                    let _ = change_tx.send(());
                }
            }
            Err(e) => {
                tracing::error!("failed to reload config: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_config_watcher_loads_initial() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "gateway:\n  port: 3777").unwrap();

        let snapshot = load_config(&path).unwrap();
        let watcher = ConfigWatcher::new(path, snapshot.config, None).unwrap();

        let config = watcher.load();
        assert_eq!(config.gateway.as_ref().unwrap().port, Some(3777));
    }

    #[tokio::test]
    async fn test_config_watcher_skips_no_op_change() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "gateway:\n  port: 3777").unwrap();

        let snapshot = load_config(&path).unwrap();
        let watcher = ConfigWatcher::new(path.clone(), snapshot.config, None).unwrap();
        let mut change_rx = watcher.subscribe();

        // Rewrite the same content — should NOT trigger a reload notification
        tokio::time::sleep(Duration::from_millis(50)).await;
        std::fs::write(&path, "gateway:\n  port: 3777").unwrap();

        // The watcher should NOT send a change notification for identical content
        let result = tokio::time::timeout(Duration::from_secs(1), change_rx.changed()).await;
        // Timeout is expected — no change should be notified
        assert!(
            result.is_err(),
            "should not receive change notification for identical config"
        );

        // Verify config is unchanged
        let config = watcher.load();
        assert_eq!(config.gateway.as_ref().unwrap().port, Some(3777));
    }

    #[tokio::test]
    async fn test_config_watcher_detects_change() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "gateway:\n  port: 3777").unwrap();

        let snapshot = load_config(&path).unwrap();
        let watcher = ConfigWatcher::new(path.clone(), snapshot.config, None).unwrap();
        let mut change_rx = watcher.subscribe();

        // Modify the config file
        tokio::time::sleep(Duration::from_millis(50)).await;
        std::fs::write(&path, "gateway:\n  port: 8080").unwrap();

        // Wait for the change notification (debounce + processing)
        let result = tokio::time::timeout(Duration::from_secs(2), change_rx.changed()).await;
        assert!(result.is_ok(), "should receive change notification");

        let config = watcher.load();
        assert_eq!(config.gateway.as_ref().unwrap().port, Some(8080));
    }
}
