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
//! ([`McpError::Unauthorized`]).
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
        // Everything else — a command that will not spawn, a handshake that
        // fails, a timeout, a protocol mismatch — is "could not be reached or
        // started". `Crashed` is reserved for a server that *was* running.
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
///   `reconnect()` respawns the child, so a dead server reads `active` until
///   **four** consecutive calls have failed to re-handshake.
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
            // An ordinary failure the reconnect ladder is still working on.
            McpError::TransportClosed,
            McpError::Timeout(Duration::from_secs(1)),
            McpError::ServerInternal("x".into()),
            McpError::ToolNotFound("x".into()),
            McpError::Cancelled,
            // The seal is the *result* of a disable, never a crash.
            McpError::Closed,
        ] {
            assert_eq!(classify_call_failure(&error), None, "{error:?}");
        }
    }
}
