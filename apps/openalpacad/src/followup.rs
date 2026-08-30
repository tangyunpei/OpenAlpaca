//! Daemon-side `FollowupRunner`: re-enters claimed follow-up items through
//! `Gateway::handle_event` as fresh turns (Routing V2).
//!
//! Each item runs with `EventSource::Internal` plus the principal captured
//! when it was queued, so persistence, memory scoping, and security checks
//! all go through the normal front door.

use openalpaca_api::events::EventSource;
use openalpaca_core::gateway::{Gateway, GatewayRequest};
use openalpaca_core::orchestrator::{FollowupItem, FollowupRunner};
use openalpaca_storage::{Database, FollowupRepository};
use std::sync::Arc;
use tracing::{info, warn};

/// Runs follow-up items as fresh gateway turns.
pub struct GatewayFollowupRunner {
    gateway: Arc<Gateway>,
    db: Database,
}

impl GatewayFollowupRunner {
    pub fn new(gateway: Arc<Gateway>, db: Database) -> Self {
        Self { gateway, db }
    }
}

impl FollowupRunner for GatewayFollowupRunner {
    fn spawn_followup(&self, item: FollowupItem) {
        let gateway = self.gateway.clone();
        let db = self.db.clone();
        tokio::spawn(async move {
            info!(
                followup_id = item.id,
                lane_key = %item.lane_key,
                "Running queued follow-up as a fresh turn"
            );
            let response = gateway
                .handle_event(GatewayRequest {
                    source: EventSource::Internal,
                    content: item.content,
                    attachments: Vec::new(),
                    principal: item.principal,
                    scope: item.scope,
                    workspace_path: item.workspace_path,
                    stream_id: None,
                })
                .await;
            if response.is_error {
                warn!(
                    followup_id = item.id,
                    "Follow-up turn returned an error: {}", response.content
                );
            }
            let repo = FollowupRepository::new(&db);
            if let Err(e) = repo.mark_done(item.id) {
                warn!(followup_id = item.id, "Failed to mark follow-up done: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use openalpaca_core::bus::EventBus;
    use openalpaca_core::context::SharedContext;
    use openalpaca_core::gateway::{HandleResult, MessageHandler};
    use openalpaca_core::lane::LaneManager;
    use openalpaca_core::security::policy::{Principal, Scope};
    use std::sync::Mutex;
    use uuid::Uuid;

    /// Stub handler that records what reached it.
    struct StubHandler {
        calls: Arc<Mutex<Vec<(String, String, Principal)>>>,
    }

    #[async_trait]
    impl MessageHandler for StubHandler {
        async fn handle(
            &self,
            _request_id: Uuid,
            source: String,
            content: String,
            principal: Principal,
            _scope: Scope,
            _lane_key: String,
            _workspace_path: Option<String>,
            _stream_id: Option<String>,
        ) -> Result<HandleResult, String> {
            self.calls
                .lock()
                .unwrap()
                .push((source, content, principal));
            Ok(HandleResult::text("ack".to_string()))
        }
    }

    #[tokio::test]
    async fn test_claim_and_run_followup_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();

        // Queue an item and claim it (queued → running), as the finalize hook does.
        let repo = FollowupRepository::new(&db);
        let principal = Principal::User {
            global_id: "junpei".to_string(),
        };
        let principal_json = serde_json::to_string(&principal).unwrap();
        let id = repo
            .queue(
                "junpei:cli",
                "followup",
                "run the follow-up work",
                &principal_json,
                None,
                Some("task-1"),
            )
            .unwrap();
        let claimed = repo.claim_next("junpei:cli").unwrap().unwrap();
        assert_eq!(claimed.id, id);
        assert_eq!(claimed.status, "running");

        // Wire a gateway with a stub handler and run the item.
        let calls = Arc::new(Mutex::new(Vec::new()));
        let gateway = Arc::new(Gateway::new(
            Arc::new(SharedContext::new()),
            Arc::new(LaneManager::new()),
            Arc::new(StubHandler {
                calls: calls.clone(),
            }),
            EventBus::default(),
            Some(db.clone()),
        ));
        let runner = GatewayFollowupRunner::new(gateway, db.clone());
        runner.spawn_followup(FollowupItem {
            id: claimed.id,
            lane_key: claimed.lane_key,
            content: claimed.content,
            principal: serde_json::from_str(&claimed.principal_json).unwrap(),
            scope: Scope::Global,
            workspace_path: claimed.workspace_path,
            source_task_id: claimed.source_task_id,
        });

        // Wait for the spawned turn to reach the handler and mark the row done.
        let row_status = |repo: &FollowupRepository<'_>| repo.get(id).unwrap().unwrap().status;
        for _ in 0..200 {
            if !calls.lock().unwrap().is_empty() && row_status(&repo) == "done" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(row_status(&repo), "done", "row should be marked done");

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "handler should have been called once");
        let (source, content, principal) = &calls[0];
        assert_eq!(source, "internal");
        assert_eq!(content, "run the follow-up work");
        assert_eq!(
            principal,
            &Principal::User {
                global_id: "junpei".to_string()
            }
        );

        // Row is terminal: done, not claimable, not queued.
        assert!(repo.claim_next("junpei:cli").unwrap().is_none());
        assert!(repo.list_queued_by_lane("junpei:cli").unwrap().is_empty());
    }
}
