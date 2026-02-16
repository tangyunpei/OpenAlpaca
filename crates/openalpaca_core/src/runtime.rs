use std::panic::AssertUnwindSafe;

/// A safe wrapper for executing Agent logic.
/// It catches panics and ensures the daemon doesn't crash.
#[derive(Default)]
pub struct AgentRuntime {
    // Intentionally empty — runtime metrics and resource limits
    // are tracked at the orchestrator/router level.
}

impl AgentRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// Execute a closure safely.
    /// If the closure panics, return an error.
    pub async fn run_safe<F, Fut, T>(&self, task: F) -> Result<T, String>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        // Wrap in AssertUnwindSafe because we trust that the task won't leave shared state broken
        // (or rather, we accept the risk for the sake of survival).
        let handle = tokio::spawn(AssertUnwindSafe(task()));

        match handle.await {
            Ok(result) => Ok(result),
            Err(join_err) => {
                if join_err.is_cancelled() {
                    Err("Task cancelled".to_string())
                } else {
                    Err(format!("Agent Panic: {}", join_err))
                }
            }
        }
    }
}
