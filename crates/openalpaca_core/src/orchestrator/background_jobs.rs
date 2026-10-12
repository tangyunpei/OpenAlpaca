//! The orchestrator's post-turn jobs — the conversation summary and the
//! memory extraction — as tracked, cancellable tasks.
//!
//! Design §6 (*Promotion and correction*): automatic extraction "runs
//! cancellably with task attribution, with no fire-and-forget jobs". A bare
//! `tokio::spawn` is exactly a fire-and-forget job: nothing can wait for it,
//! nothing can stop it, and a shutdown that races it loses whatever it was
//! about to write. Every job the orchestrator starts after a turn goes
//! through here instead, so the daemon can cancel and drain them at shutdown
//! and a test can wait for a turn's extraction to have landed.

use std::future::Future;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub struct BackgroundJobs {
    tracker: TaskTracker,
    cancel: CancellationToken,
}

impl Default for BackgroundJobs {
    fn default() -> Self {
        Self::new()
    }
}

impl BackgroundJobs {
    pub fn new() -> Self {
        Self {
            tracker: TaskTracker::new(),
            cancel: CancellationToken::new(),
        }
    }

    /// Starts a job. One that selects on [`Self::cancel_token`] stops when
    /// shutdown begins; one that does not is waited for up to the grace
    /// [`Self::shutdown`] is given.
    pub fn spawn<F>(&self, job: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.tracker.spawn(job);
    }

    /// The token every job is handed: cancelled once, at shutdown.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Jobs started and not yet finished.
    pub fn in_flight(&self) -> usize {
        self.tracker.len()
    }

    /// Waits for every job started so far to finish, cancelling nothing.
    pub async fn wait_idle(&self) {
        self.tracker.close();
        self.tracker.wait().await;
        self.tracker.reopen();
    }

    /// Cancels every job and waits up to `grace` for them to finish. `true`
    /// when they all did.
    pub async fn shutdown(&self, grace: Duration) -> bool {
        self.cancel.cancel();
        self.tracker.close();
        tokio::time::timeout(grace, self.tracker.wait())
            .await
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn wait_idle_returns_once_every_spawned_job_has_finished() {
        let jobs = BackgroundJobs::new();
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        jobs.spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            flag.store(true, Ordering::SeqCst);
        });
        assert_eq!(jobs.in_flight(), 1);
        jobs.wait_idle().await;
        assert!(done.load(Ordering::SeqCst));
        assert_eq!(jobs.in_flight(), 0);
        // Still usable afterwards: the tracker was reopened.
        jobs.spawn(async {});
        jobs.wait_idle().await;
    }

    #[tokio::test]
    async fn shutdown_cancels_a_job_that_listens_and_reports_the_stragglers() {
        let jobs = BackgroundJobs::new();
        let cancel = jobs.cancel_token();
        let stopped = Arc::new(AtomicBool::new(false));
        let flag = stopped.clone();
        jobs.spawn(async move {
            cancel.cancelled().await;
            flag.store(true, Ordering::SeqCst);
        });
        assert!(jobs.shutdown(Duration::from_secs(1)).await);
        assert!(stopped.load(Ordering::SeqCst));

        let deaf = BackgroundJobs::new();
        deaf.spawn(async {
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        assert!(
            !deaf.shutdown(Duration::from_millis(30)).await,
            "a job that ignores the token is reported, not waited for forever"
        );
    }
}
