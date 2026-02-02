use serde::{Deserialize, Serialize};

/// A task scheduled to run at specific times defined by a Cron expression
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    /// Unique identifier for the task
    pub id: String,
    /// Cron expression (e.g. "0/5 * * * * *")
    pub cron: String,
    /// Tag to identify the type of task or action to trigger
    pub tag: String,
}
