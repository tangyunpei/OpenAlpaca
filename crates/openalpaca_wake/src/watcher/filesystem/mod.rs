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

/// At what level the poll watcher reports one of its own failures (L14).
///
/// A watched path that no longer exists is not a fault. Deleting
/// `config/orchestrator/BOOTSTRAP.md` is the *designed* end of onboarding, and
/// the poll watcher reports the removal as an IO `NotFound` before the
/// orchestrator's unwatch lands — so the one moment the system worked exactly
/// as intended printed an `ERROR` naming the file the owner had just been told
/// to delete. That reads as `INFO`; everything else the watcher cannot do is
/// still an `ERROR`.
fn watch_error_level(e: &notify::Error) -> tracing::Level {
    let gone = match &e.kind {
        notify::ErrorKind::PathNotFound => true,
        notify::ErrorKind::Io(io) => io.kind() == std::io::ErrorKind::NotFound,
        _ => false,
    };
    if gone {
        tracing::Level::INFO
    } else {
        tracing::Level::ERROR
    }
}

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
                    Err(e) => match watch_error_level(&e) {
                        tracing::Level::INFO => info!(
                            "Watched path is gone; it will stop being polled: {:?}",
                            e.paths
                        ),
                        _ => error!("Watch error: {:?}", e),
                    },
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
mod tests;
