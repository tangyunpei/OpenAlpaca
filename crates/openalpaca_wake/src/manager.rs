use anyhow::{Context, Result};

use tokio::sync::mpsc;
use tracing::{error, info};

use crate::scheduler::WakeScheduler;
use crate::watcher::{EventWatcher, filesystem::FilesystemWatcher};
use openalpaca_api::events::WakeEvent;

/// Central manager for the Wake Module
///
/// Orchestrates the scheduler and event watchers.
pub struct WakeManager {
    scheduler: WakeScheduler,
    watchers: Vec<Box<dyn EventWatcher>>,
    event_tx: mpsc::Sender<WakeEvent>,
}

impl WakeManager {
    /// Create a new WakeManager
    ///
    /// * `event_tx`: Channel to send generated WakeEvents to (forwarded to Core/Daemon)
    pub async fn new(event_tx: mpsc::Sender<WakeEvent>) -> Result<Self> {
        // Initialize Scheduler
        let scheduler = WakeScheduler::new(event_tx.clone())
            .await
            .context("Failed to initialize WakeScheduler")?;

        Ok(Self {
            scheduler,
            watchers: Vec::new(),
            event_tx,
        })
    }

    /// Add a filesystem watcher
    pub fn add_filesystem_watcher(&mut self, paths: Vec<std::path::PathBuf>) {
        let watcher = FilesystemWatcher::new(paths);
        self.watchers.push(Box::new(watcher));
    }

    /// Start all components
    pub async fn start(&self) -> Result<()> {
        info!("Starting WakeManager...");

        // Start Scheduler
        self.scheduler
            .start()
            .await
            .context("Failed to start WakeScheduler")?;

        // Start Watchers
        for watcher in &self.watchers {
            // Watchers run asynchronously and send events to the shared tx
            // We shouldn't block here, but watcher.start() is async.
            // Assuming watcher.start() initializes and returns quickly (spawns internal task).
            // Our FilesystemWatcher implementation does exactly that.
            if let Err(e) = watcher.start(self.event_tx.clone()).await {
                error!("Failed to start watcher: {:?}", e);
            }
        }

        info!("WakeManager running with {} watchers", self.watchers.len());
        Ok(())
    }

    /// Shutdown all components
    pub async fn shutdown(&self) -> Result<()> {
        info!("Stopping WakeManager...");
        for watcher in &self.watchers {
            let _ = watcher.stop().await;
        }
        // Scheduler in tokio-cron-scheduler doesn't have explicit async stop needed usually,
        // or it stops when dropped/runtime ends.
        Ok(())
    }

    // accessors for testing or dynamic scheduling
    pub fn scheduler(&self) -> &WakeScheduler {
        &self.scheduler
    }
}
