//! ConfirmationBroker — interactive tool confirmation via oneshot channels.
//!
//! When a tool requires human confirmation (listed in `require_confirmation_for`),
//! the sandbox creates a oneshot channel, publishes a confirmation request event,
//! and awaits the user's response. The broker coordinates pending confirmations
//! so that any interface (CLI, GUI, Telegram) can deliver the user's decision.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::sync::oneshot;

/// A confirmation request describing the tool call awaiting approval.
pub struct ConfirmationRequest {
    pub request_id: String,
    pub agent_id: String,
    pub tool_name: String,
    pub tool_arguments: serde_json::Value,
    pub stream_id: Option<String>,
    pub lane_key: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// The user's response to a confirmation request.
pub struct ConfirmationResponse {
    pub approved: bool,
}

/// Central broker coordinating pending tool confirmations.
///
/// Each pending confirmation is a oneshot channel: the sandbox awaits the
/// receiver, and the user's interface sends via `respond()`.
pub struct ConfirmationBroker {
    pending: DashMap<String, oneshot::Sender<ConfirmationResponse>>,
}

impl ConfirmationBroker {
    pub fn new() -> Self {
        Self {
            pending: DashMap::new(),
        }
    }

    /// Register a confirmation request. Returns receiver the caller awaits.
    pub fn request(&self, req: &ConfirmationRequest) -> oneshot::Receiver<ConfirmationResponse> {
        let (tx, rx) = oneshot::channel();
        self.pending.insert(req.request_id.clone(), tx);
        rx
    }

    /// Deliver user's response to a pending request.
    pub fn respond(&self, request_id: &str, response: ConfirmationResponse) -> Result<(), String> {
        match self.pending.remove(request_id) {
            Some((_, tx)) => tx.send(response).map_err(|_| "receiver dropped".into()),
            None => Err(format!("No pending confirmation: {request_id}")),
        }
    }

    /// Cancel a pending request (cleanup on timeout).
    pub fn cancel(&self, request_id: &str) {
        self.pending.remove(request_id);
    }

    /// Number of pending confirmations (for diagnostics).
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// List all pending request IDs (for testing / diagnostics).
    pub fn pending_keys(&self) -> Vec<String> {
        self.pending.iter().map(|r| r.key().clone()).collect()
    }
}

impl Default for ConfirmationBroker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_request(id: &str) -> ConfirmationRequest {
        ConfirmationRequest {
            request_id: id.to_string(),
            agent_id: "agent1".to_string(),
            tool_name: "file_write".to_string(),
            tool_arguments: serde_json::json!({"path": "/tmp/test"}),
            stream_id: None,
            lane_key: None,
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_broker_request_respond_approved() {
        let broker = ConfirmationBroker::new();
        let req = make_request("req1");
        let rx = broker.request(&req);

        assert_eq!(broker.pending_count(), 1);

        let result = broker.respond("req1", ConfirmationResponse { approved: true });
        assert!(result.is_ok());

        let response = rx.await.unwrap();
        assert!(response.approved);
        assert_eq!(broker.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_broker_request_respond_denied() {
        let broker = ConfirmationBroker::new();
        let req = make_request("req2");
        let rx = broker.request(&req);

        let result = broker.respond("req2", ConfirmationResponse { approved: false });
        assert!(result.is_ok());

        let response = rx.await.unwrap();
        assert!(!response.approved);
    }

    #[tokio::test]
    async fn test_broker_respond_unknown_id() {
        let broker = ConfirmationBroker::new();
        let result = broker.respond("nonexistent", ConfirmationResponse { approved: true });
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No pending confirmation"));
    }

    #[tokio::test]
    async fn test_broker_cancel() {
        let broker = ConfirmationBroker::new();
        let req = make_request("req3");
        let rx = broker.request(&req);

        assert_eq!(broker.pending_count(), 1);
        broker.cancel("req3");
        assert_eq!(broker.pending_count(), 0);

        // Receiver should get RecvError since sender was dropped
        assert!(rx.await.is_err());
    }

    #[tokio::test]
    async fn test_broker_pending_count() {
        let broker = ConfirmationBroker::new();
        assert_eq!(broker.pending_count(), 0);

        let _rx1 = broker.request(&make_request("a"));
        assert_eq!(broker.pending_count(), 1);

        let _rx2 = broker.request(&make_request("b"));
        assert_eq!(broker.pending_count(), 2);

        broker.cancel("a");
        assert_eq!(broker.pending_count(), 1);
    }
}
