use anyhow::Result;
use tokio::sync::mpsc;

use openalpaca_api::events::WakeEvent;

pub mod filesystem;

/// The wake event watchers: a closed set, one variant per kind of watcher.
pub enum EventWatcher {
    /// Polls files and folders for changes.
    Filesystem(filesystem::FilesystemWatcher),
}

impl EventWatcher {
    /// Start watching for events
    pub async fn start(&self, tx: mpsc::Sender<WakeEvent>) -> Result<()> {
        match self {
            Self::Filesystem(watcher) => watcher.start(tx).await,
        }
    }

    /// Stop watching
    pub async fn stop(&self) -> Result<()> {
        match self {
            Self::Filesystem(watcher) => watcher.stop().await,
        }
    }
}
