use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

use openalpaca_api::events::WakeEvent;

pub mod filesystem;

/// Trait for wake event watchers
#[async_trait]
pub trait EventWatcher: Send + Sync {
    /// Start watching for events
    async fn start(&self, tx: mpsc::Sender<WakeEvent>) -> Result<()>;

    /// Stop watching
    async fn stop(&self) -> Result<()>;
}
