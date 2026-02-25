use std::collections::HashMap;

use anyhow::Result;
use tokio::sync::{Mutex, mpsc};
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{info, warn};
use uuid::Uuid;

use crate::models::ScheduledTask;
use openalpaca_api::events::WakeEvent;

/// Internal state guarded by a single Mutex (D1: prevents ABBA deadlock).
struct SchedulerInner {
    scheduler: JobScheduler,
    /// Maps user-facing task ID -> (scheduler UUID, ScheduledTask snapshot)
    jobs: HashMap<String, (Uuid, ScheduledTask)>,
}

/// Scheduler for time-based wake events
pub struct WakeScheduler {
    inner: Mutex<SchedulerInner>,
    event_tx: mpsc::Sender<WakeEvent>,
}

impl WakeScheduler {
    /// Create a new WakeScheduler with an event sender
    pub async fn new(event_tx: mpsc::Sender<WakeEvent>) -> Result<Self> {
        let scheduler = JobScheduler::new().await?;
        Ok(Self {
            inner: Mutex::new(SchedulerInner {
                scheduler,
                jobs: HashMap::new(),
            }),
            event_tx,
        })
    }

    /// Start the scheduler
    pub async fn start(&self) -> Result<()> {
        let inner = self.inner.lock().await;
        inner.scheduler.start().await?;
        info!("WakeScheduler started");
        Ok(())
    }

    /// Shut down the scheduler and clear all tracked jobs.
    pub async fn shutdown(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.scheduler.shutdown().await?;
        inner.jobs.clear();
        info!("WakeScheduler shut down");
        Ok(())
    }

    /// Schedule a recurring task using Cron expression.
    ///
    /// Returns the internal scheduler UUID on success.
    /// Rejects duplicate task IDs (D2) — call `remove_job()` first to reschedule.
    pub async fn schedule_cron(&self, task: ScheduledTask) -> Result<Uuid> {
        let tx = self.event_tx.clone();
        let job_id = task.id.clone();
        let tag = task.tag.clone();
        let schedule = task.cron.clone();

        let job = Job::new_async(schedule.as_str(), move |_uuid, _l| {
            let tx = tx.clone();
            let job_id = job_id.clone();
            let tag = tag.clone();
            Box::pin(async move {
                let event = WakeEvent::Timer { job_id, tag };
                if let Err(e) = tx.try_send(event) {
                    warn!("Cron wake event dropped: {}", e);
                }
            })
        })?;

        let mut inner = self.inner.lock().await;

        // Reject duplicate task IDs to prevent orphaned scheduler jobs (D2)
        if inner.jobs.contains_key(&task.id) {
            anyhow::bail!(
                "Task '{}' already scheduled. Remove it first to reschedule.",
                task.id
            );
        }

        let uuid = inner.scheduler.add(job).await?;
        info!(
            "Scheduled cron task: {} ({}) -> uuid={}",
            task.id, task.cron, uuid
        );
        inner.jobs.insert(task.id.clone(), (uuid, task));
        Ok(uuid)
    }

    /// Remove a scheduled job by user-facing task ID.
    pub async fn remove_job(&self, task_id: &str) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let uuid = inner
            .jobs
            .get(task_id)
            .map(|(uuid, _)| *uuid)
            .ok_or_else(|| anyhow::anyhow!("No scheduled task with id '{}'", task_id))?;
        // Remove from scheduler first — if this fails, the map entry is preserved
        inner.scheduler.remove(&uuid).await?;
        // Only delete the registry entry after scheduler removal succeeds
        inner.jobs.remove(task_id);
        info!("Removed cron task: {} (uuid={})", task_id, uuid);
        Ok(())
    }

    /// List all currently scheduled jobs, sorted by task ID (D7).
    pub async fn list_jobs(&self) -> Vec<ScheduledTask> {
        let inner = self.inner.lock().await;
        let mut tasks: Vec<ScheduledTask> = inner
            .jobs
            .values()
            .map(|(uuid, task)| {
                let mut t = task.clone();
                t.job_uuid = Some(*uuid);
                t
            })
            .collect();
        tasks.sort_by(|a, b| a.id.cmp(&b.id));
        tasks
    }

    /// One-shot timer — test-only. Spawns an untracked tokio task.
    /// Not covered by shutdown() — use only in tests.
    #[cfg(test)]
    pub async fn schedule_once(&self, delay: tokio::time::Duration, tag: String) {
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let event = WakeEvent::Timer {
                job_id: "oneshot".to_string(),
                tag,
            };
            if let Err(e) = tx.try_send(event) {
                warn!("Oneshot wake event dropped (channel full or closed): {}", e);
            }
        });
        info!("Scheduled oneshot task with delay: {:?}", delay);
    }
}

#[cfg(test)]
mod tests;
