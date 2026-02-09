use std::collections::HashMap;

use anyhow::Result;
use tokio::sync::{mpsc, Mutex};
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
                let event = WakeEvent::Timer {
                    job_id,
                    tag,
                };
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
mod tests {
    use super::*;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn test_schedule_once_robust() {
        let (tx, mut rx) = mpsc::channel::<WakeEvent>(16);
        let scheduler = WakeScheduler::new(tx).await.unwrap();

        // Schedule a task 100ms later
        scheduler
            .schedule_once(Duration::from_millis(100), "test_tag".to_string())
            .await;

        // Wait for max 1s
        let result = timeout(Duration::from_secs(1), rx.recv()).await;

        // Assert
        match result {
            Ok(Some(WakeEvent::Timer { tag, .. })) => {
                assert_eq!(tag, "test_tag", "Tag should match");
            }
            Ok(None) => panic!("Channel closed unexpectedly"),
            Ok(Some(_)) => panic!("Wrong event type"),
            Err(_) => panic!("Timed out await event"),
        }
    }

    #[tokio::test]
    async fn test_schedule_cron_ticks() {
        let (tx, mut rx) = mpsc::channel::<WakeEvent>(16);
        let scheduler = WakeScheduler::new(tx).await.unwrap();
        scheduler.start().await.unwrap();

        // Schedule a cron job that fires every second
        let task = ScheduledTask {
            id: "test_cron".to_string(),
            cron: "*/1 * * * * *".to_string(),
            tag: "cron_test".to_string(),
            job_uuid: None,
        };
        scheduler.schedule_cron(task).await.unwrap();

        // Wait up to 5 seconds, expecting at least 2 ticks
        let mut tick_count = 0;
        let result = timeout(Duration::from_secs(5), async {
            while tick_count < 2 {
                if let Some(WakeEvent::Timer { job_id, .. }) = rx.recv().await {
                    if job_id == "test_cron" {
                        tick_count += 1;
                    }
                }
            }
        })
        .await;

        assert!(result.is_ok(), "Timed out waiting for cron ticks");
        assert!(
            tick_count >= 2,
            "Expected at least 2 cron ticks, got {}",
            tick_count
        );
    }

    #[tokio::test]
    async fn test_remove_job() {
        let (tx, mut rx) = mpsc::channel::<WakeEvent>(16);
        let scheduler = WakeScheduler::new(tx).await.unwrap();
        scheduler.start().await.unwrap();

        let task = ScheduledTask {
            id: "removable".to_string(),
            cron: "*/1 * * * * *".to_string(),
            tag: "remove_test".to_string(),
            job_uuid: None,
        };
        scheduler.schedule_cron(task).await.unwrap();

        // Wait for at least 1 tick to confirm it's running
        let got_tick = timeout(Duration::from_secs(3), rx.recv()).await;
        assert!(got_tick.is_ok(), "Should receive at least one tick");

        // Remove the job
        scheduler.remove_job("removable").await.unwrap();

        // Drain any in-flight ticks
        while let Ok(Some(_)) = timeout(Duration::from_millis(200), rx.recv()).await {}

        // Grace window: no new events should arrive
        let result = timeout(Duration::from_secs(2), rx.recv()).await;
        // Err = timeout (good, no events). Ok(None) = channel closed (also acceptable).
        assert!(
            result.is_err() || matches!(result, Ok(None)),
            "Expected no events after removal, but got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_list_jobs() {
        let (tx, _rx) = mpsc::channel::<WakeEvent>(16);
        let scheduler = WakeScheduler::new(tx).await.unwrap();
        scheduler.start().await.unwrap();

        let task_a = ScheduledTask {
            id: "alpha".to_string(),
            cron: "0 0 * * * *".to_string(),
            tag: "tag_a".to_string(),
            job_uuid: None,
        };
        let task_b = ScheduledTask {
            id: "beta".to_string(),
            cron: "0 0 * * * *".to_string(),
            tag: "tag_b".to_string(),
            job_uuid: None,
        };
        scheduler.schedule_cron(task_a).await.unwrap();
        scheduler.schedule_cron(task_b).await.unwrap();

        let jobs = scheduler.list_jobs().await;
        assert_eq!(jobs.len(), 2);
        // Should be sorted by task ID
        assert_eq!(jobs[0].id, "alpha");
        assert_eq!(jobs[1].id, "beta");
        // Both should have job_uuid set
        assert!(jobs[0].job_uuid.is_some());
        assert!(jobs[1].job_uuid.is_some());
    }

    #[tokio::test]
    async fn test_remove_nonexistent_job() {
        let (tx, _rx) = mpsc::channel::<WakeEvent>(16);
        let scheduler = WakeScheduler::new(tx).await.unwrap();

        let result = scheduler.remove_job("no_such_task").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_duplicate_task_id_rejected() {
        let (tx, _rx) = mpsc::channel::<WakeEvent>(16);
        let scheduler = WakeScheduler::new(tx).await.unwrap();
        scheduler.start().await.unwrap();

        let task = ScheduledTask {
            id: "dup".to_string(),
            cron: "0 0 * * * *".to_string(),
            tag: "first".to_string(),
            job_uuid: None,
        };
        scheduler.schedule_cron(task).await.unwrap();

        let task2 = ScheduledTask {
            id: "dup".to_string(),
            cron: "0 0 * * * *".to_string(),
            tag: "second".to_string(),
            job_uuid: None,
        };
        let result = scheduler.schedule_cron(task2).await;
        assert!(result.is_err(), "Duplicate task ID should be rejected");

        // Original job should still be listed
        let jobs = scheduler.list_jobs().await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].tag, "first");
    }

    #[tokio::test]
    async fn test_shutdown_stops_jobs() {
        let (tx, mut rx) = mpsc::channel::<WakeEvent>(16);
        let scheduler = WakeScheduler::new(tx).await.unwrap();
        scheduler.start().await.unwrap();

        let task = ScheduledTask {
            id: "shutdown_test".to_string(),
            cron: "*/1 * * * * *".to_string(),
            tag: "shutdown".to_string(),
            job_uuid: None,
        };
        scheduler.schedule_cron(task).await.unwrap();

        // Wait for at least 1 tick
        let got_tick = timeout(Duration::from_secs(3), rx.recv()).await;
        assert!(got_tick.is_ok(), "Should receive at least one tick");

        // Shut down
        scheduler.shutdown().await.unwrap();

        // Allow in-flight events to settle after shutdown
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Drain any in-flight ticks
        while let Ok(Some(_)) = timeout(Duration::from_millis(500), rx.recv()).await {}

        // Grace window: no new events should arrive
        let result = timeout(Duration::from_secs(2), rx.recv()).await;
        assert!(
            result.is_err() || matches!(result, Ok(None)),
            "Expected no events after shutdown, but got: {:?}",
            result
        );

        // Jobs map should be cleared
        let jobs = scheduler.list_jobs().await;
        assert!(jobs.is_empty());
    }
}
