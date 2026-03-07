use std::collections::HashMap;
use std::sync::Mutex;

/// Status of a background-spawned subagent.
#[derive(Debug, Clone)]
pub enum SubagentStatus {
    Queued,
    Running,
    Completed { content: String, success: bool },
    Failed { error: String },
}

/// Shared tracker for background subagent tasks.
/// Allows the lead agent to spawn multiple subagents concurrently
/// and check/wait for their results.
pub struct SubagentTracker {
    pub statuses: Mutex<HashMap<String, SubagentStatus>>,
    /// Notifies waiters when a subagent completes or fails.
    pub notify: tokio::sync::Notify,
}

impl Default for SubagentTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentTracker {
    pub fn new() -> Self {
        Self {
            statuses: Mutex::new(HashMap::new()),
            notify: tokio::sync::Notify::new(),
        }
    }

    pub fn register(&self, run_id: &str) {
        let mut map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(run_id.to_string(), SubagentStatus::Queued);
    }

    pub fn complete(&self, run_id: &str, content: String, success: bool) {
        let mut map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(
            run_id.to_string(),
            SubagentStatus::Completed { content, success },
        );
        drop(map);
        self.notify.notify_waiters();
    }

    pub fn fail(&self, run_id: &str, error: String) {
        let mut map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(run_id.to_string(), SubagentStatus::Failed { error });
        drop(map);
        self.notify.notify_waiters();
    }

    pub fn set_status(&self, run_id: &str, status: SubagentStatus) {
        let mut map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(run_id.to_string(), status);
        drop(map);
        self.notify.notify_waiters();
    }

    pub fn get(&self, run_id: &str) -> Option<SubagentStatus> {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.get(run_id).cloned()
    }

    pub fn all_done(&self) -> bool {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.values().all(|s| {
            matches!(
                s,
                SubagentStatus::Completed { .. } | SubagentStatus::Failed { .. }
            )
        })
    }

    pub fn status_counts(&self) -> (usize, usize, usize, usize) {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        let (mut queued, mut running, mut completed, mut failed) = (0, 0, 0, 0);
        for s in map.values() {
            match s {
                SubagentStatus::Queued => queued += 1,
                SubagentStatus::Running => running += 1,
                SubagentStatus::Completed { .. } => completed += 1,
                SubagentStatus::Failed { .. } => failed += 1,
            }
        }
        (queued, running, completed, failed)
    }

    pub fn summary(&self) -> String {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        if map.is_empty() {
            return "No subagents have been spawned yet.".to_string();
        }
        let mut parts = Vec::new();
        for (id, status) in map.iter() {
            match status {
                SubagentStatus::Queued => {
                    parts.push(format!("- **{}**: queued (waiting for execution slot)", id));
                }
                SubagentStatus::Running => {
                    parts.push(format!("- **{}**: still running", id));
                }
                SubagentStatus::Completed { content, success } => {
                    let preview: String = content.chars().take(500).collect();
                    parts.push(format!(
                        "- **{}**: {} — {}",
                        id,
                        if *success {
                            "completed"
                        } else {
                            "completed (partial)"
                        },
                        preview
                    ));
                }
                SubagentStatus::Failed { error } => {
                    parts.push(format!("- **{}**: failed — {}", id, error));
                }
            }
        }
        parts.join("\n")
    }
}
