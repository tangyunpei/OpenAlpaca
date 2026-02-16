use anyhow::{Context, Result};

use tokio::sync::mpsc;
use tracing::{error, info};

use crate::models::ScheduledTask;
use crate::scheduler::WakeScheduler;
use crate::watcher::{EventWatcher, filesystem::{FileWatchHandle, FilesystemWatcher}};
use openalpaca_api::events::WakeEvent;

/// Central manager for the Wake Module
///
/// Orchestrates the scheduler and event watchers.
pub struct WakeManager {
    scheduler: WakeScheduler,
    watchers: Vec<Box<dyn EventWatcher>>,
    fs_watch_handle: Option<FileWatchHandle>,
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
            fs_watch_handle: None,
            event_tx,
        })
    }

    /// Add a filesystem watcher
    pub fn add_filesystem_watcher(&mut self, paths: Vec<std::path::PathBuf>) {
        let watcher = FilesystemWatcher::new(paths);
        self.fs_watch_handle = Some(watcher.unwatch_handle());
        self.watchers.push(Box::new(watcher));
    }

    /// Add a filesystem watcher with a custom poll interval
    pub fn add_filesystem_watcher_with_interval(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        poll_interval: std::time::Duration,
    ) {
        let watcher = FilesystemWatcher::with_poll_interval(paths, poll_interval);
        self.fs_watch_handle = Some(watcher.unwatch_handle());
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
            if let Err(e) = watcher.start(self.event_tx.clone()).await {
                error!("Failed to start watcher: {:?}", e);
            }
        }

        info!("WakeManager running with {} watchers", self.watchers.len());
        Ok(())
    }

    /// Shutdown all components (watchers + scheduler)
    pub async fn shutdown(&self) -> Result<()> {
        info!("Stopping WakeManager...");
        for watcher in &self.watchers {
            let _ = watcher.stop().await;
        }
        if let Err(e) = self.scheduler.shutdown().await {
            error!("Failed to shut down WakeScheduler: {:?}", e);
        }
        Ok(())
    }

    /// Schedule a recurring cron task (passthrough to scheduler)
    pub async fn schedule_cron(&self, task: ScheduledTask) -> Result<uuid::Uuid> {
        self.scheduler.schedule_cron(task).await
    }

    /// Remove a scheduled job by task ID (passthrough to scheduler)
    pub async fn remove_job(&self, task_id: &str) -> Result<()> {
        self.scheduler.remove_job(task_id).await
    }

    /// List all scheduled jobs (passthrough to scheduler)
    pub async fn list_jobs(&self) -> Vec<ScheduledTask> {
        self.scheduler.list_jobs().await
    }

    /// Returns a cloneable handle for unwatching individual filesystem paths.
    pub fn fs_watch_handle(&self) -> Option<FileWatchHandle> {
        self.fs_watch_handle.clone()
    }

    // accessors for testing or dynamic scheduling
    pub fn scheduler(&self) -> &WakeScheduler {
        &self.scheduler
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_manager_shutdown_stops_scheduler() {
        let (tx, _rx) = mpsc::channel::<WakeEvent>(16);
        let wake_manager = WakeManager::new(tx).await.unwrap();
        wake_manager.start().await.unwrap();

        let task = ScheduledTask {
            id: "mgr_test".to_string(),
            cron: "0 0 * * * *".to_string(),
            tag: "mgr".to_string(),
            job_uuid: None,
        };
        wake_manager.schedule_cron(task).await.unwrap();

        // Shutdown should not error
        let result = wake_manager.shutdown().await;
        assert!(result.is_ok(), "Manager shutdown should succeed");
    }
}
