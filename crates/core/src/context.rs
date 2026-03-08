use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

/// Shared application context passed via Arc — replaces TS createDefaultDeps pattern.
/// All subsystems observe `shutdown` to coordinate graceful shutdown.
pub struct AppContext {
    pub config_path: PathBuf,
    pub state_dir: PathBuf,
    pub home_dir: PathBuf,
    pub http_client: reqwest::Client,
    pub shutdown: CancellationToken,
}

#[cfg(test)]
mod tests {
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn test_cancellation_propagates_to_child() {
        let parent = CancellationToken::new();
        let child = parent.child_token();

        assert!(!child.is_cancelled());
        parent.cancel();
        assert!(child.is_cancelled());
    }

    #[tokio::test]
    async fn test_child_cancel_does_not_affect_parent() {
        let parent = CancellationToken::new();
        let child = parent.child_token();

        child.cancel();
        assert!(!parent.is_cancelled());
    }
}
