use serde::{Deserialize, Serialize};

use super::error::ErrorShape;

/// Wire protocol frame types for WebSocket communication.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum GatewayFrame {
    #[serde(rename = "req")]
    Request {
        id: String,
        method: String,
        params: Option<serde_json::Value>,
    },
    #[serde(rename = "res")]
    Response {
        id: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        payload: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<ErrorShape>,
    },
    #[serde(rename = "event")]
    Event {
        event: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        payload: Option<serde_json::Value>,
    },
}

impl GatewayFrame {
    pub fn ok_response(id: String, payload: serde_json::Value) -> Self {
        Self::Response {
            id,
            ok: true,
            payload: Some(payload),
            error: None,
        }
    }

    pub fn err_response(id: String, error: ErrorShape) -> Self {
        Self::Response {
            id,
            ok: false,
            payload: None,
            error: Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_frame_serde() {
        let frame = GatewayFrame::Request {
            id: "1".into(),
            method: "health".into(),
            params: None,
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"type\":\"req\""));
        assert!(json.contains("\"method\":\"health\""));

        let parsed: GatewayFrame = serde_json::from_str(&json).unwrap();
        if let GatewayFrame::Request { id, method, .. } = parsed {
            assert_eq!(id, "1");
            assert_eq!(method, "health");
        } else {
            panic!("expected Request frame");
        }
    }

    #[test]
    fn test_response_frame_serde() {
        let frame = GatewayFrame::ok_response("2".into(), serde_json::json!({"ok": true}));
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"type\":\"res\""));
        assert!(json.contains("\"ok\":true"));
    }

    #[test]
    fn test_error_response_serde() {
        let frame = GatewayFrame::err_response("3".into(), ErrorShape::method_not_found("foo.bar"));
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"ok\":false"));
        assert!(json.contains("METHOD_NOT_FOUND"));
    }

    #[test]
    fn test_event_frame_serde() {
        let frame = GatewayFrame::Event {
            event: "config.changed".into(),
            payload: Some(serde_json::json!({"hash": "abc123"})),
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"type\":\"event\""));
        assert!(json.contains("\"event\":\"config.changed\""));
    }
}
