use anyhow::Result;

use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info};

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
                let event = WakeEvent::Timer { job_id, tag };
                if let Err(e) = tx.send(event).await {
                    error!("Failed to send cron wake event: {}", e);
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
            if let Err(e) = tx.send(event).await {
                error!("Failed to send oneshot wake event: {}", e);
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
        let (tx, mut rx) = mpsc::channel::<WakeEvent>(1);
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
}
