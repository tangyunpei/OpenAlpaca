//! One classification table with two entry points (extension design §4.2, X-7).
//!
//! * [`classify_bringup_failure`] runs at **E2**, where every failure is a
//!   failure and the question is only *which* `FailureReason` the row should
//!   carry — it defaults to `Unreachable` when unsure.
//! * [`classify_call_failure`] runs in the registry's `Mcp` arm, where most
//!   errors are ordinary and only the **terminal** ones may flip a live row to
//!   `Failed`. It returns `Some` for exactly two classes.
//!
//! Honestly bounded, as the design insists: a stdio server that exits on an
//! expired token is **indistinguishable** from one that crashed, so it
//! classifies as `Unreachable`/`Crashed`. Only streamable-HTTP carries a
//! status, and only through the two variants rmcp preserves
//! ([`McpError::Unauthorized`]); its one other typed failure is a redirect the
//! client refused to follow ([`McpError::RedirectRefused`], a bad `url`).
//!
//! A misclassification costs a wrong button label, never a wrong lifecycle.

use openalpaca_mcp::McpError;

use crate::tools::extensions::FailureReason;

/// **E2.** Why did bring-up fail?
///
/// `NeedsConfig` is not produced here: an unresolvable `bearer_env` /
/// `api_key_env` is refused *before* `connect` by the supervisor, which knows
/// the missing key's name and is deliberately stricter than the reference
/// design about it (§4.2, X-31).
pub fn classify_bringup_failure(error: &McpError) -> FailureReason {
    match error {
        McpError::Unauthorized(_) => FailureReason::NeedsAuthorization,
        // The declared `url` leads to a redirect the client refuses; the
        // error's text names the origin and says to set `url` to the
        // canonical endpoint (N-MCP-2).
        McpError::RedirectRefused { .. } => FailureReason::ConfigInvalid,
        // The lifecycle could not be negotiated; the error's text names the
        // `protocol` value to set (DU-25 (m-b)). Before P19 this read
        // `Unreachable`, "not actionable — retry".
        McpError::ProtocolNegotiationFailed(_) => FailureReason::ProtocolNegotiationFailed,
        // Everything else — a command that will not spawn, a handshake that
        // fails, a timeout — is "could not be reached or started". `Crashed`
        // is reserved for a server that *was* running.
        _ => FailureReason::Unreachable,
    }
}

/// **Call time.** Is this error terminal enough to flip an `Enabled` row?
///
/// `Some` only for the two terminal classes:
///
/// * [`McpError::ReconnectExhausted`] → `Crashed`. The client's own terminal
///   state is the trigger — no string matching. Note what that means for a
///   stdio server, because the design states it rather than hiding it: each
///   pre-send `reconnect()` respawns the child and only a successful call
///   resets the count (N-MCP-1), so a dead server reads `active` until **four**
///   consecutive calls have found it dead before sending — whether each
///   respawn failed to handshake or handshook and then died on its call.
/// * [`McpError::Unauthorized`] → `NeedsAuthorization`. It fires mid-session,
///   and taking it out of the retry ladder is what stops an expired token from
///   burning the four reconnect entries and ending as `Failed{Crashed}` with a
///   Retry button that cannot help.
///
/// [`McpError::Closed`] returns `None` deliberately: a sealed client is the
/// *result* of a disable, and `mark_failed` is a no-op outside `Enabled`
/// anyway — but a `None` here says so at the call site.
pub fn classify_call_failure(error: &McpError) -> Option<FailureReason> {
    match error {
        McpError::ReconnectExhausted(_) => Some(FailureReason::Crashed),
        McpError::Unauthorized(_) => Some(FailureReason::NeedsAuthorization),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn bringup_maps_a_401_to_needs_authorization_and_everything_else_to_unreachable() {
        assert_eq!(
            classify_bringup_failure(&McpError::Unauthorized(401)),
            FailureReason::NeedsAuthorization
        );
        assert_eq!(
            classify_bringup_failure(&McpError::Unauthorized(403)),
            FailureReason::NeedsAuthorization
        );
        for error in [
            McpError::HandshakeFailed("boom".into()),
            McpError::Timeout(Duration::from_secs(1)),
            McpError::TransportClosed,
            McpError::Transport(std::io::Error::from(std::io::ErrorKind::NotFound)),
            McpError::ReconnectExhausted(3),
            McpError::Sdk("x".into()),
        ] {
            assert_eq!(
                classify_bringup_failure(&error),
                FailureReason::Unreachable,
                "{error:?}"
            );
        }
    }

    /// A server whose URL relies on a redirect the client refuses is a bad
    /// declaration: the owner sets `url` to the canonical endpoint (N-MCP-2).
    #[test]
    fn bringup_maps_a_refused_redirect_to_config_invalid() {
        assert_eq!(
            classify_bringup_failure(&McpError::RedirectRefused {
                origin: "https://other.example".into(),
            }),
            FailureReason::ConfigInvalid
        );
    }

    /// A failed negotiation is the owner's to fix with `protocol`, not a
    /// retry (DU-25 (m-b)).
    #[test]
    fn bringup_maps_a_failed_negotiation_to_protocol_negotiation_failed() {
        let reason = classify_bringup_failure(&McpError::ProtocolNegotiationFailed(
            r#"ExpectedInitResult(None); set `protocol = "legacy"`"#.into(),
        ));
        assert_eq!(reason, FailureReason::ProtocolNegotiationFailed);
        assert!(reason.actionable());
        assert_eq!(reason.word(), "protocol_negotiation_failed");
    }

    #[test]
    fn only_the_two_terminal_classes_flip_a_live_row() {
        assert_eq!(
            classify_call_failure(&McpError::ReconnectExhausted(3)),
            Some(FailureReason::Crashed)
        );
        assert_eq!(
            classify_call_failure(&McpError::Unauthorized(401)),
            Some(FailureReason::NeedsAuthorization)
        );
        for error in [
            // An ordinary failure: it goes back to the model, and the next
            // call reconnects first only if the transport has closed (N-MCP-1).
            McpError::TransportClosed,
            McpError::Timeout(Duration::from_secs(1)),
            McpError::ServerInternal("x".into()),
            McpError::ToolNotFound("x".into()),
            McpError::Cancelled,
            // The seal is the *result* of a disable, never a crash.
            McpError::Closed,
            // A respawn that cannot renegotiate goes back to the model with
            // the remedy named; only the next load reads it on the row.
            McpError::ProtocolNegotiationFailed("x".into()),
            // A call refused a redirect goes back to the model with the
            // remedy named; it never retries, and the row stays as it is.
            McpError::RedirectRefused {
                origin: "https://other.example".into(),
            },
        ] {
            assert_eq!(classify_call_failure(&error), None, "{error:?}");
        }
    }
}
