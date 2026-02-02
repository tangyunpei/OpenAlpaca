use anyhow::Result;

use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{info, warn};

use crate::models::ScheduledTask;
use openalpaca_api::events::WakeEvent;

/// Scheduler for time-based wake events
pub struct WakeScheduler {
    scheduler: JobScheduler,
    event_tx: mpsc::Sender<WakeEvent>,
}

impl WakeScheduler {
    /// Create a new WakeScheduler with an event sender
    pub async fn new(event_tx: mpsc::Sender<WakeEvent>) -> Result<Self> {
        let scheduler = JobScheduler::new().await?;
        Ok(Self {
            scheduler,
            event_tx,
        })
    }

    /// Start the scheduler
    pub async fn start(&self) -> Result<()> {
        self.scheduler.start().await?;
        info!("WakeScheduler started");
        Ok(())
    }

    /// Schedule a recurring task using Cron expression
    pub async fn schedule_cron(&self, task: ScheduledTask) -> Result<()> {
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
                    job_id: job_id.clone(),
                    tag,
                };
                // Use try_send for backpressure consistency (drop if channel full)
                if let Err(e) = tx.try_send(event) {
                    warn!("Cron wake event dropped (channel full or closed): {}", e);
                }
            })
        })?;

        self.scheduler.add(job).await?;
        info!("Scheduled cron task: {} ({})", task.id, task.cron);
        Ok(())
    }

    /// Schedule a one-time task after a delay (Robust testing helper)
    pub async fn schedule_once(&self, delay: Duration, tag: String) {
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let event = WakeEvent::Timer {
                job_id: "oneshot".to_string(),
                tag,
            };
            // Use try_send for backpressure consistency
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
    use tokio::time::timeout;

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
}
