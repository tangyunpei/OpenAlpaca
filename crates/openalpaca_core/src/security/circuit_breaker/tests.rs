use super::*;
use crate::bus::EventBus;

fn make_config(threshold: usize, timeout_secs: u64) -> CircuitBreakerConfig {
    CircuitBreakerConfig {
        enabled: true,
        failure_threshold: threshold,
        reset_timeout_secs: timeout_secs,
    }
}

fn make_cb(threshold: usize) -> ToolCircuitBreaker {
    ToolCircuitBreaker::new(&make_config(threshold, 300), EventBus::default())
}

#[test]
fn test_closed_allows_calls() {
    let cb = make_cb(5);
    assert!(cb.check("agent1", "web_search").is_ok());
    assert!(cb.check("agent1", "web_search").is_ok());
}

#[test]
fn test_opens_after_threshold() {
    let cb = make_cb(3);

    // 3 consecutive failures should open the circuit
    cb.record_failure("agent1", "tool_x");
    cb.record_failure("agent1", "tool_x");
    assert!(cb.check("agent1", "tool_x").is_ok()); // still closed after 2
    let tripped = cb.record_failure("agent1", "tool_x");
    assert!(tripped); // 3rd failure trips it
    assert!(cb.check("agent1", "tool_x").is_err()); // now open
}

#[test]
fn test_resets_on_success() {
    let cb = make_cb(3);

    cb.record_failure("agent1", "tool_x");
    cb.record_failure("agent1", "tool_x");
    // 2 failures, then success resets
    cb.record_success("agent1", "tool_x");
    assert!(cb.check("agent1", "tool_x").is_ok());

    // Need 3 more failures to trip again
    cb.record_failure("agent1", "tool_x");
    cb.record_failure("agent1", "tool_x");
    assert!(cb.check("agent1", "tool_x").is_ok()); // still closed
    cb.record_failure("agent1", "tool_x");
    assert!(cb.check("agent1", "tool_x").is_err()); // now open
}

#[test]
fn test_half_open_after_timeout() {
    // Use a very short timeout for testing
    let config = CircuitBreakerConfig {
        enabled: true,
        failure_threshold: 1,
        reset_timeout_secs: 0, // instant reset for test
    };
    let cb = ToolCircuitBreaker::new(&config, EventBus::default());

    cb.record_failure("agent1", "tool_x");
    // Circuit is open, but timeout is 0 so it immediately transitions to half-open
    assert!(cb.check("agent1", "tool_x").is_ok()); // transitions to half-open, allows probe
}

#[test]
fn test_half_open_success_closes() {
    let config = CircuitBreakerConfig {
        enabled: true,
        failure_threshold: 1,
        reset_timeout_secs: 0,
    };
    let cb = ToolCircuitBreaker::new(&config, EventBus::default());

    cb.record_failure("agent1", "tool_x"); // opens
    assert!(cb.check("agent1", "tool_x").is_ok()); // half-open, probe allowed
    cb.record_success("agent1", "tool_x"); // closes
    assert!(cb.check("agent1", "tool_x").is_ok()); // now closed again
}

#[test]
fn test_half_open_failure_reopens() {
    let config = CircuitBreakerConfig {
        enabled: true,
        failure_threshold: 1,
        reset_timeout_secs: 0,
    };
    let cb = ToolCircuitBreaker::new(&config, EventBus::default());

    cb.record_failure("agent1", "tool_x"); // opens
    assert!(cb.check("agent1", "tool_x").is_ok()); // half-open, probe allowed
    cb.record_failure("agent1", "tool_x"); // probe failed, re-opens

    // Now it's open again with reset_timeout=0, so next check transitions to half-open
    assert!(cb.check("agent1", "tool_x").is_ok()); // half-open again
}

#[test]
fn test_agent_isolation() {
    let cb = make_cb(2);

    // Agent A fails twice → open
    cb.record_failure("agent_a", "tool_x");
    cb.record_failure("agent_a", "tool_x");
    assert!(cb.check("agent_a", "tool_x").is_err());

    // Agent B should still be able to use the same tool
    assert!(cb.check("agent_b", "tool_x").is_ok());
}

#[test]
fn test_tool_isolation() {
    let cb = make_cb(2);

    // tool_x fails twice → open
    cb.record_failure("agent1", "tool_x");
    cb.record_failure("agent1", "tool_x");
    assert!(cb.check("agent1", "tool_x").is_err());

    // tool_y should still work for the same agent
    assert!(cb.check("agent1", "tool_y").is_ok());
}

#[test]
fn test_disabled_does_nothing() {
    let config = CircuitBreakerConfig {
        enabled: false,
        failure_threshold: 1,
        reset_timeout_secs: 300,
    };
    let cb = ToolCircuitBreaker::new(&config, EventBus::default());

    // Even after many failures, calls should still be allowed
    for _ in 0..10 {
        cb.record_failure("agent1", "tool_x");
    }
    assert!(cb.check("agent1", "tool_x").is_ok());
}

#[test]
fn test_transient_error_classification() {
    // Transient errors (should trip)
    assert!(is_transient_tool_error(
        "HTTP request failed: connection refused"
    ));
    assert!(is_transient_tool_error(
        "Tool 'api_call' timed out after 60s"
    ));
    assert!(is_transient_tool_error("HTTP 503 — Service Unavailable"));
    assert!(is_transient_tool_error("HTTP 500 — Internal Server Error"));
    assert!(is_transient_tool_error("Connection refused"));
    assert!(is_transient_tool_error("connection reset by peer"));
    assert!(is_transient_tool_error("network error"));
    assert!(is_transient_tool_error(
        "Command failed (exit 1): something broke"
    ));
    assert!(is_transient_tool_error("Command timed out after 30s"));

    // Permanent errors (should NOT trip)
    assert!(!is_transient_tool_error("HTTP 404 — Not Found"));
    assert!(!is_transient_tool_error("HTTP 403 — Forbidden"));
    assert!(!is_transient_tool_error("Unknown tool: nonexistent"));
    assert!(!is_transient_tool_error("Access denied by policy"));
    assert!(!is_transient_tool_error(
        "Command failed (exit 127): not found"
    ));
    assert!(!is_transient_tool_error(
        "Invalid argument: missing required field"
    ));
}

#[test]
fn test_tripped_event_emitted() {
    let bus = EventBus::default();
    let mut rx = bus.subscribe();
    let config = make_config(2, 300);
    let cb = ToolCircuitBreaker::new(&config, bus);

    cb.record_failure("agent1", "web_search");
    let tripped = cb.record_failure("agent1", "web_search");
    assert!(tripped);

    let event = rx.try_recv().unwrap();
    match event {
        SystemEvent::CircuitBreakerTripped {
            agent_id,
            tool_name,
            consecutive_failures,
            reset_after_secs,
            ..
        } => {
            assert_eq!(agent_id, "agent1");
            assert_eq!(tool_name, "web_search");
            assert_eq!(consecutive_failures, 2);
            assert_eq!(reset_after_secs, 300);
        }
        other => panic!("Expected CircuitBreakerTripped, got: {:?}", other),
    }
}
