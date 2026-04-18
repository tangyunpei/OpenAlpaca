//! ConfirmationBroker — interactive tool confirmation via oneshot channels.
//!
//! When a tool requires human confirmation (listed in `require_confirmation_for`),
//! the sandbox creates a oneshot channel, publishes a confirmation request event,
//! and awaits the user's response. The broker coordinates pending confirmations
//! so that any interface (CLI, GUI, Telegram) can deliver the user's decision.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::{DashMap, DashSet};
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConfirmationResponse {
    pub approved: bool,
    /// Granularity of the approval — only meaningful when `approved = true`.
    /// Missing/`None` defaults to `TheseArgs` at enforcement time (safest).
    #[serde(default)]
    pub approval_scope: Option<ApprovalScope>,
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

/// The granularity of an approval decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScope {
    /// Approve invocations with the exact same arguments as this one.
    /// Cached by (tool_name, args_hash).
    TheseArgs,
    /// Approve all invocations of this tool regardless of arguments.
    /// Cached by tool_name only.
    EntireTool,
}

/// Session-scoped approval cache. Lock-free via DashSet; cheap to clone (internal Arc).
/// Cleared on daemon restart.
#[derive(Default, Clone)]
pub struct ApprovalCache {
    tool_approvals: Arc<DashSet<String>>,
    args_approvals: Arc<DashSet<(String, u64)>>,
}

impl ApprovalCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an approval decision.
    pub fn record(&self, tool_name: &str, args_hash: u64, scope: ApprovalScope) {
        match scope {
            ApprovalScope::TheseArgs => {
                self.args_approvals
                    .insert((tool_name.to_string(), args_hash));
            }
            ApprovalScope::EntireTool => {
                self.tool_approvals.insert(tool_name.to_string());
            }
        }
    }

    /// Check if an invocation is pre-approved.
    pub fn is_approved(&self, tool_name: &str, args_hash: u64) -> bool {
        self.tool_approvals.contains(tool_name)
            || self
                .args_approvals
                .contains(&(tool_name.to_string(), args_hash))
    }

    /// Clear all approvals. Useful for tests and future manual-reset UX.
    #[allow(dead_code)]
    pub fn clear(&self) {
        self.tool_approvals.clear();
        self.args_approvals.clear();
    }
}

/// Canonicalize a JSON value's object keys recursively so semantically-equal
/// arguments (differing only in key order) hash to the same value.
pub fn canonicalize_json(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let sorted: std::collections::BTreeMap<_, _> = map
                .iter()
                .map(|(k, v)| (k.clone(), canonicalize_json(v)))
                .collect();
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonicalize_json).collect())
        }
        other => other.clone(),
    }
}

/// Compute a 64-bit hash of tool arguments, order-insensitive over object keys.
pub fn hash_canonical_args(args: &serde_json::Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let canonical = canonicalize_json(args);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical.to_string().hash(&mut hasher);
    hasher.finish()
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

        let result = broker.respond(
            "req1",
            ConfirmationResponse {
                approved: true,
                approval_scope: None,
            },
        );
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

        let result = broker.respond(
            "req2",
            ConfirmationResponse {
                approved: false,
                approval_scope: None,
            },
        );
        assert!(result.is_ok());

        let response = rx.await.unwrap();
        assert!(!response.approved);
    }

    #[tokio::test]
    async fn test_broker_respond_unknown_id() {
        let broker = ConfirmationBroker::new();
        let result = broker.respond(
            "nonexistent",
            ConfirmationResponse {
                approved: true,
                approval_scope: None,
            },
        );
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

    #[test]
    fn approval_cache_these_args_bypasses_same_args() {
        let cache = ApprovalCache::new();
        cache.record("shell_execute", 0x12345, ApprovalScope::TheseArgs);
        assert!(cache.is_approved("shell_execute", 0x12345));
    }

    #[test]
    fn approval_cache_these_args_rejects_different_args() {
        let cache = ApprovalCache::new();
        cache.record("shell_execute", 0x12345, ApprovalScope::TheseArgs);
        assert!(!cache.is_approved("shell_execute", 0x99999));
    }

    #[test]
    fn approval_cache_entire_tool_bypasses_all_args() {
        let cache = ApprovalCache::new();
        cache.record("shell_execute", 0x12345, ApprovalScope::EntireTool);
        assert!(cache.is_approved("shell_execute", 0x12345));
        assert!(cache.is_approved("shell_execute", 0x99999));
        assert!(cache.is_approved("shell_execute", 0));
    }

    #[test]
    fn approval_cache_different_tool_not_approved() {
        let cache = ApprovalCache::new();
        cache.record("shell_execute", 0x12345, ApprovalScope::EntireTool);
        assert!(!cache.is_approved("file_write", 0x12345));
    }

    #[test]
    fn approval_cache_clear_removes_all() {
        let cache = ApprovalCache::new();
        cache.record("shell_execute", 0x12345, ApprovalScope::TheseArgs);
        cache.record("file_write", 0, ApprovalScope::EntireTool);
        cache.clear();
        assert!(!cache.is_approved("shell_execute", 0x12345));
        assert!(!cache.is_approved("file_write", 0));
    }

    #[test]
    fn hash_canonical_args_order_insensitive() {
        let a = serde_json::json!({"a": 1, "b": 2});
        let b = serde_json::json!({"b": 2, "a": 1});
        assert_eq!(hash_canonical_args(&a), hash_canonical_args(&b));
    }

    #[test]
    fn hash_canonical_args_nested_objects() {
        let a = serde_json::json!({ "outer": {"a": 1, "b": 2}, "x": [1,2] });
        let b = serde_json::json!({ "x": [1,2], "outer": {"b": 2, "a": 1} });
        assert_eq!(hash_canonical_args(&a), hash_canonical_args(&b));
    }

    #[test]
    fn hash_canonical_args_array_order_matters() {
        let a = serde_json::json!([1, 2, 3]);
        let b = serde_json::json!([3, 2, 1]);
        assert_ne!(hash_canonical_args(&a), hash_canonical_args(&b));
    }

    #[test]
    fn confirmation_response_deserialize_missing_scope_defaults_to_none() {
        let json = r#"{"approved": true}"#;
        let resp: ConfirmationResponse = serde_json::from_str(json).unwrap();
        assert!(resp.approved);
        assert!(resp.approval_scope.is_none());
    }

    #[test]
    fn confirmation_response_deserialize_these_args() {
        let json = r#"{"approved": true, "approval_scope": "these_args"}"#;
        let resp: ConfirmationResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.approval_scope, Some(ApprovalScope::TheseArgs));
    }

    #[test]
    fn confirmation_response_deserialize_entire_tool() {
        let json = r#"{"approved": true, "approval_scope": "entire_tool"}"#;
        let resp: ConfirmationResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.approval_scope, Some(ApprovalScope::EntireTool));
    }
}
